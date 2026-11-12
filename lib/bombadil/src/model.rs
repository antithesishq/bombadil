//! Action keys and state keys, used to learn action weights from traces and
//! to apply them (`--weights`). Traces record each step's action vocabulary
//! (the available templates and the one taken). Actions are identified by
//! their template's `category_hash` under [`StableHasher`].
//!
//! A state has named 64-bit hashes: `snapshot.<name>` of each of the spec's
//! extracts, and the driver's own ([`RunState::hashes`]). The state key is
//! the tuple of the hashes the weights file names ([`state_key`]), and a
//! learned state applies when the tuple is equal. The hashes have a fixed
//! algorithm (FNV-1a, SplitMix64's finalizer), as training computes the
//! same keys from traces.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::hash::Hasher;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail};
use bombadil_schema::{ActionKey, Time};
use rand::{RngExt, TryRng};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::driver::{ActionTemplate, InterfaceSession, RunState};
use crate::hash::{FNV_OFFSET, fnv1a64_from};
use crate::runner::{ControlFlow, PropertiesState, RunStrategy};
use crate::specification::domain::Snapshot;
use crate::tree::{Leaves, Tree};

/// What a trace records about each step for training.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StepModel {
    /// Key ([`action_key`]) of each available action template, with the
    /// probability the spec weights give it.
    pub actions_available: Vec<(ActionKey, f64)>,
    /// Key of the template that generated the action leading to this
    /// state.
    pub action_taken: Option<ActionKey>,
}

impl StepModel {
    pub fn new<A, T: ActionTemplate<A>>(
        actions_available: &Leaves<'_, T>,
        action_taken: Option<ActionKey>,
    ) -> Self {
        let actions_available = actions_available
            .as_slice()
            .iter()
            .map(|(template, probability)| {
                (action_key(*template), *probability)
            })
            .collect();
        StepModel {
            actions_available,
            action_taken,
        }
    }
}

/// An action and the key of the template it came from.
#[derive(Clone, Debug, PartialEq)]
pub struct Picked<A> {
    pub action: A,
    pub template: ActionKey,
}

impl<A> Picked<A> {
    pub fn generate<R: TryRng + RngExt>(
        template: &impl ActionTemplate<A>,
        rng: &mut R,
    ) -> Self {
        Picked {
            action: template.generate(rng),
            template: action_key(template),
        }
    }
}

/// Key of the template among `actions_available` that an action replayed
/// from a trace is credited to: the first that accepts and produced it, or
/// the first that accepts it when none did.
pub fn accepting_key<A, T: ActionTemplate<A>>(
    actions_available: &Leaves<'_, T>,
    action: &A,
) -> Option<ActionKey> {
    let mut accepting = None;
    for (template, _) in actions_available.as_slice() {
        if template.accepts(action) {
            if template.produced(action) {
                return Some(action_key(*template));
            }
            accepting.get_or_insert(*template);
        }
    }
    accepting.map(action_key)
}

/// The table `--action-templates` writes: the template behind each key in a
/// trace's `actions_available`, in order of first sight. A key ignores some
/// of its template's parameters (see `category_hash`), and the table holds
/// the first template seen under it.
#[derive(Clone, Default)]
pub struct TemplateRegistry(Arc<Mutex<TemplateTable>>);

#[derive(Default)]
struct TemplateTable {
    keys: HashSet<ActionKey>,
    /// One `{"key": …, "template": …}` JSON object per key, each on its
    /// own line.
    lines: Vec<u8>,
}

#[derive(Serialize)]
struct TemplateRow<'a, T> {
    key: ActionKey,
    template: &'a T,
}

impl TemplateRegistry {
    /// Adds the templates of `leaves` whose keys are new. `model` is the
    /// step model built from the same leaves.
    pub fn record<T: Serialize>(
        &self,
        leaves: &Leaves<'_, T>,
        model: &StepModel,
    ) -> Result<()> {
        let leaves = leaves.as_slice();
        let keys = &model.actions_available;
        debug_assert_eq!(leaves.len(), keys.len());

        let mut unseen = Vec::new();
        {
            let table = self.0.lock().expect("template table poisoned");
            for (index, (key, _)) in keys.iter().enumerate() {
                if !table.keys.contains(key) {
                    unseen.push((index, 0));
                }
            }
        }
        if unseen.is_empty() {
            return Ok(());
        }

        // Serialized outside the lock, which other runs take at every step.
        let mut lines = Vec::new();
        for (index, line_end) in &mut unseen {
            let row = TemplateRow {
                key: keys[*index].0,
                template: leaves[*index].0,
            };
            serde_json::to_writer(&mut lines, &row)
                .context("serializing an action template")?;
            lines.push(b'\n');
            *line_end = lines.len();
        }

        let mut table = self.0.lock().expect("template table poisoned");
        let mut line_start = 0;
        for (index, line_end) in unseen {
            if table.keys.insert(keys[index].0) {
                table.lines.extend_from_slice(&lines[line_start..line_end]);
            }
            line_start = line_end;
        }
        Ok(())
    }

    pub fn write(&self, writer: &mut impl Write) -> std::io::Result<()> {
        let table = self.0.lock().expect("template table poisoned");
        writer.write_all(&table.lines)
    }
}

/// The template's `category_hash` under [`StableHasher`].
pub fn action_key<A>(template: &impl ActionTemplate<A>) -> ActionKey {
    let mut hasher = StableHasher::default();
    template.category_hash(&mut hasher);
    ActionKey(hasher.finish())
}

/// FNV-1a, with integers written little-endian and `usize` as 64 bits, so
/// that action keys, which are recorded in traces, are the same on every
/// platform. What a template writes still comes from std's `Hash` impls
/// (string terminators, length prefixes), which Rust does not promise to
/// keep; the pinned keys in the drivers' tests would show a change.
pub struct StableHasher(u64);

impl Default for StableHasher {
    fn default() -> Self {
        StableHasher(FNV_OFFSET)
    }
}

impl Hasher for StableHasher {
    fn write(&mut self, bytes: &[u8]) {
        self.0 = fnv1a64_from(self.0, bytes.iter().copied());
    }

    fn write_u16(&mut self, value: u16) {
        self.write(&value.to_le_bytes());
    }

    fn write_u32(&mut self, value: u32) {
        self.write(&value.to_le_bytes());
    }

    fn write_u64(&mut self, value: u64) {
        self.write(&value.to_le_bytes());
    }

    fn write_u128(&mut self, value: u128) {
        self.write(&value.to_le_bytes());
    }

    fn write_usize(&mut self, value: usize) {
        self.write_u64(value as u64);
    }

    fn write_isize(&mut self, value: isize) {
        self.write_u64(value as i64 as u64);
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

/// The hash of a sequence of string features: FNV-1a over the features,
/// each followed by a newline, then SplitMix64's finalizer. `None` for no
/// features.
pub fn feature_hash<'a>(
    features: impl IntoIterator<Item = &'a str>,
) -> Option<u64> {
    let mut features = features.into_iter();
    let mut hasher = FeatureHasher::new(features.next()?.as_bytes());
    for feature in features {
        hasher.feature(feature.as_bytes());
    }
    Some(hasher.finish())
}

/// [`feature_hash`] one feature at a time. It starts with a feature, as no
/// features have no hash.
#[derive(Clone, Copy, Debug)]
pub struct FeatureHasher(u64);

impl FeatureHasher {
    pub fn new(feature: &[u8]) -> Self {
        let mut hasher = FeatureHasher(FNV_OFFSET);
        hasher.feature(feature);
        hasher
    }

    pub fn feature(&mut self, feature: &[u8]) {
        self.0 = fnv1a64_from(self.0, feature.iter().copied());
        self.0 = fnv1a64_from(self.0, *b"\n");
    }

