//! What a trace records about each step's actions (the available templates
//! and the one taken), so that action weights can be learned from traces.
//! Actions are identified by their template's `category_hash` under
//! [`StableHasher`].

use std::hash::Hasher;

use bombadil_schema::ActionKey;
use bombadil_trace::StepError;
use rand::{RngExt, TryRng};
use serde::{Deserialize, Serialize};

use crate::driver::ActionTemplate;
use crate::tree::Leaves;
use stdx::hash::StableHasher;

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

/// An action read back from a trace, for replay. Serialized, it is what
/// a trace holds for an action about to be applied.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Recorded<A> {
    pub action: A,
    /// The trace's `action_taken`: the key of the template the action
    /// was credited to. `None` for traces that don't record it.
    #[serde(rename = "action_taken", default)]
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

/// What to say once the actions of a run that ended without a state are replayed.
pub fn replayed_end_report(
    recorded: &StepError,
    state_followed: bool,
) -> String {
    let this_time = if state_followed {
        "this time a state followed"
    } else {
        "this time no state followed either"
    };
    format!(
        "The recorded run ended after the last action: {recorded}; {this_time}."
    )
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

/// The template's `category_hash` under [`StableHasher`].
pub fn action_key<A>(template: &impl ActionTemplate<A>) -> ActionKey {
    let mut hasher = StableHasher::default();
    template.category_hash(&mut hasher);
    ActionKey(hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::Tree;
    use crate::tree::Weight;
    use hegel::TestCase;
    use hegel::generators::{booleans, integers};
    use std::convert::Infallible;

    #[test]
    fn reports_how_the_recorded_run_ended() {
        assert_eq!(
            replayed_end_report(&StepError::Stalled, true),
            "The recorded run ended after the last action: the driver \
             stalled; this time a state followed."
        );
        assert_eq!(
            replayed_end_report(&StepError::Unknown, false),
            "The recorded run ended after the last action: unknown (the \
             trace ends before what came of it); this time no state \
             followed either."
        );
    }

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
    #[derive(Clone, Debug, PartialEq)]
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

    #[test]
    fn action_key_is_fnv1a64() {
        assert_eq!(action_key(&Key("a")), ActionKey(0xaf63_dc4c_8601_ec8c));
        assert_eq!(
            action_key(&Key("foobar")),
            ActionKey(0x8594_4171_f739_67e8)
        );
    }
}
