use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use chromiumoxide::cdp::browser_protocol::{input, page};
use chromiumoxide::Page;
use include_dir::{include_dir, Dir};
use serde::Serialize;
use serde::{de::DeserializeOwned, Deserialize};
use serde_json as json;

use crate::browser::actions::keys::key_name;
use crate::browser::actions::tree::{Tree, Weight};
use crate::browser::state::BrowserState;
use crate::geometry::Point;

#[cfg(feature = "hegel")]
pub mod hegel;
pub mod keys;
pub mod random;
pub mod tree;

#[allow(unused, reason = "some fields are useful for debugging")]
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum BrowserAction {
    Back,
    Click {
        name: String,
        content: Option<String>,
        point: Point,
    },
    TypeText {
        text: String,
    },
    PressKey {
        code: u8,
    },
    ScrollUp {
        origin: Point,
        distance: f64,
    },
    ScrollDown {
        origin: Point,
        distance: f64,
    },
    Reload,
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub struct Timeout {
    millis: u64,
}

impl Timeout {
    pub fn from_millis(millis: u64) -> Self {
        Timeout { millis }
    }

    pub fn from_secs(secs: u64) -> Self {
        Timeout {
            millis: secs.saturating_mul(1000),
        }
    }

    pub fn to_duration(&self) -> Duration {
        Duration::from_millis(self.millis)
    }
}

impl BrowserAction {
    pub async fn apply(&self, page: &Page) -> Result<()> {
        match self {
            BrowserAction::Back => {
                let history =
                    page.execute(page::GetNavigationHistoryParams {}).await?;
                if history.current_index == 0 {
                    bail!("can't go back from first navigation entry");
                }
                let last: page::NavigationEntry = history.entries
                    [(history.current_index - 1) as usize]
                    .clone();
                page.execute(
                    page::NavigateToHistoryEntryParams::builder()
                        .entry_id(last.id)
                        .build()
                        .map_err(|err| anyhow!(err))?,
                )
                .await?;
            }
            BrowserAction::Reload => {
                page.reload().await?;
            }
            BrowserAction::ScrollUp { origin, distance } => {
                page.execute(
                    input::SynthesizeScrollGestureParams::builder()
                        .x(origin.x)
                        .y(origin.y)
                        .y_distance(*distance)
                        .speed((distance.abs() * 10.0) as i64)
                        .build()
                        .map_err(|err| anyhow!(err))?,
                )
                .await?;
            }
            BrowserAction::ScrollDown { origin, distance } => {
                page.execute(
                    input::SynthesizeScrollGestureParams::builder()
                        .x(origin.x)
                        .y(origin.y)
                        .y_distance(-distance)
                        .speed((distance.abs() * 10.0) as i64)
                        .build()
                        .map_err(|err| anyhow!(err))?,
                )
                .await?;
            }
            BrowserAction::Click { point, .. } => {
                page.click((*point).into()).await?;
            }
            BrowserAction::TypeText { text } => {
                // TODO: maybe dispatch key presses instead with some random timing inbetween
                page.execute(input::InsertTextParams::new(text)).await?;
            }
            BrowserAction::PressKey { code } => {
                let build_params = |event_type| {
                    if let Some(name) = key_name(*code) {
                        input::DispatchKeyEventParams::builder()
                            .r#type(event_type)
                            .native_virtual_key_code(*code as i64)
                            .windows_virtual_key_code(*code as i64)
                            .code(name)
                            .key(name)
                            .unmodified_text("\r")
                            .text("\r")
                            .build()
                            .map_err(|err| anyhow!(err))
                    } else {
                        bail!("unknown key with code: {:?}", code)
                    }
                };
                page.execute(build_params(
                    input::DispatchKeyEventType::RawKeyDown,
                )?)
                .await?;
                page.execute(build_params(input::DispatchKeyEventType::Char)?)
                    .await?;
                page.execute(build_params(input::DispatchKeyEventType::KeyUp)?)
                    .await?;
            }
        };
        Ok(())
    }
}

static ACTIONS_DIR: Dir =
    include_dir!("$CARGO_MANIFEST_DIR/src/browser/actions");

async fn run_script<Input: Into<json::Value>, Output: DeserializeOwned>(
    state: &BrowserState,
    name: impl Into<&str>,
    input: Input,
) -> Result<Output> {
    let script_path = format!("{}.js", name.into());
    let script_file = ACTIONS_DIR
        .get_file(&script_path)
        .ok_or(anyhow!("missing script {}", script_path))?;

    let script_contents = script_file
        .contents_utf8()
        .ok_or(anyhow!("failed to get script contents"))?;

    state
        .evaluate_function_call(script_contents, vec![input.into()])
        .await
}

async fn run_actions_script(
    state: &BrowserState,
    name: impl Into<&str>,
) -> Result<Vec<Tree<(BrowserAction, Timeout)>>> {
    let actions: Vec<(Weight, u64, BrowserAction)> =
        run_script(state, name, ()).await?;
    Ok(actions
        .iter()
        .map(|(_weight, timeout_ms, action)| {
            Tree::Leaf((action.clone(), Timeout::from_millis(*timeout_ms)))
        })
        .collect::<Vec<_>>())
}

pub async fn available_actions(
    state: &BrowserState,
) -> Result<Tree<(BrowserAction, Timeout)>> {
    let tree = Tree::Branch(vec![
        (Tree::Branch(run_actions_script(state, "clicks").await?)),
        (Tree::Branch(run_actions_script(state, "inputs").await?)),
        (Tree::Branch(run_actions_script(state, "scrolls").await?)),
    ])
    .prune();

    if state.content_type != "text/html" {
        return Ok(Tree::Leaf((BrowserAction::Back, Timeout::from_secs(2))));
    }

    if let Some(tree) = tree {
        Ok(tree)
    } else {
        Ok(Tree::Branch(vec![
            (Tree::Leaf((BrowserAction::Back, Timeout::from_secs(2)))),
            Tree::Leaf((BrowserAction::Reload, Timeout::from_secs(1))),
        ]))
    }
}
