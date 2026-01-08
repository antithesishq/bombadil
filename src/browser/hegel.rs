/// Exceluded for now, until Hegel is available in crates.io.
///
use hegel::r#gen::{floats, just, one_of, BoxedGenerator, Generate};
use serde::Deserialize;

use crate::browser::actions::{
    tree::Tree, BrowserAction, BrowserActionCandidate, Timeout, TypeTextFormat,
};

pub fn generate_action<'a>(
    action: BrowserActionCandidate,
) -> BoxedGenerator<'a, BrowserAction> {
    match action {
        BrowserActionCandidate::Back => just(BrowserAction::Back).boxed(),
        BrowserActionCandidate::Click {
            name,
            content,
            point,
        } => just(BrowserAction::Click {
            name: name.clone(),
            content: content.clone(),
            point: point.clone(),
        })
        .boxed(),
        BrowserActionCandidate::TypeText { format } => match format {
            TypeTextFormat::Text => hegel::r#gen::text()
                .map(|text| BrowserAction::TypeText { text })
                .boxed(),
            TypeTextFormat::Email => hegel::r#gen::emails()
                .map(|text| BrowserAction::TypeText { text })
                .boxed(),
            TypeTextFormat::Number => hegel::r#gen::integers::<u16>()
                .map(|n| BrowserAction::TypeText {
                    text: format!("{}", n),
                })
                .boxed(),
        },
        BrowserActionCandidate::PressKey { .. } => one_of(vec![
            hegel::r#gen::just::<u8>(13).boxed(),
            hegel::r#gen::just::<u8>(27).boxed(),
        ])
        .map(|code| BrowserAction::PressKey { code })
        .boxed(),
        BrowserActionCandidate::ScrollUp { origin, distance } => {
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
        BrowserActionCandidate::ScrollDown { origin, distance } => {
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
        BrowserActionCandidate::Reload => just(BrowserAction::Reload).boxed(),
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
    actions: Tree<(BrowserActionCandidate, Timeout)>,
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
