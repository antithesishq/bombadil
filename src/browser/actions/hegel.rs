use hegel::r#gen::{floats, just, one_of, BoxedGenerator, Generate};
use serde::Deserialize;

use crate::browser::actions::{tree::Tree, BrowserAction, Timeout};

pub fn generate_action(
    action: &BrowserAction,
) -> BoxedGenerator<BrowserAction> {
    match action {
        BrowserAction::Back => BoxedGenerator::new(just(BrowserAction::Back)),
        BrowserAction::Click { .. } => {
            BoxedGenerator::new(just(action.clone()))
        }
        BrowserAction::TypeText { .. } => BoxedGenerator::new(
            hegel::r#gen::text().map(|text| BrowserAction::TypeText { text }),
        ),
        BrowserAction::PressKey { .. } => BoxedGenerator::new(
            one_of(vec![
                BoxedGenerator::new(hegel::r#gen::just::<u8>(13)),
                BoxedGenerator::new(hegel::r#gen::just::<u8>(27)),
            ])
            .map(|code| BrowserAction::PressKey { code }),
        ),
        BrowserAction::ScrollUp { origin, distance } => {
            let origin = origin.clone();
            BoxedGenerator::new(
                floats().with_min(*distance / 2.0).with_max(*distance).map(
                    move |distance| BrowserAction::ScrollUp {
                        origin,
                        distance,
                    },
                ),
            )
        }
        BrowserAction::ScrollDown { origin, distance } => {
            let origin = origin.clone();
            BoxedGenerator::new(
                floats().with_min(*distance / 2.0).with_max(*distance).map(
                    move |distance| BrowserAction::ScrollDown {
                        origin,
                        distance,
                    },
                ),
            )
        }
        BrowserAction::Reload => {
            BoxedGenerator::new(just(BrowserAction::Reload))
        }
    }
}

pub fn pick_from_tree<T: for<'de> Deserialize<'de> + 'static>(
    tree: &Tree<BoxedGenerator<T>>,
) -> BoxedGenerator<T> {
    match tree {
        Tree::Leaf(x) => x.clone(),
        Tree::Branch(branches) => BoxedGenerator::new(hegel::r#gen::one_of(
            branches.iter().map(pick_from_tree).collect(),
        )),
    }
}

pub fn pick_action(
    actions: Tree<(BrowserAction, Timeout)>,
) -> (BrowserAction, Timeout) {
    pick_from_tree(&actions.map(|(action, timeout)| {
        let timeout = timeout.clone();
        BoxedGenerator::new(
            generate_action(action).map(move |action| (action, timeout)),
        )
    }))
    .generate()
}
