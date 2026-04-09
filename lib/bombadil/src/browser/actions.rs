use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use chromiumoxide::Page;
use chromiumoxide::cdp::browser_protocol::{dom, input, page};
use serde::{Deserialize, Serialize};
use tokio::time::sleep;

use crate::geometry::Point;
use bombadil_browser_keys::key_name;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum BrowserAction {
    Back,
    Forward,
    Click {
        name: String,
        content: Option<String>,
        point: Point,
    },
    DoubleClick {
        name: String,
        content: Option<String>,
        point: Point,
        delay_millis: u64,
    },
    TypeText {
        text: String,
        delay_millis: u64,
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
    Wait,
    SetFileInputFiles {
        selector: String,
        files: Vec<String>,
    },
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
            BrowserAction::Forward => {
                let history =
                    page.execute(page::GetNavigationHistoryParams {}).await?;
                let next_index = (history.current_index + 1) as usize;
                if next_index >= history.entries.len() {
                    bail!("can't go forward from last navigation entry");
                }
                let next: page::NavigationEntry =
                    history.entries[next_index].clone();
                page.execute(
                    page::NavigateToHistoryEntryParams::builder()
                        .entry_id(next.id)
                        .build()
                        .map_err(|err| anyhow!(err))?,
                )
                .await?;
            }
            BrowserAction::Reload => {
                page.reload().await?;
            }
            BrowserAction::Wait => {}
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
            BrowserAction::DoubleClick {
                point,
                delay_millis,
                ..
            } => {
                page.click((*point).into()).await?;
                sleep(Duration::from_millis(*delay_millis)).await;
                page.click((*point).into()).await?;
            }
            BrowserAction::TypeText { text, delay_millis } => {
                let delay = Duration::from_millis(*delay_millis);
                for char in text.chars() {
                    sleep(delay).await;
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
            BrowserAction::SetFileInputFiles { selector, files } => {
                let document =
                    page.execute(dom::GetDocumentParams::default()).await?;
                let node = page
                    .execute(
                        dom::QuerySelectorParams::builder()
                            .node_id(document.root.node_id)
                            .selector(selector)
                            .build()
                            .map_err(|err| anyhow!(err))?,
                    )
                    .await?;
                if node.node_id.inner() == &0 {
                    bail!("element not found for selector: {:?}", selector);
                }
                page.execute(
                    dom::SetFileInputFilesParams::builder()
                        .files(files.clone())
                        .node_id(node.node_id)
                        .build()
                        .map_err(|err| anyhow!(err))?,
                )
                .await?;
            }
        };
        Ok(())
    }

    pub fn to_api(&self) -> bombadil_schema::BrowserAction {
        match self {
            BrowserAction::Back => bombadil_schema::BrowserAction::Back,
            BrowserAction::Forward => bombadil_schema::BrowserAction::Forward,
            BrowserAction::Click {
                name,
                content,
                point,
            } => bombadil_schema::BrowserAction::Click {
                name: name.clone(),
                content: content.clone(),
                point: point.to_api(),
            },
            BrowserAction::DoubleClick {
                name,
                content,
                point,
                delay_millis,
            } => bombadil_schema::BrowserAction::DoubleClick {
                name: name.clone(),
                content: content.clone(),
                point: point.to_api(),
                delay_millis: *delay_millis,
            },
            BrowserAction::TypeText { text, delay_millis } => {
                bombadil_schema::BrowserAction::TypeText {
                    text: text.clone(),
                    delay_millis: *delay_millis,
                }
            }
            BrowserAction::PressKey { code } => {
                bombadil_schema::BrowserAction::PressKey { code: *code }
            }
            BrowserAction::ScrollUp { origin, distance } => {
                bombadil_schema::BrowserAction::ScrollUp {
                    origin: origin.to_api(),
                    distance: *distance,
                }
            }
            BrowserAction::ScrollDown { origin, distance } => {
                bombadil_schema::BrowserAction::ScrollDown {
                    origin: origin.to_api(),
                    distance: *distance,
                }
            }
            BrowserAction::Reload => bombadil_schema::BrowserAction::Reload,
            BrowserAction::Wait => bombadil_schema::BrowserAction::Wait,
            BrowserAction::SetFileInputFiles { selector, files } => {
                bombadil_schema::BrowserAction::SetFileInputFiles {
                    selector: selector.clone(),
                    files: files.clone(),
                }
            }
        }
    }
}
