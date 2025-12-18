use hegel::r#gen::{floats, just, one_of, BoxedGenerator, Generate};
use serde::Deserialize;

use crate::browser::actions::{tree::Tree, BrowserAction, Timeout};

pub fn generate_action<'a>(
    action: BrowserAction,
) -> BoxedGenerator<'a, BrowserAction> {
    match action {
        BrowserAction::Back => just(BrowserAction::Back).boxed(),
        BrowserAction::Click { .. } => just(action.clone()).boxed(),
        BrowserAction::TypeText { .. } => hegel::r#gen::text()
            .map(|text| BrowserAction::TypeText { text })
            .boxed(),
        BrowserAction::PressKey { .. } => one_of(vec![
            hegel::r#gen::just::<u8>(13).boxed(),
            hegel::r#gen::just::<u8>(27).boxed(),
        ])
        .map(|code| BrowserAction::PressKey { code })
        .boxed(),
        BrowserAction::ScrollUp { origin, distance } => {
            let origin = origin.clone();
            floats()
                .with_min(distance / 2.0)
                .with_max(distance)
                .map(move |distance| BrowserAction::ScrollUp {
                    origin,
                    distance,
                })
                .boxed()
        }
        BrowserAction::ScrollDown { origin, distance } => {
            let origin = origin.clone();
            floats()
                .with_min(distance / 2.0)
                .with_max(distance)
                .map(move |distance| BrowserAction::ScrollDown {
                    origin,
                    distance,
                })
                .boxed()
        }
        BrowserAction::Reload => just(BrowserAction::Reload).boxed(),
    }
}

pub fn pick_from_tree<'a, T: for<'de> Deserialize<'de> + 'static>(
    tree: &'a Tree<BoxedGenerator<T>>,
) -> BoxedGenerator<'a, T> {
    match tree {
        Tree::Leaf(x) => x.clone(),
        Tree::Branch(branches) => {
            hegel::r#gen::one_of(branches.iter().map(pick_from_tree).collect())
                .boxed()
        }
    }
}

pub fn pick_action(
    actions: Tree<(BrowserAction, Timeout)>,
) -> (BrowserAction, Timeout) {
    pick_from_tree(&actions.map(|(action, timeout)| {
        let action = action.clone();
        let timeout = timeout.clone();
        generate_action(action)
            .map(move |action| (action, timeout))
            .boxed()
    }))
    .generate()
}
