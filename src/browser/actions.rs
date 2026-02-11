use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use chromiumoxide::Page;
use chromiumoxide::cdp::browser_protocol::{input, page};
use include_dir::{Dir, include_dir};
use serde::Serialize;
use serde::{Deserialize, de::DeserializeOwned};
use tokio::time::sleep;
use url::Url;

use crate::browser::keys::key_name;
use crate::browser::state::BrowserState;
use crate::geometry::Point;
use crate::tree::{Tree, Weight};
use crate::url::is_within_domain;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum BrowserActionCandidate {
    Back,
    Click {
        name: String,
        content: Option<String>,
        point: Point,
    },
    TypeText {
        format: TypeTextFormat,
    },
    PressKey,
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum TypeTextFormat {
    Text,
    Email,
    Number,
}

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
        delay: Duration,
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
pub struct Timeout(pub u64);

impl Timeout {
    pub fn from_secs(secs: u64) -> Self {
        Timeout(secs.saturating_mul(1000))
    }

    pub fn to_duration(&self) -> Duration {
        let Timeout(millis) = self;
        Duration::from_millis(*millis)
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
            BrowserAction::TypeText { text, delay } => {
                for char in text.chars() {
                    sleep(*delay).await;
                    page.execute(input::InsertTextParams::new(char)).await?;
                }
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

static ACTIONS_DIR: Dir = include_dir!("$CARGO_MANIFEST_DIR/target/actions");

async fn run_script<Output: DeserializeOwned>(
    state: &BrowserState,
    name: impl Into<&str>,
) -> Result<Output> {
    let script_path = format!("{}.js", name.into());
    let script_file = ACTIONS_DIR
        .get_file(&script_path)
        .ok_or(anyhow!("missing script {}", script_path))?;

    let script_contents = script_file
        .contents_utf8()
        .ok_or(anyhow!("failed to get script contents"))?;

    state
        .evaluate_function_call(script_contents, vec![])
        .await
        .map_err(|err| anyhow!("script call ({}) failed: {}", script_path, err))
}

async fn run_actions_script(
    state: &BrowserState,
    name: impl Into<&str>,
) -> Result<Vec<Tree<(BrowserActionCandidate, Timeout)>>> {
    let actions: Vec<(Weight, Timeout, BrowserActionCandidate)> =
        run_script(state, name).await?;
    Ok(actions
        .iter()
        .map(|(_weight, timeout, action)| {
            Tree::Leaf((action.clone(), *timeout))
        })
        .collect::<Vec<_>>())
}

fn back(state: &BrowserState) -> Tree<(BrowserActionCandidate, Timeout)> {
    if state.navigation_history.back.is_empty() {
        Tree::Branch(vec![])
    } else {
        Tree::Leaf((BrowserActionCandidate::Back, Timeout::from_secs(2)))
    }
}

pub async fn available_actions(
    origin: &Url,
    state: &BrowserState,
) -> Result<Tree<(BrowserActionCandidate, Timeout)>> {
    if state.content_type != "text/html"
        || !is_within_domain(&state.url, origin)
    {
        return Ok(back(state));
    }

    let tree = Tree::Branch(vec![
        (Tree::Branch(run_actions_script(state, "clicks").await?)),
        (Tree::Branch(run_actions_script(state, "inputs").await?)),
        (Tree::Branch(run_actions_script(state, "scrolls").await?)),
    ])
    .prune();
    log::debug!("action tree: {:?}", &tree);

    if let Some(tree) = tree {
        Ok(tree)
    } else {
        back(state)
            .prune()
            .ok_or(anyhow!("no fallback action available"))
    }
}