    pub fn finish(self) -> u64 {
        mix64(self.0)
    }
}

fn mix64(x: u64) -> u64 {
    let mut h = x.wrapping_add(0x9e3779b97f4a7c15);
    h = (h ^ (h >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94d049bb133111eb);
    h ^ (h >> 31)
}

/// [`feature_hash`] of lines that are written in pieces.
struct LineHasher {
    hash: u64,
    lines: bool,
}

impl LineHasher {
    fn new() -> Self {
        LineHasher {
            hash: FNV_OFFSET,
            lines: false,
        }
    }

    fn bytes(&mut self, bytes: &[u8]) {
        self.hash = fnv1a64_from(self.hash, bytes.iter().copied());
    }

    fn line_end(&mut self) {
        self.bytes(b"\n");
        self.lines = true;
    }

    fn finish(self) -> Option<u64> {
        self.lines.then(|| mix64(self.hash))
    }
}

impl Write for LineHasher {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.bytes(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The hash of a snapshot's value: [`feature_hash`] of `path=value` for
/// every scalar in it, e.g. `a.0=1` for `{"a": [1]}`. Object keys are
/// visited in sorted order, as the order is part of the hash. `None` when
/// the value holds no scalar. `path` is scratch space.
fn snapshot_hash(value: &serde_json::Value, path: &mut String) -> Option<u64> {
    let mut hasher = LineHasher::new();
    path.clear();
    hash_leaves(value, path, &mut Vec::new(), &mut hasher);
    hasher.finish()
}

/// `entries` is a stack of the object entries being visited, shared by the
/// whole walk so that sorting them takes one allocation.
fn hash_leaves<'a>(
    value: &'a serde_json::Value,
    path: &mut String,
    entries: &mut Vec<(&'a str, &'a serde_json::Value)>,
    hasher: &mut LineHasher,
) {
    match value {
        serde_json::Value::Object(map) => {
            let start = entries.len();
            entries.extend(map.iter().map(|(key, item)| (key.as_str(), item)));
            entries[start..].sort_unstable_by_key(|(key, _)| *key);
            for index in start..entries.len() {
                let (key, item) = entries[index];
                let parent = path.len();
                if parent > 0 {
                    path.push('.');
                }
                path.push_str(key);
                hash_leaves(item, path, entries, hasher);
                path.truncate(parent);
            }
            entries.truncate(start);
        }
        serde_json::Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                let parent = path.len();
                if parent > 0 {
                    path.push('.');
                }
                write!(path, "{index}").expect("writing to a String");
                hash_leaves(item, path, entries, hasher);
                path.truncate(parent);
            }
        }
        scalar => {
            hasher.bytes(path.as_bytes());
            hasher.bytes(b"=");
            serde_json::to_writer(&mut *hasher, scalar)
                .expect("writing to a hasher");
            hasher.line_end();
        }
    }
}

const SNAPSHOT_PREFIX: &str = "snapshot.";

/// The hash a state has under `name`: for `snapshot.<name>` the
/// [`snapshot_hash`] of the spec's extract of that name, unless it is
/// excluded from the key, and for any other name the driver's hash.
fn component_hash(
    name: &str,
    state: &(impl RunState + ?Sized),
    snapshots: &[Snapshot],
    path: &mut String,
) -> Option<u64> {
    match name.strip_prefix(SNAPSHOT_PREFIX) {
        Some(name) => snapshots
            .iter()
            .rev()
            .filter(|snapshot| {
                !snapshot.excluded && snapshot.name.as_deref() == Some(name)
            })
            .find_map(|snapshot| snapshot_hash(&snapshot.value, path)),
        None => state.hashes().get(name).copied(),
    }
}

/// Which named hashes form a state's key ([`state_key`]).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyLookup {
    /// The structural components, in name order.
    #[serde(default)]
    components: Vec<String>,
    /// The goal components, each with its idle value (`null`: absent). A
    /// goal is active when its hash is present and off its idle value.
    #[serde(default)]
    goals: BTreeMap<String, Option<u64>>,
}

impl KeyLookup {
    fn new(
        mut components: Vec<String>,
        goals: BTreeMap<String, Option<u64>>,
    ) -> Result<Self> {
        if components.is_empty() && goals.is_empty() {
            bail!("the key names no components and no goals");
        }
        components.sort_unstable();
        if let Some(pair) =
            components.windows(2).find(|pair| pair[0] == pair[1])
        {
            bail!("the key names the component {} twice", pair[0]);
        }
        Ok(KeyLookup { components, goals })
    }

    /// The keyed `snapshot.<name>` components that the spec behind
    /// `snapshots` never gives a hash: it records no snapshot of that name,
    /// or excludes it from the key.
    fn unservable<'a>(
        &'a self,
        snapshots: &'a [Snapshot],
    ) -> impl Iterator<Item = &'a str> {
        self.components
            .iter()
            .chain(self.goals.keys())
            .map(String::as_str)
            .filter(|component| {
                component.strip_prefix(SNAPSHOT_PREFIX).is_some_and(|name| {
                    !snapshots.iter().any(|snapshot| {
                        !snapshot.excluded
                            && snapshot.name.as_deref() == Some(name)
                    })
                })
            })
    }
}

/// The key of a state: [`feature_hash`] of the `name=hash` lines (the hash
/// as 16 hex digits) of its keyed components, in name order. While any goal
/// is active the key is the active goals alone, otherwise the structural
/// components that are present. `None` when nothing keyed is present.
/// `hash_of` gives the state's hash under a name.
fn state_key<'a>(
    lookup: &'a KeyLookup,
    mut hash_of: impl FnMut(&'a str) -> Option<u64>,
) -> Option<u64> {
    let mut hasher = LineHasher::new();
    let mut line = |name: &str, hash: u64| {
        hasher.bytes(name.as_bytes());
        hasher.bytes(b"=");
        hasher.bytes(&hex_digits(hash));
        hasher.line_end();
    };
    let mut active = false;
    for (name, idle) in &lookup.goals {
        if let Some(hash) = hash_of(name)
            && Some(hash) != *idle
        {
            line(name, hash);
            active = true;
        }
    }
    if !active {
        for name in &lookup.components {
            if let Some(hash) = hash_of(name) {
                line(name, hash);
            }
        }
    }
    hasher.finish()
}

fn hex_digits(value: u64) -> [u8; 16] {
    let mut digits = [0; 16];
    for (index, digit) in digits.iter_mut().enumerate() {
        let nibble = (value >> (60 - 4 * index)) & 0xf;
        *digit = b"0123456789abcdef"[nibble as usize];
    }
    digits
}

/// A state key as a weights file writes it: 16 lowercase hex digits.
fn parse_state_key(digits: &str) -> Result<u64> {
    if digits.len() != 16
        || !digits
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        bail!("state key {digits:?} is not 16 lowercase hex digits");
    }
    Ok(u64::from_str_radix(digits, 16)?)
}

/// How learned weights combine with the spec's in a learned state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Policy {
    /// The learned weights replace the spec's.
    #[default]
    Replace,
    /// The spec's probabilities, scaled by the learned weights: where the
    /// learned weights are equal, the spec's distribution is kept.
    Prior,
}

/// A weights file, as produced by training on traces (see [`StepModel`]).
#[derive(Deserialize)]
#[serde(bound(deserialize = "T: Deserialize<'de>"))]
struct WeightsFile<T> {
    #[serde(default = "epsilon_default")]
    epsilon: f64,
    #[serde(default)]
    policy: Policy,
    key: KeyLookup,
    /// State key → action key → weight.
    states: HashMap<String, HashMap<ActionKey, f64>>,
    /// State key → action key → the state the action most likely leads to.
    #[serde(default)]
    next: Option<HashMap<String, HashMap<ActionKey, String>>>,
    /// Action key → its template, for the actions to serve under a coarser
    /// template of the spec.
    #[serde(default)]
    templates: HashMap<ActionKey, T>,
}

fn epsilon_default() -> f64 {
    0.1
}

struct LearnedAction {
    key: ActionKey,
    weight: f64,
    /// Index of the learned state this action most likely leads to.
    state_next: Option<u32>,
    /// Index of the action's template in `Weights::templates`.
    template: Option<u32>,
}

/// Learned action weights per state (`--weights`).
///
/// In a learned state, an available action is picked by its learned weight,
/// or the mean learned weight of the state when it has none. An action the
/// spec gives no probability stays disabled. A learned action with a
/// template is served under an available template that contains it
/// ([`ActionTemplate::contains`]) when the spec does not offer it itself;
/// the containing template then only keeps its `epsilon` share, unless it
/// has a learned weight of its own.
pub struct Weights<T> {
    /// Share of probability kept by the fallback distribution (uniform for
    /// [`Policy::Replace`], the spec's for [`Policy::Prior`]), so that no
    /// available action is starved.
    epsilon: f64,
    policy: Policy,
    lookup: KeyLookup,
    /// State key → index in `states`.
    states_index: HashMap<u64, u32>,
    /// The learned actions of each state, in key order.
    states: Vec<Vec<LearnedAction>>,
    templates: Vec<T>,
    unservable_checked: AtomicBool,
}

