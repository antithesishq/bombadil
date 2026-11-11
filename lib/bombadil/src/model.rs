//! What a trace records about each step's actions (the available templates
//! and the one taken), so that action weights can be learned from traces.
//! Actions are identified by their template's `category_hash` under
//! [`StableHasher`].

use std::hash::Hasher;

use bombadil_schema::ActionKey;
use rand::{RngExt, TryRng};

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

/// Key of the first of `actions_available` that accepts `action`, for an
/// action replayed from a trace.
pub fn accepting_key<A, T: ActionTemplate<A>>(
    actions_available: &Leaves<'_, T>,
    action: &A,
) -> Option<ActionKey> {
    actions_available
        .as_slice()
        .iter()
        .find(|(template, _)| template.accepts(action))
        .map(|(template, _)| action_key(*template))
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
    use crate::tree::Tree;
    use crate::tree::Weight;
    use hegel::TestCase;
    use hegel::generators::{booleans, integers};

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

    #[test]
    fn action_key_is_fnv1a64() {
        assert_eq!(action_key(&Key("a")), ActionKey(0xaf63_dc4c_8601_ec8c));
        assert_eq!(
            action_key(&Key("foobar")),
            ActionKey(0x8594_4171_f739_67e8)
        );
    }
}
