//! What a trace records about each step's actions (the available templates
//! and the one taken), so that action weights can be learned from traces.
//! Actions are identified by their template's `category_hash` under
//! [`StableHasher`].

use std::collections::HashSet;
use std::hash::Hasher;
use std::io::{self, Write};
use std::iter;
use std::sync::{Arc, Mutex, mpsc};

use anyhow::{Context, Result};
use bombadil_schema::ActionKey;
use rand::{RngExt, TryRng};
use serde::Serialize;

use crate::driver::ActionTemplate;
use crate::tree::Leaves;
use stdx::hash::{FNV_OFFSET, StableHasher, fnv1a64_from};

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

/// An action read back from a trace, for replay.
#[derive(Clone, Debug, PartialEq)]
pub struct Recorded<A> {
    pub action: A,
    /// The trace's `action_taken`: the key of the template the action
    /// was credited to. `None` for traces that don't record it.
    pub template: Option<ActionKey>,
}

impl<A> Recorded<A> {
    pub fn map<B>(self, f: impl FnOnce(A) -> B) -> Recorded<B> {
        Recorded {
            action: f(self.action),
            template: self.template,
        }
    }
}

/// Key of the template to credit for a replayed action, or `None` if no
/// available template accepts it.
///
/// The recorded key wins if a template with that key accepts the action.
/// Otherwise the first accepting template is credited, which is a guess
/// when several accept: the action alone can't tell them apart. That
/// happens when the trace has no key, or no accepting template has it: a
/// shrunk sequence, a changed spec, or a click on an element whose
/// fingerprint changed (the key hashes the whole fingerprint, accepting
/// matches it loosely).
pub fn accepting_key<A, T: ActionTemplate<A>>(
    actions_available: &Leaves<'_, T>,
    recorded: &Recorded<A>,
) -> Option<ActionKey> {
    let templates = || {
        actions_available
            .as_slice()
            .iter()
            .map(|(template, _)| *template)
    };
    if let Some(key) = recorded.template
        && templates().any(|template| {
            action_key(template) == key && template.accepts(&recorded.action)
        })
    {
        return Some(key);
    }
    templates()
        .find(|template| template.accepts(&recorded.action))
        .map(action_key)
}

/// The table `--action-templates` writes: the template behind each key in a
/// trace's `actions_available`, in order of first sight. A key ignores some
/// of its template's parameters (see `category_hash`), and the table holds
/// the first template seen under it.
#[derive(Clone, Debug)]
pub struct TemplateRegistry(Arc<Mutex<TemplateTable>>);

#[derive(Debug)]
struct TemplateTable {
    keys: HashSet<ActionKey>,
    /// Receives the row of each new key, in order of first sight.
    rows_new: Option<mpsc::Sender<Vec<u8>>>,
}

#[derive(Serialize)]
struct TemplateRow<'a, T> {
    key: ActionKey,
    template: &'a T,
}

impl TemplateRegistry {
    /// A registry that sends the row of each new key on the returned
    /// channel, until [`TemplateRegistry::close`]. A row is a
    /// `{"key": …, "template": …}` JSON object and a newline.
    pub fn with_channel() -> (Self, mpsc::Receiver<Vec<u8>>) {
        let (sender, receiver) = mpsc::channel();
        let table = TemplateTable {
            keys: HashSet::new(),
            rows_new: Some(sender),
        };
        (TemplateRegistry(Arc::new(Mutex::new(table))), receiver)
    }

    /// Closes the channel of [`TemplateRegistry::with_channel`].
    pub fn close(&self) {
        self.lock().rows_new = None;
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, TemplateTable> {
        self.0.lock().expect("template registry poisoned")
    }

    /// Adds the templates of `leaves` whose keys are new.
    pub fn record<A, T: ActionTemplate<A> + Serialize>(
        &self,
        leaves: &Leaves<'_, T>,
    ) -> Result<()> {
        let mut unseen = Vec::new();
        {
            let table = self.lock();
            for (template, _) in leaves.as_slice() {
                let key = action_key(*template);
                if !table.keys.contains(&key) {
                    unseen.push((key, *template, Vec::new()));
                }
            }
        }
        if unseen.is_empty() {
            return Ok(());
        }

        // Serialized outside the lock, which other runs take at every step.
        for (key, template, row) in &mut unseen {
            serde_json::to_writer(
                &mut *row,
                &TemplateRow {
                    key: *key,
                    template: *template,
                },
            )
            .context("serializing an action template")?;
            row.push(b'\n');
        }

        let table = &mut *self.lock();
        for (key, _, row) in unseen {
            if table.keys.insert(key)
                && let Some(rows_new) = &table.rows_new
            {
                // The receiver is gone if writing failed, which the
                // writer reports.
                let _ = rows_new.send(row);
            }
        }
        Ok(())
    }
}

/// Writes the rows of `rows_new` until the channel closes, flushing whenever
/// it runs empty.
pub fn write_template_rows(
    rows_new: mpsc::Receiver<Vec<u8>>,
    writer: impl io::Write,
) -> io::Result<()> {
    let mut writer = io::BufWriter::new(writer);
    while let Ok(row) = rows_new.recv() {
        for row in iter::once(row).chain(rows_new.try_iter()) {
            writer.write_all(&row)?;
        }
        writer.flush()?;
    }
    Ok(())
}

/// The template's `category_hash` under [`StableHasher`].
pub fn action_key<A>(template: &impl ActionTemplate<A>) -> ActionKey {
    let mut hasher = StableHasher::default();
    template.category_hash(&mut hasher);
    ActionKey(hasher.finish())
}

/// The hash of a sequence of string features: FNV-1a over the features,
/// each followed by a newline, then SplitMix64's finalizer. Equal exactly
/// when the sequences are equal; `None` for no features.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::Tree;
    use crate::tree::Weight;
    use hegel::TestCase;
    use hegel::generators::{booleans, integers};
    use std::convert::Infallible;

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