impl<T> Weights<T> {
    const FORMAT: u32 = 4;

    pub fn load(path: &Path) -> Result<Arc<Self>>
    where
        T: DeserializeOwned,
    {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading weights {}", path.display()))?;
        let weights = Self::parse(&text)
            .with_context(|| format!("parsing weights {}", path.display()))?;
        Ok(Arc::new(weights))
    }

    fn parse(text: &str) -> Result<Self>
    where
        T: DeserializeOwned,
    {
        #[derive(Deserialize)]
        struct Header {
            format: u32,
        }
        // Read first: a file of another format fails to parse as this one.
        let Header { format } = serde_json::from_str(text)?;
        if format != Self::FORMAT {
            bail!(
                "unsupported weights format {format} (expected {})",
                Self::FORMAT
            );
        }
        let file: WeightsFile<T> = serde_json::from_str(text)?;
        if !(0.0..=1.0).contains(&file.epsilon) {
            bail!("epsilon must be within [0, 1], got {}", file.epsilon);
        }
        let lookup = KeyLookup::new(file.key.components, file.key.goals)?;

        let mut templates = Vec::with_capacity(file.templates.len());
        let mut templates_index = HashMap::with_capacity(file.templates.len());
        for (key, template) in file.templates {
            templates_index.insert(key, templates.len() as u32);
            templates.push(template);
        }

        let mut states_index = HashMap::with_capacity(file.states.len());
        let mut states = Vec::with_capacity(file.states.len());
        for (state, weights) in &file.states {
            let mut actions = Vec::with_capacity(weights.len());
            for (key, weight) in weights {
                if !(weight.is_finite() && *weight >= 0.0) {
                    bail!(
                        "state {state}: the weight of action {key} is \
                         {weight}, which is not a non-negative number"
                    );
                }
                actions.push(LearnedAction {
                    key: *key,
                    weight: *weight,
                    state_next: None,
                    template: templates_index.get(key).copied(),
                });
            }
            actions.sort_unstable_by_key(|action| action.key);
            states_index.insert(parse_state_key(state)?, states.len() as u32);
            states.push(actions);
        }

        for (state, targets) in file.next.iter().flatten() {
            let state = states_index.get(&parse_state_key(state)?).copied();
            for (key, target) in targets {
                let target = states_index.get(&parse_state_key(target)?);
                // A state or action without learned weights has nothing to
                // divide, and a target that is not learned is never counted.
                let Some(state) = state else { continue };
                let actions = &mut states[state as usize];
                if let Ok(index) =
                    actions.binary_search_by_key(key, |action| action.key)
                {
                    actions[index].state_next = target.copied();
                }
            }
        }

        Ok(Weights {
            epsilon: file.epsilon,
            policy: file.policy,
            lookup,
            states_index,
            states,
            templates,
            unservable_checked: AtomicBool::new(false),
        })
    }

    /// Warns about keyed components the spec never gives a hash, when the
    /// weights first meet the spec's snapshots.
    fn warn_unservable_once(&self, snapshots: &[Snapshot]) {
        if self.unservable_checked.swap(true, Ordering::Relaxed) {
            return;
        }
        let unservable: Vec<&str> = self.lookup.unservable(snapshots).collect();
        if unservable.is_empty() {
            return;
        }
        let named = |excluded: bool| -> Vec<&str> {
            snapshots
                .iter()
                .filter(|snapshot| snapshot.excluded == excluded)
                .filter_map(|snapshot| snapshot.name.as_deref())
                .collect()
        };
        log::warn!(
            "the weights key states on {unservable:?}, which this spec never \
             gives a hash, so they are left out of every state key (keyable \
             snapshots: {:?}, excluded from the key: {:?})",
            named(false),
            named(true),
        );
    }
}

#[derive(Default)]
struct Scratch {
    path: String,
    /// For each offered leaf, the index of its own learned action.
    own: Vec<Option<u32>>,
    /// For each learned action of the state, whether a leaf serves it.
    served: Vec<bool>,
    /// For each resulting leaf, its learned weight.
    weights: Vec<Option<f64>>,
}

/// Learned weights as one run applies them.
struct WeightsRun<T> {
    weights: Arc<Weights<T>>,
    /// Visits in this run of each learned state.
    visits: Vec<u32>,
    scratch: Scratch,
}

impl<T> WeightsRun<T> {
    fn new(weights: Arc<Weights<T>>) -> Self {
        WeightsRun {
            visits: vec![0; weights.states.len()],
            weights,
            scratch: Scratch::default(),
        }
    }

    /// The probabilities to pick by in `state`, when it is a learned state
    /// with something to pick; counts the visit. A learned weight is divided
    /// by sqrt(1 + visits in this run of the state its action most likely
    /// leads to), so that a run spreads over the states.
    #[hotpath::measure]
    fn reweight<'a, A>(
        &'a mut self,
        state: &(impl RunState + ?Sized),
        snapshots: &[Snapshot],
        leaves: &Leaves<'a, T>,
    ) -> Option<Leaves<'a, T>>
    where
        T: ActionTemplate<A>,
    {
        let WeightsRun {
            weights,
            visits,
            scratch,
        } = self;
        let weights: &'a Weights<T> = weights;
        weights.warn_unservable_once(snapshots);

        let key = state_key(&weights.lookup, |name| {
            component_hash(name, state, snapshots, &mut scratch.path)
        });
        let state_index =
            key.and_then(|key| weights.states_index.get(&key)).copied();
        log::debug!("state key {key:016x?}, learned: {state_index:?}");
        let state_index = state_index? as usize;
        visits[state_index] += 1;

        let offered = leaves.as_slice();
        if !offered.iter().any(|(_, probability)| *probability > 0.0) {
            return None;
        }
        let learned = &weights.states[state_index];
        let weight_now = |action: &LearnedAction| match action.state_next {
            Some(state_next) => {
                action.weight
                    / (1.0 + f64::from(visits[state_next as usize])).sqrt()
            }
            None => action.weight,
        };

        // An offered template serves its own learned action, so that no
        // other template serves it as a finer one.
        scratch.own.clear();
        scratch.served.clear();
        scratch.served.resize(learned.len(), false);
        for (template, _) in offered {
            let own = learned
                .binary_search_by_key(&action_key(*template), |action| {
                    action.key
                })
                .ok();
            if let Some(own) = own {
                scratch.served[own] = true;
            }
            scratch.own.push(own.map(|own| own as u32));
        }

        // Each leaf starts with its probability under the spec. An offered
        // leaf shares the spec's equally with the finer ones it serves.
        let mut reweighted: Vec<(&'a T, f64)> =
            Vec::with_capacity(offered.len());
        scratch.weights.clear();
        let mut known_total = 0.0;
        let mut known_count = 0usize;
        for (&(template, probability), own) in offered.iter().zip(&scratch.own)
        {
            let start = reweighted.len();
            if probability > 0.0 && !weights.templates.is_empty() {
                for (action, served) in learned.iter().zip(&mut scratch.served)
                {
                    let Some(fine) = action.template else {
                        continue;
                    };
                    let fine = &weights.templates[fine as usize];
                    if !*served && template.contains(fine) {
                        *served = true;
                        let weight = weight_now(action);
                        known_total += weight;
                        known_count += 1;
                        reweighted.push((fine, 0.0));
                        scratch.weights.push(Some(weight));
                    }
                }
            }
            let served_finer = reweighted.len() > start;
            reweighted.push((template, 0.0));
            scratch.weights.push(match own {
                Some(own) if probability > 0.0 => {
                    let weight = weight_now(&learned[*own as usize]);
                    known_total += weight;
                    known_count += 1;
                    Some(weight)
                }
                Some(_) => Some(0.0),
                None if served_finer => Some(0.0),
                None => None,
            });
            let share = probability / (reweighted.len() - start) as f64;
            for (_, probability) in &mut reweighted[start..] {
                *probability = share;
            }
        }

        let fallback = if known_count == 0 {
            1.0
        } else {
            known_total / known_count as f64
        };
        let prior = weights.policy == Policy::Prior;
        let mut total = 0.0;
        let mut enabled_count = 0usize;
        for ((_, probability), weight) in
            reweighted.iter().zip(&mut scratch.weights)
        {
            if *probability > 0.0 {
                enabled_count += 1;
                let weight = weight.get_or_insert(fallback);
                if prior {
                    *weight *= probability;
                }
                total += *weight;
            }
        }
        for ((_, probability), weight) in
            reweighted.iter_mut().zip(&scratch.weights)
        {
            let Some(weight) = weight.filter(|_| *probability > 0.0) else {
                continue;
            };
            let floor = if prior {
                *probability
            } else {
                1.0 / enabled_count as f64
            };
            let share = if total > 0.0 { weight / total } else { floor };
            *probability =
                (1.0 - weights.epsilon) * share + weights.epsilon * floor;
        }
        Leaves::from_weights(reweighted)
    }
}

