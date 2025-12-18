use rand::{
    self,
    distr::{Alphanumeric, SampleString},
    seq::IndexedRandom,
};
use serde::Deserialize;

use crate::browser::actions::{tree::Tree, BrowserAction, Timeout};

pub fn generate_action<R: rand::Rng>(
    rng: &mut R,
    action: &BrowserAction,
) -> BrowserAction {
    match action {
        BrowserAction::Back => BrowserAction::Back,
        BrowserAction::Click { .. } => action.clone(),
        BrowserAction::TypeText { .. } => {
            let length = rng.random_range(1..16);
            BrowserAction::TypeText {
                text: Alphanumeric.sample_string(rng, length),
            }
        }
        BrowserAction::PressKey { .. } => {
            let code: u8 =
                *(vec![13, 27]).choose(rng).expect("there should be a code");
            BrowserAction::PressKey { code }
        }
        BrowserAction::ScrollUp { origin, distance } => {
            let distance = rng.random_range((*distance / 2.0)..=(*distance));
            BrowserAction::ScrollUp {
                origin: origin.clone(),
                distance,
            }
        }
        BrowserAction::ScrollDown { origin, distance } => {
            let distance = rng.random_range((*distance / 2.0)..=(*distance));
            BrowserAction::ScrollDown {
                origin: origin.clone(),
                distance,
            }
        }
        BrowserAction::Reload => BrowserAction::Reload,
    }
}

pub fn pick_from_tree<'a, T: Clone, R: rand::Rng>(
    rng: &mut R,
    tree: &Tree<T>,
) -> T {
    match tree {
        Tree::Leaf(x) => x.clone(),
        Tree::Branch(branches) => {
            let branch = branches
                .choose(rng)
                .expect("there should be at least one branch");
            pick_from_tree(rng, branch)
        }
    }
}

pub fn pick_action<R: rand::Rng>(
    rng: &mut R,
    actions: Tree<(BrowserAction, Timeout)>,
) -> (BrowserAction, Timeout) {
    let (action, timeout) = pick_from_tree(rng, &actions);
    (generate_action(rng, &action), timeout)
}
