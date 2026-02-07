use std::time::Duration;

use crate::browser::actions::{BrowserAction, BrowserActionCandidate, Timeout};
use crate::tree::Tree;
use rand::{
    self,
    distr::{Alphanumeric, SampleString},
    seq::IndexedRandom,
};

pub fn generate_action<R: rand::Rng>(
    rng: &mut R,
    action: &BrowserActionCandidate,
) -> BrowserAction {
    match action {
        BrowserActionCandidate::Back => BrowserAction::Back,
        BrowserActionCandidate::Click {
            name,
            content,
            point,
        } => BrowserAction::Click {
            name: name.clone(),
            content: content.clone(),
            point: *point,
        },
        BrowserActionCandidate::TypeText { .. } => {
            let length = rng.random_range(1..16);
            let delay = Duration::from_millis(rng.random_range(10..200));
            BrowserAction::TypeText {
                text: Alphanumeric.sample_string(rng, length),
                delay,
            }
        }
        BrowserActionCandidate::PressKey => {
            let code: u8 =
                *([13, 27]).choose(rng).expect("there should be a code");
            BrowserAction::PressKey { code }
        }
        BrowserActionCandidate::ScrollUp { origin, distance } => {
            let distance = rng.random_range((*distance / 2.0)..=(*distance));
            BrowserAction::ScrollUp {
                origin: *origin,
                distance,
            }
        }
        BrowserActionCandidate::ScrollDown { origin, distance } => {
            let distance = rng.random_range((*distance / 2.0)..=(*distance));
            BrowserAction::ScrollDown {
                origin: *origin,
                distance,
            }
        }
        BrowserActionCandidate::Reload => BrowserAction::Reload,
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
    actions: Tree<(BrowserActionCandidate, Timeout)>,
) -> (BrowserAction, Timeout) {
    let (action, timeout) = pick_from_tree(rng, &actions);
    (generate_action(rng, &action), timeout)
}