/// Runs `inner`, but lets the strategy that picks the actions pick by the
/// learned [`Weights`], if any, in the states they cover.
pub struct WeightedStrategy<Inner, T> {
    inner: Inner,
    weights: Option<WeightsRun<T>>,
}

impl<Inner, T> WeightedStrategy<Inner, T> {
    pub fn new(inner: Inner, weights: Option<Arc<Weights<T>>>) -> Self {
        WeightedStrategy {
            inner,
            weights: weights.map(WeightsRun::new),
        }
    }
}

impl<Session: InterfaceSession, Inner: RunStrategy<Session>>
    RunStrategy<Session> for WeightedStrategy<Inner, Session::ActionTemplate>
{
    type StopValue = Inner::StopValue;

    fn on_new_state(
        &mut self,
        state: &Session::State,
        leaves: &Leaves<'_, Session::ActionTemplate>,
        last_action: Option<&Session::Action>,
        snapshots: &[Snapshot],
        properties: PropertiesState,
    ) -> Result<ControlFlow<Self::StopValue, Picked<Session::Action>>> {
        self.inner.on_new_state(
            state,
            leaves,
            last_action,
            snapshots,
            properties,
        )
    }

    fn deadline(&self) -> Option<Time> {
        self.inner.deadline()
    }

    fn on_time_limit(&mut self) -> Result<Self::StopValue> {
        self.inner.on_time_limit()
    }

    fn on_stalled(&mut self) -> Result<Self::StopValue> {
        self.inner.on_stalled()
    }

    fn on_interrupted(&mut self) -> Result<Self::StopValue> {
        self.inner.on_interrupted()
    }

    fn constrain(
        &self,
        state: &Session::State,
        tree: Tree<Session::ActionTemplate>,
    ) -> Tree<Session::ActionTemplate> {
        self.inner.constrain(state, tree)
    }

    fn reweight<'a>(
        &'a mut self,
        state: &Session::State,
        snapshots: &[Snapshot],
        leaves: &Leaves<'a, Session::ActionTemplate>,
    ) -> Option<Leaves<'a, Session::ActionTemplate>> {
        let WeightedStrategy { inner, weights } = self;
        weights
            .as_mut()
            .and_then(|weights| weights.reweight(state, snapshots, leaves))
            .or_else(|| inner.reweight(state, snapshots, leaves))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::fnv1a64;
    use crate::tree::Tree;
    use crate::tree::Weight;
    use hegel::TestCase;
    use hegel::generators::{booleans, integers, sampled_from, text, vecs};
    use serde_json::{Value, json};
    use std::collections::BTreeSet;
    use std::convert::Infallible;
    use std::hash::Hash;

    #[derive(Clone, Debug, PartialEq)]
    struct Key(&'static str);

    impl ActionTemplate<Key> for Key {
        fn generate<R: rand::TryRng + rand::RngExt>(&self, _: &mut R) -> Key {
            self.clone()
        }
        fn accepts(&self, original: &Key) -> bool {
            self == original
        }
        fn category_hash<H: Hasher>(&self, hasher: &mut H) {
            hasher.write(self.0.as_bytes());
        }
        fn contains(&self, other: &Self) -> bool {
            self == other
        }
    }

    /// Accepts any action of its group; produced only its own.
    #[derive(Clone, Debug, PartialEq, Serialize)]
    struct Loose(&'static str, u8);

    impl ActionTemplate<Loose> for Loose {
        fn generate<R: rand::TryRng + rand::RngExt>(&self, _: &mut R) -> Loose {
            self.clone()
        }
        fn accepts(&self, original: &Loose) -> bool {
            self.1 == original.1
        }
        fn produced(&self, original: &Loose) -> bool {
            self == original
        }
        fn category_hash<H: Hasher>(&self, hasher: &mut H) {
            hasher.write(self.0.as_bytes());
        }
        fn contains(&self, other: &Self) -> bool {
            self == other
        }
    }

    const NAMES: [&str; 9] = ["a", "b", "c", "d", "e", "f", "g", "h", "i"];

    /// A tree of distinct templates, named in depth-first order. Branches
    /// may be empty, and weights are positive so that every leaf of the
    /// pruned tree can be picked.
    fn trees(tc: &TestCase, depth: usize, next_leaf: &mut usize) -> Tree<Key> {
        if depth == 0 || tc.draw_silent(booleans()) {
            let value = Key(NAMES[*next_leaf]);
            *next_leaf += 1;
            return Tree::Leaf { value };
        }
        let size = tc.draw_silent(integers::<usize>().max_value(3));
        let branches = (0..size)
            .map(|_| {
                let weight = tc.draw_silent(integers::<Weight>().min_value(1));
                (weight, trees(tc, depth - 1, next_leaf))
            })
            .collect();
        Tree::Branch { branches }
    }

    fn generated_tree(tc: &TestCase) -> Tree<Key> {
        let tree = trees(tc, 2, &mut 0);
        tc.note(&format!("tree: {tree:?}"));
        tree
    }

    /// A tree of distinct templates in few groups, so that several accept
    /// the same action, and its templates in depth-first order.
    fn loose_tree(tc: &TestCase) -> (Tree<Loose>, Vec<Loose>) {
        let mut templates = vec![];
        let tree = trees(tc, 2, &mut 0)
            .try_map(&mut |Key(name)| {
                let group = tc.draw_silent(integers::<u8>().max_value(2));
                templates.push(Loose(name, group));
                Ok::<_, Infallible>(Loose(name, group))
            })
            .unwrap();
        tc.note(&format!("tree: {tree:?}"));
        (tree, templates)
    }

    #[hegel::test]
    fn actions_available_are_the_leaves_in_order(tc: TestCase) {
        let mut leaf_count = 0;
        let tree = trees(&tc, 2, &mut leaf_count);
        tc.note(&format!("tree: {tree:?}"));

        let model = StepModel::new(&tree.leaves(), None);
        let keys: Vec<ActionKey> = model
            .actions_available
            .iter()
            .map(|(key, _)| *key)
            .collect();
        let expected: Vec<ActionKey> = NAMES[..leaf_count]
            .iter()
            .map(|name| action_key(&Key(name)))
            .collect();
        assert_eq!(keys, expected);
    }

    #[hegel::test]
    fn actions_available_probabilities_sum_to_one(tc: TestCase) {
        let model = StepModel::new(&generated_tree(&tc).leaves(), None);
        tc.assume(!model.actions_available.is_empty());
        let total: f64 = model
            .actions_available
            .iter()
            .map(|(_, probability)| *probability)
            .sum();
        assert!((total - 1.0).abs() < 1e-9, "total is {total}");
    }

    #[hegel::test]
    fn picked_and_replayed_actions_have_the_key_of_their_template(
        tc: TestCase,
    ) {
        let tree = generated_tree(&tc);
        let leaves = tree.leaves();
        tc.assume(!leaves.as_slice().is_empty());
        let index =
            tc.draw(integers::<usize>().max_value(leaves.as_slice().len() - 1));
        let template = leaves.as_slice()[index].0;

        let picked = Picked::generate(template, &mut rand::rng());
        assert_eq!(picked.template, action_key(template));
        assert_eq!(
            accepting_key(&leaves, &picked.action),
            Some(picked.template)
        );
        assert_eq!(accepting_key(&leaves, &Key("other")), None);
    }

    #[hegel::test]
    fn accepting_key_is_the_producing_template_among_accepting(tc: TestCase) {
        let (tree, templates) = loose_tree(&tc);
        tc.assume(!templates.is_empty());
        let template = &templates
            [tc.draw(integers::<usize>().max_value(templates.len() - 1))];
        let action = template.generate(&mut rand::rng());

        assert_eq!(
            accepting_key(&tree.leaves(), &action),
            Some(action_key(template))
        );
    }

    #[hegel::test]
    fn accepting_key_is_the_first_accepting_template_without_a_producer(
        tc: TestCase,
    ) {
        let (tree, templates) = loose_tree(&tc);
        let action = Loose("other", tc.draw(integers::<u8>().max_value(2)));

        let expected = templates
            .iter()
            .find(|template| template.1 == action.1)
            .map(action_key);
        assert_eq!(accepting_key(&tree.leaves(), &action), expected);
    }

    #[derive(Debug, PartialEq, serde::Deserialize)]
    struct Row {
        key: ActionKey,
        template: (String, u8),
    }

    // `Loose` templates of one name share a key whatever their group, so the
    // table must hold the group seen first.
    #[hegel::test]
    fn template_table_holds_the_first_template_of_each_key_in_order(
        tc: TestCase,
    ) {
        let registry = TemplateRegistry::default();
        let mut expected: Vec<Row> = vec![];
        for _ in 0..tc.draw(integers::<usize>().max_value(4)) {
            let size = tc.draw(integers::<usize>().max_value(6));
            let branches = (0..size)
                .map(|_| {
                    let name = tc
                        .draw(hegel::generators::sampled_from(NAMES.to_vec()));
                    let group = tc.draw(integers::<u8>().max_value(2));
                    let value = Loose(name, group);
                    let key = action_key(&value);
                    if expected.iter().all(|row| row.key != key) {
                        expected.push(Row {
                            key,
                            template: (name.to_string(), group),
                        });
                    }
                    (1, Tree::Leaf { value })
                })
                .collect();
            let tree = Tree::Branch { branches };
            let leaves = tree.leaves();
            let model = StepModel::new(&leaves, None);
            registry.record(&leaves, &model).unwrap();
        }

        let mut table = vec![];
        registry.write(&mut table).unwrap();
        let rows: Vec<Row> = std::str::from_utf8(&table)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(rows, expected);
    }

    #[test]
    fn action_key_is_fnv1a64() {
        assert_eq!(action_key(&Key("a")), ActionKey(0xaf63_dc4c_8601_ec8c));
        assert_eq!(
            action_key(&Key("foobar")),
            ActionKey(0x8594_4171_f739_67e8)
        );
    }

    fn stable_hash(value: impl Hash) -> u64 {
        let mut hasher = StableHasher::default();
        value.hash(&mut hasher);
        hasher.finish()
    }

    #[hegel::test]
    fn stable_hasher_writes_integers_little_endian(tc: TestCase) {
        let value = tc.draw(integers::<u64>());
        let bytes = value.to_le_bytes();
        assert_eq!(stable_hash(value), fnv1a64(bytes));
        assert_eq!(stable_hash(value as u32), fnv1a64(bytes[..4].to_vec()));
        assert_eq!(stable_hash(value as u16), fnv1a64(bytes[..2].to_vec()));
        assert_eq!(
            stable_hash(value as usize),
            stable_hash(value as usize as u64)
        );
        assert_eq!(
            stable_hash(value as isize),
            stable_hash(value as isize as i64)
        );
    }

    /// Contains the templates it is a prefix of.
    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    struct Prefix(String);

    impl ActionTemplate<Prefix> for Prefix {
        fn generate<R: rand::TryRng + rand::RngExt>(
            &self,
            _: &mut R,
        ) -> Prefix {
            self.clone()
        }
        fn accepts(&self, original: &Prefix) -> bool {
            original.0.starts_with(&self.0)
        }
        fn category_hash<H: Hasher>(&self, hasher: &mut H) {
            hasher.write(self.0.as_bytes());
        }
        fn contains(&self, other: &Self) -> bool {
            other.0.starts_with(&self.0)
        }
    }

    struct At(BTreeMap<String, u64>);

    impl At {
        fn place(place: u64) -> Self {
            At(BTreeMap::from([("place".to_string(), place)]))
        }
    }

    impl RunState for At {
        fn timestamp(&self) -> Time {
            Time::from_system_time(std::time::SystemTime::UNIX_EPOCH)
        }
        fn hashes(&self) -> &BTreeMap<String, u64> {
            &self.0
        }
    }

    fn snapshot(name: Option<&str>, value: Value, excluded: bool) -> Snapshot {
        Snapshot {
            index: 0,
            name: name.map(str::to_string),
            value,
            time: Time::from_system_time(std::time::SystemTime::UNIX_EPOCH),
            excluded,
        }
    }

    fn lookup(components: &[&str], goals: &[(&str, Option<u64>)]) -> KeyLookup {
        KeyLookup::new(
            components.iter().map(|name| name.to_string()).collect(),
            goals
                .iter()
                .map(|(name, idle)| (name.to_string(), *idle))
                .collect(),
        )
        .unwrap()
    }

    /// The key, as a weights file writes it, of the states whose driver
    /// hash `place` has this value, under a lookup of that one component.
    fn place_key(place: u64) -> String {
        let key = state_key(&lookup(&["place"], &[]), |_| Some(place)).unwrap();
        format!("{key:016x}")
    }

    fn actions_json<V: Serialize>(actions: &[(Prefix, V)]) -> Value {
        actions
            .iter()
            .map(|(template, value)| {
                (action_key(template).to_string(), json!(value))
            })
            .collect::<serde_json::Map<_, _>>()
            .into()
    }

    /// Weights keyed on `place`, with one learned state. The actions in
    /// `templates` come with their templates.
    fn weights_at(
        place: u64,
        epsilon_percent: u8,
        policy: Policy,
        learned: &[(Prefix, f64)],
        templates: &[Prefix],
    ) -> WeightsRun<Prefix> {
        let templates: Vec<(Prefix, &Prefix)> = templates
            .iter()
            .map(|template| (template.clone(), template))
            .collect();
        let document = json!({
            "format": 4,
            "epsilon": f64::from(epsilon_percent) / 100.0,
            "policy": match policy {
                Policy::Replace => "replace",
                Policy::Prior => "prior",
            },
            "key": {"components": ["place"]},
            "states": {place_key(place): actions_json(learned)},
            "templates": actions_json(&templates),
        });
        WeightsRun::new(Arc::new(
            Weights::parse(&document.to_string()).unwrap(),
        ))
    }

    fn policies(tc: &TestCase) -> Policy {
        if tc.draw(booleans()) {
            Policy::Replace
        } else {
            Policy::Prior
        }
    }

    fn zero_some_weights<T>(tc: &TestCase, tree: &mut Tree<T>) {
        if let Tree::Branch { branches } = tree {
            for (weight, subtree) in branches {
                if tc.draw_silent(integers::<u8>().max_value(3)) == 0 {
                    *weight = 0;
                }
                zero_some_weights(tc, subtree);
            }
        }
    }

    /// A tree of templates none of which contains another, some of them
    /// below a zero weight, and a learned weight for some of them.
    fn spec_and_learned(tc: &TestCase) -> (Tree<Prefix>, Vec<(Prefix, f64)>) {
        let mut learned = vec![];
        let mut tree = trees(tc, 2, &mut 0)
            .try_map(&mut |Key(name)| {
                let template = Prefix(name.to_string());
                if tc.draw_silent(booleans()) {
                    let weight = tc.draw_silent(integers::<u16>());
                    learned.push((template.clone(), f64::from(weight)));
                }
                Ok::<_, Infallible>(template)
            })
            .unwrap();
        zero_some_weights(tc, &mut tree);
        tc.note(&format!("tree: {tree:?}, learned: {learned:?}"));
        (tree, learned)
    }

    fn enabled_count<T>(leaves: &Leaves<'_, T>) -> usize {
        leaves
            .as_slice()
            .iter()
            .filter(|(_, probability)| *probability > 0.0)
            .count()
    }

    #[hegel::test]
    fn reweight_applies_in_the_learned_state_only(tc: TestCase) {
        let (tree, learned) = spec_and_learned(&tc);
        let stored = tc.draw(integers::<u64>());
        let live = if tc.draw(booleans()) {
            stored
        } else {
            tc.draw(integers::<u64>())
        };
        let mut weights = weights_at(
            stored,
            tc.draw(integers().max_value(100)),
            policies(&tc),
            &learned,
            &[],
        );

        let leaves = tree.leaves();
        let pickable = enabled_count(&leaves) > 0;
        let reweighted = weights.reweight(&At::place(live), &[], &leaves);
        assert_eq!(reweighted.is_some(), stored == live && pickable);
    }

    #[hegel::test]
    fn reweighted_leaves_are_the_spec_leaves_with_the_disabled_at_zero(
        tc: TestCase,
    ) {
        let (tree, learned) = spec_and_learned(&tc);
        let mut weights = weights_at(
            42,
            tc.draw(integers().max_value(100)),
            policies(&tc),
            &learned,
            &[],
        );

        let leaves = tree.leaves();
        tc.assume(enabled_count(&leaves) > 0);
        let reweighted =
            weights.reweight(&At::place(42), &[], &leaves).unwrap();
        assert_eq!(reweighted.as_slice().len(), leaves.as_slice().len());
        for ((template, probability), (offered, spec)) in
            reweighted.as_slice().iter().zip(leaves.as_slice())
        {
            assert_eq!(template, offered);
            if *spec == 0.0 {
                assert_eq!(*probability, 0.0, "{template:?} was revived");
            }
        }
        let total: f64 = reweighted.as_slice().iter().map(|(_, p)| *p).sum();
        assert!((total - 1.0).abs() < 1e-9, "total is {total}");
    }

    #[hegel::test]
    fn reweighted_leaves_keep_their_share_of_epsilon(tc: TestCase) {
        let (tree, learned) = spec_and_learned(&tc);
        let policy = policies(&tc);
        let mut weights = weights_at(
            42,
            tc.draw(integers().max_value(100)),
            policy,
            &learned,
            &[],
        );
        let epsilon = weights.weights.epsilon;

        let leaves = tree.leaves();
        let enabled_count = enabled_count(&leaves);
        tc.assume(enabled_count > 0);
        let reweighted =
            weights.reweight(&At::place(42), &[], &leaves).unwrap();
        for ((template, probability), (_, spec)) in
            reweighted.as_slice().iter().zip(leaves.as_slice())
        {
            let fallback = match policy {
                Policy::Replace if *spec > 0.0 => 1.0 / enabled_count as f64,
                Policy::Replace => 0.0,
                Policy::Prior => *spec,
            };
            assert!(
                *probability >= epsilon * fallback - 1e-12,
                "{template:?} has probability {probability}, below its \
                 share {fallback} of epsilon {epsilon}"
            );
        }
    }

    #[hegel::test]
    fn prior_with_equal_learned_weights_keeps_the_spec_distribution(
        tc: TestCase,
    ) {
        let (tree, _) = spec_and_learned(&tc);
        let weight = f64::from(tc.draw(integers::<u16>().min_value(1)));
        let learned: Vec<(Prefix, f64)> = tree
            .values()
            .into_iter()
            .map(|template| (template.clone(), weight))
            .collect();
        let mut weights = weights_at(
            42,
            tc.draw(integers().max_value(100)),
            Policy::Prior,
            &learned,
            &[],
        );

        let leaves = tree.leaves();
        tc.assume(enabled_count(&leaves) > 0);
        let reweighted =
            weights.reweight(&At::place(42), &[], &leaves).unwrap();
        for ((template, probability), (_, spec)) in
            reweighted.as_slice().iter().zip(leaves.as_slice())
        {
            assert!(
                (probability - spec).abs() < 1e-9,
                "{template:?} has probability {probability}, the spec's \
                 is {spec}"
            );
        }
    }

    // An action without a learned weight counts with the mean of the
    // learned weights of the enabled actions.
    #[hegel::test]
    fn replace_picks_in_proportion_to_the_learned_weights(tc: TestCase) {
        let (tree, learned) = spec_and_learned(&tc);
        let mut weights = weights_at(42, 0, Policy::Replace, &learned, &[]);

        let leaves = tree.leaves();
        tc.assume(enabled_count(&leaves) > 0);
        let learned_weights: Vec<Option<f64>> = leaves
            .as_slice()
            .iter()
            .map(|(template, spec)| {
                let learned =
                    learned.iter().find(|(learned, _)| learned == *template);
                learned.map(|(_, weight)| *weight).filter(|_| *spec > 0.0)
            })
            .collect();
        let known: Vec<f64> =
            learned_weights.iter().copied().flatten().collect();
        let mean = if known.is_empty() {
            1.0
        } else {
            known.iter().sum::<f64>() / known.len() as f64
        };
        let expected: Vec<f64> = leaves
            .as_slice()
            .iter()
            .zip(&learned_weights)
            .map(|((_, spec), learned)| match learned {
                _ if *spec == 0.0 => 0.0,
                Some(weight) => *weight,
                None => mean,
            })
            .collect();
        let expected_total: f64 = expected.iter().sum();
        // Learned weights that are all zero say nothing.
        tc.assume(expected_total > 0.0);

        let reweighted =
            weights.reweight(&At::place(42), &[], &leaves).unwrap();
        for ((template, probability), expected) in
            reweighted.as_slice().iter().zip(expected)
        {
            let expected = expected / expected_total;
            assert!(
                (probability - expected).abs() < 1e-9,
                "{template:?} has probability {probability}, expected \
                 {expected}"
            );
        }
    }

    #[test]
    fn learned_weights_of_zero_fall_back_to_uniform() {
        let [a, b] = ["a", "b"].map(|name| Prefix(name.to_string()));
        let tree = Tree::Branch {
            branches: vec![
                (3, Tree::Leaf { value: a.clone() }),
                (1, Tree::Leaf { value: b.clone() }),
            ],
        };
        let learned = [(a.clone(), 0.0), (b.clone(), 0.0)];
        let mut weights = weights_at(42, 0, Policy::Replace, &learned, &[]);
        let leaves = tree.leaves();
        let reweighted =
            weights.reweight(&At::place(42), &[], &leaves).unwrap();
        assert_eq!(reweighted.as_slice(), [(&a, 0.5), (&b, 0.5)]);
    }

    // Two actions `a` and `b`, learned in the states at places 0 and 1.
    // Place 2 is not learned.
    #[hegel::test]
    fn a_learned_weight_is_divided_by_the_root_of_the_visits_of_its_next_state(
        tc: TestCase,
    ) {
        let [a, b] = ["a", "b"].map(|name| Prefix(name.to_string()));
        let tree = Tree::Branch {
            branches: vec![
                (1, Tree::Leaf { value: a.clone() }),
                (1, Tree::Leaf { value: b.clone() }),
            ],
        };
        let weight = || f64::from(tc.draw(integers::<u16>().min_value(1)));
        let next = || tc.draw(integers::<usize>().max_value(3));
        // Per place: each action's weight and next place, 3 for none.
        let learned: [[(f64, usize); 2]; 2] = std::array::from_fn(|_| {
            std::array::from_fn(|_| (weight(), next()))
        });
        let mut states = serde_json::Map::new();
        let mut states_next = serde_json::Map::new();
        for (place, [action_a, action_b]) in learned.iter().enumerate() {
            let actions = [(a.clone(), *action_a), (b.clone(), *action_b)];
            let weights: Vec<(Prefix, f64)> = actions
                .iter()
                .map(|(template, (weight, _))| (template.clone(), *weight))
                .collect();
            let targets: Vec<(Prefix, String)> = actions
                .iter()
                .filter(|(_, (_, next))| *next < 3)
                .map(|(template, (_, next))| {
                    (template.clone(), place_key(*next as u64))
                })
                .collect();
            states.insert(place_key(place as u64), actions_json(&weights));
            states_next.insert(place_key(place as u64), actions_json(&targets));
        }
        let document = json!({
            "format": 4,
            "epsilon": 0.0,
            "key": {"components": ["place"]},
            "states": states,
            "next": states_next,
        });
        let mut weights = WeightsRun::new(Arc::new(
            Weights::<Prefix>::parse(&document.to_string()).unwrap(),
        ));

        let leaves = tree.leaves();
        let mut visits = [0u32; 3];
        let walk: Vec<usize> =
            tc.draw(vecs(integers::<usize>().max_value(2)).max_size(20));
        for place in walk {
            visits[place] += 1;
            let reweighted =
                weights.reweight(&At::place(place as u64), &[], &leaves);
            let Some(actions) = learned.get(place) else {
                assert_eq!(reweighted, None);
                continue;
            };
            let [expected_a, expected_b] = actions.map(|(weight, next)| {
                let seen = if next < 2 { visits[next] } else { 0 };
                weight / (1.0 + f64::from(seen)).sqrt()
            });
            let reweighted = reweighted.unwrap();
            let [(_, probability_a), (_, probability_b)] =
                reweighted.as_slice()
            else {
                panic!("two leaves, got {reweighted:?}");
            };
            let expected = expected_a / (expected_a + expected_b);
            assert!(
                (probability_a - expected).abs() < 1e-9,
                "at place {place} after {visits:?} visits, a has \
                 probability {probability_a}, expected {expected}"
            );
            assert!((probability_a + probability_b - 1.0).abs() < 1e-9);
        }
    }

    fn prefixes(tc: &TestCase, size_max: usize) -> Vec<Prefix> {
        let names: BTreeSet<String> = tc
            .draw(vecs(text().alphabet("xy").max_size(size_max)).max_size(6))
            .into_iter()
            .collect();
        names.into_iter().map(Prefix).collect()
    }

    #[hegel::test]
    fn a_learned_template_is_served_once_under_a_template_that_contains_it(
        tc: TestCase,
    ) {
        let offered = prefixes(&tc, 2);
        let branches = offered
            .iter()
            .map(|template| {
                let weight = tc.draw(integers::<Weight>().max_value(3));
                (
                    weight,
                    Tree::Leaf {
                        value: template.clone(),
                    },
                )
            })
            .collect();
        let tree = Tree::Branch { branches };
        let learned: Vec<(Prefix, f64)> = prefixes(&tc, 3)
            .into_iter()
            .map(|template| (template, f64::from(tc.draw(integers::<u16>()))))
            .collect();
        let with_template: Vec<Prefix> = learned
            .iter()
            .filter(|_| tc.draw(booleans()))
            .map(|(template, _)| template.clone())
            .collect();
        let mut weights = weights_at(
            42,
            tc.draw(integers().max_value(100)),
            policies(&tc),
            &learned,
            &with_template,
        );

        let leaves = tree.leaves();
        tc.assume(enabled_count(&leaves) > 0);
        let reweighted =
            weights.reweight(&At::place(42), &[], &leaves).unwrap();

        let mut served: Vec<&Prefix> = vec![];
        let mut offered_leaves = leaves.as_slice().iter();
        for (index, (template, _)) in reweighted.as_slice().iter().enumerate() {
            if offered.contains(template) {
                // The offered leaves stay, in order, each after the finer
                // ones it serves.
                let (expected, spec) = offered_leaves.next().unwrap();
                assert_eq!(template, expected);
                for fine in served.drain(..) {
                    assert!(
                        *spec > 0.0 && template.contains(fine),
                        "{fine:?} is served under {template:?}"
                    );
                }
            } else {
                assert!(
                    with_template.contains(template),
                    "{template:?} is neither offered nor learned with a \
                     template"
                );
                let earlier = &reweighted.as_slice()[..index];
                assert!(
                    earlier.iter().all(|(other, _)| other != template),
                    "{template:?} is served twice"
                );
                served.push(template);
            }
        }
        assert_eq!(offered_leaves.next(), None);
        assert!(served.is_empty());

        for fine in &with_template {
            let servable = leaves
                .as_slice()
                .iter()
                .any(|(template, spec)| *spec > 0.0 && template.contains(fine));
            let listed = reweighted
                .as_slice()
                .iter()
                .any(|(template, _)| *template == fine);
            assert_eq!(listed, servable || offered.contains(fine), "{fine:?}");
        }
    }

    // The coarse template has no learned weight of its own and keeps only
    // its share of epsilon.
    #[test]
    fn a_coarse_template_serves_the_finer_learned_one() {
        let [coarse, fine] = ["x", "xy"].map(|name| Prefix(name.to_string()));
        let tree = Tree::Leaf {
            value: coarse.clone(),
        };
        let learned = [(fine.clone(), 5.0)];
        let mut weights = weights_at(
            42,
            10,
            Policy::Replace,
            &learned,
            std::slice::from_ref(&fine),
        );
        let leaves = tree.leaves();
        let reweighted =
            weights.reweight(&At::place(42), &[], &leaves).unwrap();
        let rounded: Vec<(&Prefix, f64)> = reweighted
            .as_slice()
            .iter()
            .map(|(template, p)| (*template, (p * 1e6).round() / 1e6))
            .collect();
        assert_eq!(rounded, [(&fine, 0.95), (&coarse, 0.05)]);
    }

    #[hegel::test]
    fn unservable_components_are_the_snapshots_the_spec_does_not_key(
        tc: TestCase,
    ) {
        const NAMES: [&str; 6] = [
            "snapshot.a",
            "snapshot.b",
            "snapshot.c",
            "snapshot.a.inner",
            "coverage.f",
            "place",
        ];
        let mut keyable = BTreeSet::new();
        let mut snapshots = vec![];
        for name in ["a", "b", "c"] {
            if tc.draw(booleans()) {
                let excluded = tc.draw(booleans());
                if !excluded {
                    keyable.insert(format!("snapshot.{name}"));
                }
                snapshots.push(snapshot(Some(name), json!(1), excluded));
            }
        }
        if tc.draw(booleans()) {
            snapshots.push(snapshot(None, json!(1), false));
        }
        let components: BTreeSet<&str> = tc
            .draw(vecs(sampled_from(NAMES.to_vec())))
            .into_iter()
            .collect();
        let goals: Vec<(&str, Option<u64>)> = tc
            .draw(vecs(sampled_from(NAMES.to_vec())))
            .into_iter()
            .map(|name| (name, None))
            .collect();
        tc.assume(!components.is_empty() || !goals.is_empty());
        let components: Vec<&str> = components.into_iter().collect();
        let lookup = lookup(&components, &goals);

        let unservable: BTreeSet<&str> =
            lookup.unservable(&snapshots).collect();
        let expected: BTreeSet<&str> = components
            .iter()
            .chain(goals.iter().map(|(name, _)| name))
            .copied()
            .filter(|name| {
                name.starts_with("snapshot.") && !keyable.contains(*name)
            })
            .collect();
        assert_eq!(unservable, expected);
    }

    /// Keys are matched against the ones training writes, so a change here
    /// invalidates weights files.
    #[test]
    fn state_keys_are_pinned() {
        let lookup = lookup(
            &["snapshot.route", "snapshot.cart"],
            &[
                ("coverage./app.js#badge", None),
                ("snapshot.combo", Some(42)),
            ],
        );
        let structural = BTreeMap::from([
            ("snapshot.route", 0x0123_4567_89ab_cdef),
            ("snapshot.cart", u64::MAX),
            ("snapshot.combo", 42),
        ]);
        assert_eq!(
            state_key(&lookup, |name| structural.get(name).copied()),
            Some(0xe8a1_160d_ce81_b58f)
        );
        let goals = BTreeMap::from([
            ("snapshot.route", 1),
            ("coverage./app.js#badge", 7),
            ("snapshot.combo", 43),
        ]);
        assert_eq!(
            state_key(&lookup, |name| goals.get(name).copied()),
            Some(0xa464_585f_c79d_2d7e)
        );
    }

    #[test]
    fn snapshot_hashes_are_pinned() {
        let value = json!({
            "b": [1, {"c": null}, "q\"uo\nte"],
            "a": "x",
            "e": {},
            "f": 1.5,
            "g": true,
            "h": -3,
        });
        let mut path = String::new();
        assert_eq!(
            snapshot_hash(&value, &mut path),
            Some(0x74d5_2ac6_0bea_8350)
        );
        assert_eq!(
            snapshot_hash(&json!("root"), &mut path),
            Some(0xe041_d8d4_d0a4_f2e0)
        );
    }

    fn json_values(tc: &TestCase, depth: usize) -> Value {
        let kinds = if depth == 0 { 3 } else { 5 };
        match tc.draw_silent(integers::<u8>().max_value(kinds)) {
            0 => Value::Null,
            1 => json!(tc.draw_silent(booleans())),
            2 => json!(tc.draw_silent(integers::<i64>())),
            3 => json!(tc.draw_silent(text())),
            4 => (0..tc.draw_silent(integers::<usize>().max_value(3)))
                .map(|_| json_values(tc, depth - 1))
                .collect(),
            _ => (0..tc.draw_silent(integers::<usize>().max_value(3)))
                .map(|_| (tc.draw_silent(text()), json_values(tc, depth - 1)))
                .collect::<serde_json::Map<_, _>>()
                .into(),
        }
    }

    /// `path=value` for every scalar in `value`, object keys sorted.
    fn json_leaves(value: &Value, path: &str, leaves: &mut Vec<String>) {
        let child = |key: &str| {
            if path.is_empty() {
                key.to_string()
            } else {
                format!("{path}.{key}")
            }
        };
        match value {
            Value::Object(map) => {
                let entries: BTreeMap<&String, &Value> = map.iter().collect();
                for (key, item) in entries {
                    json_leaves(item, &child(key), leaves);
                }
            }
            Value::Array(items) => {
                for (index, item) in items.iter().enumerate() {
                    json_leaves(item, &child(&index.to_string()), leaves);
                }
            }
            scalar => leaves.push(format!("{path}={scalar}")),
        }
    }

    #[hegel::test]
    fn snapshot_hash_is_the_feature_hash_of_the_leaves(tc: TestCase) {
        let value = json_values(&tc, 3);
        tc.note(&format!("value: {value}"));
        let mut leaves = vec![];
        json_leaves(&value, "", &mut leaves);

        // The path is scratch space: what it holds does not matter.
        let mut path = tc.draw(text());
        assert_eq!(
            snapshot_hash(&value, &mut path),
            feature_hash(leaves.iter().map(String::as_str))
        );
    }

    const COMPONENTS: [&str; 5] = ["a", "b", "c", "d", "e"];

    /// Hashes under some of `COMPONENTS`, from few values so that goals are
    /// often at their idle value.
    fn component_hashes(tc: &TestCase) -> BTreeMap<&'static str, u64> {
        COMPONENTS
            .into_iter()
            .filter(|_| tc.draw(booleans()))
            .map(|name| (name, tc.draw(integers::<u64>().max_value(2))))
            .collect()
    }

    fn component_subsets(tc: &TestCase) -> Vec<&'static str> {
        COMPONENTS
            .into_iter()
            .filter(|_| tc.draw(booleans()))
            .collect()
    }

    #[hegel::test]
    fn state_key_is_of_the_active_goals_or_else_the_structural_components(
        tc: TestCase,
    ) {
        let components = component_subsets(&tc);
        let goals: Vec<(&str, Option<u64>)> = component_subsets(&tc)
            .into_iter()
            .map(|name| {
                let idle = tc
                    .draw(booleans())
                    .then(|| tc.draw(integers::<u64>().max_value(2)));
                (name, idle)
            })
            .collect();
        tc.assume(!components.is_empty() || !goals.is_empty());
        let hashes = component_hashes(&tc);

        let active: Vec<&str> = goals
            .iter()
            .filter(|(name, idle)| {
                hashes.get(name).is_some_and(|hash| Some(*hash) != *idle)
            })
            .map(|(name, _)| *name)
            .collect();
        let keyed = if active.is_empty() {
            &components
        } else {
            &active
        };
        let expected = if keyed.is_empty() {
            None
        } else {
            state_key(&lookup(keyed, &[]), |name| hashes.get(name).copied())
        };
        assert_eq!(
            state_key(&lookup(&components, &goals), |name| {
                hashes.get(name).copied()
            }),
            expected
        );
    }

    #[hegel::test]
    fn structural_state_key_is_of_the_present_components_and_their_hashes(
        tc: TestCase,
    ) {
        let components = component_subsets(&tc);
        tc.assume(!components.is_empty());
        let lookup = lookup(&components, &[]);
        let first = component_hashes(&tc);
        let second = component_hashes(&tc);

        let keyed = |hashes: &BTreeMap<&str, u64>| -> Vec<(&str, u64)> {
            components
                .iter()
                .filter_map(|name| Some((*name, *hashes.get(name)?)))
                .collect()
        };
        let key = |hashes: &BTreeMap<&'static str, u64>| {
            state_key(&lookup, |name| hashes.get(name).copied())
        };
        assert_eq!(key(&first).is_some(), !keyed(&first).is_empty());
        assert_eq!(
            key(&first) == key(&second),
            keyed(&first) == keyed(&second)
        );
    }

    #[hegel::test]
    fn component_hashes_skip_excluded_and_unnamed_snapshots(tc: TestCase) {
        let snapshots: Vec<Snapshot> = (0..tc
            .draw(integers::<usize>().max_value(5)))
            .map(|_| {
                let name = tc
                    .draw(booleans())
                    .then(|| tc.draw(sampled_from(vec!["a", "b"])));
                snapshot(name, json_values(&tc, 1), tc.draw(booleans()))
            })
            .collect();
        tc.note(&format!("snapshots: {snapshots:?}"));
        let name = tc.draw(sampled_from(vec!["a", "b"]));

        let keyable: Vec<Snapshot> = snapshots
            .iter()
            .filter(|snapshot| !snapshot.excluded && snapshot.name.is_some())
            .cloned()
            .collect();
        let hash = |snapshots: &[Snapshot]| {
            component_hash(
                &format!("snapshot.{name}"),
                &At::place(0),
                snapshots,
                &mut String::new(),
            )
        };
        assert_eq!(hash(&snapshots), hash(&keyable));
    }

    #[test]
    fn component_hash_of_another_name_is_the_drivers() {
        let snapshots = [snapshot(Some("place"), json!(1), false)];
        let mut path = String::new();
        assert_eq!(
            component_hash("place", &At::place(7), &snapshots, &mut path),
            Some(7)
        );
        assert_eq!(
            component_hash("other", &At::place(7), &snapshots, &mut path),
            None
        );
    }

    #[hegel::test]
    fn weights_file_goals_hold_any_idle_hash(tc: TestCase) {
        let goals: BTreeMap<String, Option<u64>> = (0..tc
            .draw(integers::<usize>().min_value(1).max_value(4)))
            .map(|_| {
                let idle =
                    tc.draw(booleans()).then(|| tc.draw(integers::<u64>()));
                (tc.draw(text()), idle)
            })
            .collect();
        let document = json!({
            "format": 4,
            "key": {"goals": goals},
            "states": {},
        });
        let weights = Weights::<Prefix>::parse(&document.to_string()).unwrap();
        assert_eq!(weights.lookup.goals, goals);
    }

    #[test]
    fn weights_file_defaults() {
        let weights = Weights::<Prefix>::parse(
            r#"{"format": 4, "key": {"components": ["b", "a"]}, "states": {}}"#,
        )
        .unwrap();
        assert_eq!(weights.epsilon, 0.1);
        assert_eq!(weights.policy, Policy::Replace);
        assert_eq!(weights.lookup.components, ["a", "b"]);
    }

    #[test]
    fn weights_file_rejections() {
        let key = r#""key": {"components": ["snapshot.route"]}"#;
        let action = "00000000000000aa";
        let state = "00000000000000bb";
        let documents = [
            // A format 3 file has other fields.
            r#"{"format": 3, "simhash": {"weights": {}}, "states": {}}"#
                .to_string(),
            r#"{"states": {}}"#.to_string(),
            format!(r#"{{"format": 4, {key}}}"#),
            format!(r#"{{"format": 4, "epsilon": 1.5, {key}, "states": {{}}}}"#),
            format!(r#"{{"format": 4, "policy": "other", {key}, "states": {{}}}}"#),
            r#"{"format": 4, "key": {}, "states": {}}"#.to_string(),
            r#"{"format": 4, "key": {"weights": {"a": 1}}, "states": {}}"#
                .to_string(),
            r#"{"format": 4, "key": {"components": ["a", "a"]}, "states": {}}"#
                .to_string(),
            r#"{"format": 4, "key": {"goals": {"a": -1}}, "states": {}}"#
                .to_string(),
            r#"{"format": 4, "key": {"goals": {"a": "00000000000000aa"}}, "states": {}}"#
                .to_string(),
            format!(r#"{{"format": 4, {key}, "states": {{"2a": {{}}}}}}"#),
            format!(
                r#"{{"format": 4, {key}, "states": {{"00000000000000BB": {{}}}}}}"#
            ),
            format!(
                r#"{{"format": 4, {key}, "states": {{"{state}": {{"aa": 1}}}}}}"#
            ),
            format!(
                r#"{{"format": 4, {key}, "states": {{"{state}": {{"{action}": -1}}}}}}"#
            ),
            format!(
                r#"{{"format": 4, {key}, "states": {{}}, "next": {{"{state}": {{"{action}": "2a"}}}}}}"#
            ),
            format!(
                r#"{{"format": 4, {key}, "states": {{}}, "templates": {{"{action}": 1}}}}"#
            ),
        ];
        let errors: Vec<String> = documents
            .iter()
            .map(|document| {
                let error = Weights::<Prefix>::parse(document)
                    .err()
                    .expect("the document is rejected");
                format!("{document}\n  {error:#}")
            })
            .collect();
        insta::assert_snapshot!(errors.join("\n"));
    }
}
