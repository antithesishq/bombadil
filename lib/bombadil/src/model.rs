//! State hashes and action keys, used to learn action weights from traces and
//! to apply them (`--weights`). Traces record each step's action vocabulary
//! (the available templates and the one taken); the hashes are derived from
//! the recorded state, here and in training, by the same code.
//!
//! Each driver describes a state by named 64-bit hashes (e.g. `location`,
//! `snapshot.<name>`), equal exactly when the hashed value is equal. Training
//! decides how to combine them into one state key, and the weights file tells
//! Bombadil to combine them the same way. Actions are identified by their
//! template's `category_hash`. Every hash here has a fixed algorithm (FNV-1a,
//! SplitMix64's finalizer), so recorded hashes and keys are the same across
//! Rust versions, unlike `DefaultHasher`'s.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::hash::Hasher;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use bombadil_schema::Time;
use serde::{Deserialize, Serialize};

use crate::driver::{ActionTemplate, InterfaceSession, RunState};
use crate::runner::{ControlFlow, PropertiesState, RunStrategy};
use crate::specification::domain::Snapshot;
use crate::tree::Tree;

/// What a trace records about each step for training.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct StepModel {
    /// Key ([`action_key`]) of each available action template, with the
    /// probability the spec weights give it.
    pub actions_available: Vec<(String, f64)>,
    /// Key of the template, among the previous step's available ones, that
    /// generated the action leading to this state.
    pub action_taken: Option<String>,
}

impl StepModel {
    /// `previous` is the last step's available actions and the action taken
    /// from them.
    pub fn new<A, T: ActionTemplate<A> + Clone>(
        tree: &Tree<T>,
        previous: Option<(&Tree<T>, &A)>,
    ) -> Self {
        // Picking prunes empty branches first, so probabilities must too.
        let actions_available = tree
            .clone()
            .prune()
            .iter()
            .flat_map(|tree| tree.leaves_with_probabilities())
            .map(|(template, probability)| (action_key(template), probability))
            .collect();
        let action_taken = previous.and_then(|(tree, action)| {
            tree.values()
                .into_iter()
                .find(|template| template.accepts(action))
                .map(|template| action_key(template))
        });
        StepModel {
            actions_available,
            action_taken,
        }
    }
}

