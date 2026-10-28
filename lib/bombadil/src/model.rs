//! What a trace records about each step's actions (the available templates
//! and the one taken), so that action weights can be learned from traces.
//! Actions are identified by their template's `category_hash` under
//! [`StableHasher`].

use std::hash::Hasher;

use bombadil_schema::ActionKey;
use rand::{RngExt, TryRng};

use crate::driver::ActionTemplate;
use crate::hash::{FNV_OFFSET, fnv1a64_from};
use crate::tree::Leaves;

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
/// each followed by a newline, then SplitMix64's finalizer. Equal exactly
/// when the sequences are equal; `None` for no features.
pub fn feature_hash<'a>(
    features: impl IntoIterator<Item = &'a str>,
) -> Option<u64> {
    let mut hash = FNV_OFFSET;
    let mut any = false;
    for feature in features {
        any = true;
        hash = fnv1a64_from(hash, feature.bytes());
        hash = fnv1a64_from(hash, *b"\n");
    }
    any.then(|| mix64(hash))
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
    use crate::hash::fnv1a64;
    use crate::tree::Tree;
    use crate::tree::Weight;
    use hegel::TestCase;
    use hegel::generators::{booleans, integers};
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
    }

    /// Accepts any action of its group; produced only its own.
    #[derive(Clone, Debug, PartialEq)]
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
}