    const GROUP_MAX: u8 = 2;

    #[derive(Clone, Debug, PartialEq)]
    struct Group(u8);

    /// Templates of a group generate and accept the same action. The key
    /// is the name alone.
    #[derive(Clone, Debug, PartialEq, Serialize)]
    struct Overlapping {
        name: &'static str,
        group: u8,
    }

    impl ActionTemplate<Group> for Overlapping {
        fn generate<R: rand::TryRng + rand::RngExt>(&self, _: &mut R) -> Group {
            Group(self.group)
        }
        fn accepts(&self, original: &Group) -> bool {
            self.group == original.0
        }
        fn category_hash<H: Hasher>(&self, hasher: &mut H) {
            hasher.write(self.name.as_bytes());
        }
    }

    /// A tree of templates in a few groups, so several accept the same
    /// action. Also returns them in depth-first order.
    fn overlapping_tree(
        tc: &TestCase,
    ) -> (Tree<Overlapping>, Vec<Overlapping>) {
        let mut templates = vec![];
        let tree = trees(tc, 2, &mut 0)
            .try_map(&mut |Key(name)| {
                let group = tc.draw_silent(integers().max_value(GROUP_MAX));
                let template = Overlapping { name, group };
                templates.push(template.clone());
                Ok::<_, Infallible>(template)
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
        for recorded_key in [Some(picked.template), None] {
            let recorded = Recorded {
                action: picked.action.clone(),
                template: recorded_key,
            };
            assert_eq!(
                accepting_key(&leaves, &recorded),
                Some(picked.template)
            );
            let unavailable = Recorded {
                action: Key("other"),
                template: recorded_key,
            };
            assert_eq!(accepting_key(&leaves, &unavailable), None);
        }
    }

    #[hegel::test]
    fn replayed_action_is_credited_to_the_accepting_template_with_its_key(
        tc: TestCase,
    ) {
        let (tree, templates) = overlapping_tree(&tc);
        tc.assume(!templates.is_empty());
        let template = &templates
            [tc.draw(integers::<usize>().max_value(templates.len() - 1))];
        let recorded = Recorded {
            action: template.generate(&mut rand::rng()),
            template: Some(action_key(template)),
        };

        assert_eq!(
            accepting_key(&tree.leaves(), &recorded),
            Some(action_key(template))
        );
    }

    #[hegel::test]
    fn replayed_action_is_credited_to_the_first_accepting_template_otherwise(
        tc: TestCase,
    ) {
        let (tree, templates) = overlapping_tree(&tc);
        let action = Group(tc.draw(integers().max_value(GROUP_MAX)));
        // No key, an unknown key, or the key of a template that rejects
        // the action.
        let rejecting: Vec<&Overlapping> = templates
            .iter()
            .filter(|template| !template.accepts(&action))
            .collect();
        let choice =
            tc.draw(integers::<usize>().max_value(rejecting.len() + 1));
        let recorded_key = match choice.checked_sub(2) {
            Some(index) => Some(action_key(rejecting[index])),
            None if choice == 1 => Some(action_key(&Key("other"))),
            None => None,
        };
        let recorded = Recorded {
            action: action.clone(),
            template: recorded_key,
        };

        let expected = templates
            .iter()
            .find(|template| template.accepts(&action))
            .map(action_key);
        assert_eq!(accepting_key(&tree.leaves(), &recorded), expected);
    }

    #[derive(Debug, PartialEq, serde::Deserialize)]
    struct Row {
        key: ActionKey,
        template: RowTemplate,
    }

    #[derive(Debug, PartialEq, serde::Deserialize)]
    struct RowTemplate {
        name: String,
        group: u8,
    }

    // `Overlapping` templates of one name share a key whatever their group, so the
    // table must hold the group seen first.
    #[hegel::test]
    fn template_table_holds_the_first_template_of_each_key_in_order(
        tc: TestCase,
    ) {
        let (registry, rows_new) = TemplateRegistry::with_channel();
        let mut expected: Vec<Row> = vec![];
        for _ in 0..tc.draw(integers::<usize>().max_value(4)) {
            let size = tc.draw(integers::<usize>().max_value(6));
            let branches = (0..size)
                .map(|_| {
                    let name = tc
                        .draw(hegel::generators::sampled_from(NAMES.to_vec()));
                    let group = tc.draw(integers::<u8>().max_value(GROUP_MAX));
                    let value = Overlapping { name, group };
                    let key = action_key(&value);
                    if expected.iter().all(|row| row.key != key) {
                        expected.push(Row {
                            key,
                            template: RowTemplate {
                                name: name.to_string(),
                                group,
                            },
                        });
                    }
                    (1, Tree::Leaf { value })
                })
                .collect();
            let tree = Tree::Branch { branches };
            registry.record(&tree.leaves()).unwrap();
        }
        registry.close();

        let mut table = vec![];
        write_template_rows(rows_new, &mut table).unwrap();
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
}