/// The template's `category_hash` under [`StableHasher`], as 16 hex digits.
pub fn action_key<A>(template: &impl ActionTemplate<A>) -> String {
    let mut hasher = StableHasher::default();
    template.category_hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// FNV-1a. Action keys are recorded in traces and weights files, so they must
/// not depend on the Rust version as `std::hash::DefaultHasher` does.
pub struct StableHasher(u64);

impl Default for StableHasher {
    fn default() -> Self {
        StableHasher(FNV_OFFSET)
    }
}

impl Hasher for StableHasher {
    fn write(&mut self, bytes: &[u8]) {
        self.0 = fnv1a64_from(self.0, bytes);
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

/// The driver's hashes of the state, plus, for each named snapshot,
/// `snapshot.<name>`: the hash of its JSON leaves ([`feature_hash`] of the
/// `path=value` lines, see `json_leaves`).
pub fn state_hashes(
    state: &(impl RunState + ?Sized),
    snapshots: &[Snapshot],
) -> BTreeMap<String, u64> {
    let mut hashes = state.hashes();
    for snapshot in snapshots {
        let Some(name) = &snapshot.name else { continue };
        let mut leaves = Vec::new();
        json_leaves(&snapshot.value, "", &mut leaves);
        if let Some(hash) = feature_hash(leaves.iter().map(String::as_str)) {
            hashes.insert(format!("snapshot.{name}"), hash);
        }
    }
    hashes
}

/// Combines named hashes into one simhash: each name gets a share of the 64
/// bits proportional to its weight (largest remainder, names in sorted
/// order) and contributes its low bits there. A slice of a simhash is a
/// simhash, so the Hamming distance between two combined hashes is the
/// weighted sum of per-name distances. A missing hash contributes zeros.
/// `None` when none of the weighted hashes is present.
pub fn combine(
    hashes: &BTreeMap<String, u64>,
    weights: &BTreeMap<String, i64>,
) -> Option<u64> {
    let parts: Vec<(Option<u64>, u64)> = weights
        .iter()
        .map(|(name, weight)| {
            (hashes.get(name).copied(), (*weight).max(0) as u64)
        })
        .collect();
    if parts
        .iter()
        .all(|(hash, weight)| hash.is_none() || *weight == 0)
    {
        return None;
    }
    let weights: Vec<u64> = parts.iter().map(|(_, w)| *w).collect();
    let mut combined = 0u64;
    let mut offset = 0;
    for ((hash, _), bits) in parts.iter().zip(allocate_bits(&weights)) {
        if bits > 0 {
            let slice = hash.unwrap_or(0) & (u64::MAX >> (64 - bits));
            combined |= slice << offset;
            offset += bits;
        }
    }
    Some(combined)
}

/// Splits 64 bits in proportion to `weights` by largest remainder; ties go to
/// the earlier weight.
fn allocate_bits(weights: &[u64]) -> Vec<u32> {
    let total: u64 = weights.iter().sum();
    if total == 0 {
        return vec![0; weights.len()];
    }
    let mut bits: Vec<u32> =
        weights.iter().map(|w| (64 * w / total) as u32).collect();
    let mut order: Vec<usize> = (0..weights.len()).collect();
    order.sort_by_key(|&i| (std::cmp::Reverse(64 * weights[i] % total), i));
    let missing = 64 - bits.iter().sum::<u32>();
    for &i in order.iter().take(missing as usize) {
        bits[i] += 1;
    }
    bits
}

/// The hash of a sequence of string features: FNV-1a over the features,
/// each followed by a newline, then SplitMix64's finalizer. Equal exactly
/// when the sequences are equal; `None` for no features.
pub fn feature_hash<'a>(
    features: impl IntoIterator<Item = &'a str>,
) -> Option<u64> {
    let mut hash = FNV_OFFSET;
    let mut any = false;
    for feature in features {
        any = true;
        hash = fnv1a64_from(hash, feature.as_bytes());
        hash = fnv1a64_from(hash, b"\n");
    }
    any.then(|| mix64(hash))
}

const FNV_OFFSET: u64 = 0xcbf29ce484222325;

pub fn fnv1a64(bytes: &[u8]) -> u64 {
    fnv1a64_from(FNV_OFFSET, bytes)
}

fn fnv1a64_from(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn mix64(x: u64) -> u64 {
    let mut h = x.wrapping_add(0x9e3779b97f4a7c15);
    h = (h ^ (h >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94d049bb133111eb);
    h ^ (h >> 31)
}

/// `path=value` for every scalar in a JSON value, e.g. `{"a": [1]}` gives
/// `a.0=1`. Object keys are visited in sorted order, as the order is part of
/// the hash.
fn json_leaves(value: &serde_json::Value, path: &str, out: &mut Vec<String>) {
    let child = |key: &str| {
        if path.is_empty() {
            key.to_string()
        } else {
            format!("{path}.{key}")
        }
    };
    match value {
        serde_json::Value::Object(map) => {
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_by_key(|(key, _)| *key);
            for (key, item) in entries {
                json_leaves(item, &child(key), out);
            }
        }
        serde_json::Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                json_leaves(item, &child(&index.to_string()), out);
            }
        }
        scalar => out.push(format!("{path}={scalar}")),
    }
}

/// Learned action weights per state, as produced by training on traces
/// (see [`StepModel`]).
#[derive(Debug, Clone, Deserialize)]
pub struct Weights {
    pub format: u32,
    /// Share of probability kept by the fallback distribution (uniform for
    /// [`Policy::Replace`], the spec's for [`Policy::Prior`]), so no action is
    /// ever starved.
    #[serde(default = "Weights::default_epsilon")]
    pub epsilon: f64,
    #[serde(default)]
    pub policy: Policy,
    pub simhash: SimhashLookup,
    /// State simhash (16 hex digits) → action key → non-negative weight.
    pub states: HashMap<String, HashMap<String, f64>>,
    /// The `states` simhashes, sorted.
    #[serde(skip)]
    simhashes: Vec<(u64, String)>,
}

/// How learned weights combine with the spec's in a learned state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Policy {
    /// The learned weights replace the spec's.
    #[default]
    Replace,
    /// The spec's probabilities, scaled by the learned weights: where the
    /// learned weights are equal, the spec's distribution is kept.
    Prior,
}

/// How states are matched to learned ones.
#[derive(Debug, Clone, Deserialize)]
pub struct SimhashLookup {
    /// Weights of the named hashes in the state simhash ([`combine`]).
    pub weights: BTreeMap<String, i64>,
    /// The largest Hamming distance at which a learned state still applies.
    #[serde(default)]
    pub radius: u32,
}

impl Weights {
    const FORMAT: u32 = 3;

    fn default_epsilon() -> f64 {
        0.1
    }

    pub fn load(path: &Path) -> Result<Self> {
        let file = std::fs::File::open(path)
            .with_context(|| format!("opening weights {}", path.display()))?;
        let weights: Weights =
            serde_json::from_reader(std::io::BufReader::new(file))
                .with_context(|| {
                    format!("parsing weights {}", path.display())
                })?;
        if weights.format != Self::FORMAT {
            bail!(
                "unsupported weights format {} (expected {})",
                weights.format,
                Self::FORMAT
            );
        }
        if !(0.0..=1.0).contains(&weights.epsilon) {
            bail!("epsilon must be within [0, 1], got {}", weights.epsilon);
        }
        weights.index()
    }

    fn index(mut self) -> Result<Self> {
        let mut simhashes = self
            .states
            .keys()
            .map(|key| {
                u64::from_str_radix(key, 16)
                    .map(|hash| (hash, key.clone()))
                    .with_context(|| format!("state {key:?} is not a simhash"))
            })
            .collect::<Result<Vec<_>>>()?;
        simhashes.sort();
        self.simhashes = simhashes;
        Ok(self)
    }

    /// The learned state nearest to `hash` within the radius; ties go to the
    /// smaller simhash.
    fn nearest(&self, hash: u64) -> Option<&HashMap<String, f64>> {
        let mut best: Option<(u32, &str)> = None;
        for (candidate, key) in &self.simhashes {
            let distance = (candidate ^ hash).count_ones();
            if distance <= self.simhash.radius
                && best.is_none_or(|(d, _)| distance < d)
            {
                best = Some((distance, key));
            }
        }
        best.and_then(|(_, key)| self.states.get(key))
    }

    /// Replaces the spec weights with learned ones when a learned state is
    /// near enough. Available actions without a learned weight get the mean
    /// learned weight of the state. Returns the tree to pick from and whether
    /// a learned state was found. Otherwise the spec weights are kept.
    pub fn reweight<A, T: ActionTemplate<A> + Clone>(
        &self,
        state: &(impl RunState + ?Sized),
        snapshots: &[Snapshot],
        tree: Tree<T>,
    ) -> (Tree<T>, bool) {
        let Some(pruned) = tree.clone().prune() else {
            return (tree, false);
        };
        let learned =
            combine(&state_hashes(state, snapshots), &self.simhash.weights)
                .and_then(|hash| self.nearest(hash));
        let Some(learned) = learned else {
            return (tree, false);
        };

        let leaves = pruned.leaves_with_probabilities();
        let keys: Vec<String> =
            leaves.iter().map(|(t, _)| action_key(*t)).collect();
        let known: Vec<f64> = keys
            .iter()
            .filter_map(|k| learned.get(k).copied())
            .map(|w| w.max(0.0))
            .collect();
        let fallback = if known.is_empty() {
            1.0
        } else {
            known.iter().sum::<f64>() / known.len() as f64
        };
        let n = leaves.len() as f64;
        let raw: Vec<f64> = keys
            .iter()
            .zip(&leaves)
            .map(|(k, (_, spec))| {
                let w = learned.get(k).map_or(fallback, |w| w.max(0.0));
                match self.policy {
                    Policy::Replace => w,
                    Policy::Prior => w * spec,
                }
            })
            .collect();
        let total: f64 = raw.iter().sum();
        let branches = leaves
            .into_iter()
            .zip(raw)
            .map(|((template, spec), w)| {
                let floor = match self.policy {
                    Policy::Replace => 1.0 / n,
                    Policy::Prior => spec,
                };
                let learned_share = if total > 0.0 { w / total } else { floor };
                let p =
                    (1.0 - self.epsilon) * learned_share + self.epsilon * floor;
                let weight = (p * 60_000.0).round().clamp(1.0, 65_535.0) as u16;
                (
                    weight,
                    Tree::Leaf {
                        value: template.clone(),
                    },
                )
            })
            .collect();
        (Tree::Branch { branches }, true)
    }
}

/// Runs `inner`, but with the learned [`Weights`], if any, in the states
/// they cover: actions are picked from the tree this strategy reweights.
pub struct WeightedStrategy<Inner> {
    pub inner: Inner,
    pub weights: Option<Arc<Weights>>,
}

impl<Session: InterfaceSession, Inner: RunStrategy<Session>>
    RunStrategy<Session> for WeightedStrategy<Inner>
{
    type StopValue = Inner::StopValue;

    fn on_new_state(
        &mut self,
        state: &Session::State,
        tree: Tree<Session::ActionTemplate>,
        last_action: Option<&Session::Action>,
        snapshots: &[Snapshot],
        properties: PropertiesState,
    ) -> Result<ControlFlow<Self::StopValue, Session::Action>> {
        self.inner
            .on_new_state(state, tree, last_action, snapshots, properties)
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

    fn reweight(
        &self,
        state: &Session::State,
        snapshots: &[Snapshot],
        tree: Tree<Session::ActionTemplate>,
    ) -> Tree<Session::ActionTemplate> {
        let tree = self.inner.reweight(state, snapshots, tree);
        match &self.weights {
            Some(weights) => weights.reweight(state, snapshots, tree).0,
            None => tree,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    }

    struct At(u64);

    impl RunState for At {
        fn timestamp(&self) -> Time {
            Time::from_system_time(std::time::SystemTime::UNIX_EPOCH)
        }
        fn hashes(&self) -> BTreeMap<String, u64> {
            BTreeMap::from([("place".to_string(), self.0)])
        }
    }

    fn tree() -> Tree<Key> {
        Tree::Branch {
            branches: vec![
                (1, Tree::Leaf { value: Key("a") }),
                (1, Tree::Leaf { value: Key("b") }),
                (1, Tree::Leaf { value: Key("c") }),
            ],
        }
    }

    fn weights(
        stored: u64,
        radius: u32,
        learned: HashMap<String, f64>,
    ) -> Weights {
        Weights {
            format: Weights::FORMAT,
            epsilon: 0.0,
            policy: Policy::Replace,
            simhash: SimhashLookup {
                weights: BTreeMap::from([("place".to_string(), 1)]),
                radius,
            },
            simhashes: Vec::new(),
            states: HashMap::from([(format!("{stored:016x}"), learned)]),
        }
        .index()
        .unwrap()
    }

    #[test]
    fn reweight_known_state() {
        let learned = HashMap::from([
            (action_key(&Key("a")), 3.0),
            (action_key(&Key("b")), 0.0),
        ]);
        let (reweighted, hit) =
            weights(42, 0, learned).reweight(&At(42), &[], tree());
        assert!(hit);
        // c is unlisted and gets the mean learned weight (1.5).
        let probabilities: Vec<(&str, f64)> = reweighted
            .leaves_with_probabilities()
            .into_iter()
            .map(|(k, p)| (k.0, (p * 100.0).round() / 100.0))
            .collect();
        assert_eq!(probabilities, vec![("a", 0.67), ("b", 0.0), ("c", 0.33)]);
    }

    #[test]
    fn reweight_prior_scales_spec_probabilities() {
        let spec = Tree::Branch {
            branches: vec![
                (1, Tree::Leaf { value: Key("a") }),
                (1, Tree::Leaf { value: Key("b") }),
                (2, Tree::Leaf { value: Key("c") }),
            ],
        };
        let probabilities =
            |learned: &[(&'static str, f64)]| -> Vec<(&'static str, f64)> {
                let learned = learned
                    .iter()
                    .map(|(k, w)| (action_key(&Key(k)), *w))
                    .collect();
                let weights = Weights {
                    policy: Policy::Prior,
                    ..weights(42, 0, learned)
                };
                let (reweighted, hit) =
                    weights.reweight(&At(42), &[], spec.clone());
                assert!(hit);
                reweighted
                    .leaves_with_probabilities()
                    .into_iter()
                    .map(|(k, p)| (k.0, (p * 100.0).round() / 100.0))
                    .collect()
            };
        // Equal learned weights keep the spec's distribution.
        assert_eq!(
            probabilities(&[("a", 5.0), ("b", 5.0), ("c", 5.0)]),
            vec![("a", 0.25), ("b", 0.25), ("c", 0.5)]
        );
        // c is unlisted and gets the mean learned weight (1.5).
        assert_eq!(
            probabilities(&[("a", 2.0), ("b", 1.0)]),
            vec![("a", 0.33), ("b", 0.17), ("c", 0.5)]
        );
    }

    #[test]
    fn reweight_nearest_simhash_within_radius() {
        let learned = HashMap::from([(action_key(&Key("a")), 1.0)]);
        let near = weights(42 ^ 0b101, 2, learned.clone());
        assert!(near.reweight(&At(42), &[], tree()).1);
        let far = weights(42 ^ 0b111, 2, learned);
        let (reweighted, hit) = far.reweight(&At(42), &[], tree());
        assert!(!hit);
        assert_eq!(reweighted, tree());
    }

    #[test]
    fn action_taken_is_the_accepting_template() {
        let previous = tree();
        let model =
            StepModel::new(&tree(), Some((&previous, &Key("b"))));
        assert_eq!(model.action_taken, Some(action_key(&Key("b"))));
        assert_eq!(model.actions_available.len(), 3);
    }

    #[test]
    fn combine_matches_python() {
        // Same inputs as the matching test of the Python training code,
        // which must combine hashes the same way.
        let hashes = BTreeMap::from([
            ("coverage".to_string(), 0x00ff_00ff_00ff_00ff),
            (
                "location".to_string(),
                feature_hash(["examples", "#/active"]).unwrap(),
            ),
            (
                "snapshot.dot".to_string(),
                feature_hash(["x=1", "y=3"]).unwrap(),
            ),
        ]);
        let weights = BTreeMap::from([
            ("coverage".to_string(), 1),
            ("location".to_string(), 1),
            ("screen".to_string(), 1),
            ("snapshot.dot".to_string(), 1),
        ]);
        assert_eq!(
            format!("{:016x}", combine(&hashes, &weights).unwrap()),
            "39c70000457d00ff"
        );
    }

    #[test]
    fn combine_concatenates_weighted_slices() {
        assert_eq!(allocate_bits(&[1, 1, 1]), vec![22, 21, 21]);
        assert_eq!(allocate_bits(&[1, 0, 3]), vec![16, 0, 48]);
        let hashes = BTreeMap::from([
            ("a".to_string(), u64::MAX),
            ("b".to_string(), 0b10),
        ]);
        let half = BTreeMap::from([
            ("a".to_string(), 1),
            ("b".to_string(), 1),
            ("c".to_string(), 0),
        ]);
        // Low 32 bits of a, then low 32 bits of b.
        assert_eq!(combine(&hashes, &half), Some(0x0000_0002_ffff_ffff));
        let only_c = BTreeMap::from([("c".to_string(), 1)]);
        assert_eq!(combine(&hashes, &only_c), None);
    }

    #[test]
    fn json_leaves_flatten_paths() {
        let mut leaves = Vec::new();
        json_leaves(
            &serde_json::json!({"b": [1, {"c": null}], "a": "x"}),
            "",
            &mut leaves,
        );
        // Object keys sorted ("a" before "b"), array order kept.
        assert_eq!(leaves, vec!["a=\"x\"", "b.0=1", "b.1.c=null"]);
    }

}
