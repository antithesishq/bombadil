use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use bombadil::driver::FromGeneratedAction;
use chromiumoxide::Page;
use chromiumoxide::cdp::browser_protocol::{dom, emulation, input, page};
use serde::{Deserialize, Serialize};
use serde_json as json;
use tokio::time::sleep;

use crate::geometry::Point;
use crate::js_action::JsAction;
use bombadil_browser_keys::{key_name, key_text};

#[derive(Clone, Copy, Debug)]
pub struct ActionOptions {
    pub device_scale_factor: f64,
}

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
    MouseDrag {
        from: Point,
        to: Point,
        steps: u8,
        delay_millis: u64,
    },
    SetViewport {
        width: u16,
        height: u16,
    },
}

impl FromGeneratedAction for BrowserAction {
    fn from_generated(value: json::Value) -> Result<Self> {
        let js_action: JsAction = json::from_value(value)?;
        js_action.into_browser_action()
    }
}

impl BrowserAction {
    pub async fn apply(
        &self,
        page: &Page,
        options: ActionOptions,
    ) -> Result<()> {
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
                let Some(name) = key_name(*code) else {
                    bail!("unknown key with code: {:?}", code);
                };
                let text = key_text(*code);
                let build_params = |event_type, text: Option<&str>| {
                    let mut builder = input::DispatchKeyEventParams::builder()
                        .r#type(event_type)
                        .native_virtual_key_code(*code as i64)
                        .windows_virtual_key_code(*code as i64)
                        .code(name)
                        .key(name);
                    if let Some(text) = text {
                        builder = builder.unmodified_text(text).text(text);
                    }
                    builder.build().map_err(|err| anyhow!(err))
                };
                page.execute(build_params(
                    input::DispatchKeyEventType::RawKeyDown,
                    None,
                )?)
                .await?;
                if let Some(text) = text {
                    page.execute(build_params(
                        input::DispatchKeyEventType::Char,
                        Some(text),
                    )?)
                    .await?;
                }
                page.execute(build_params(
                    input::DispatchKeyEventType::KeyUp,
                    None,
                )?)
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
            BrowserAction::MouseDrag {
                from,
                to,
                steps,
                delay_millis,
            } => {
                // `buttons: 1` (left held) must be set on every event during
                // the drag so JS sees the held button on mousemove. Chrome
                // doesn't track button state across CDP events.
                let dispatch = |event_type, point: Point, buttons: i64| {
                    input::DispatchMouseEventParams::builder()
                        .r#type(event_type)
                        .x(point.x)
                        .y(point.y)
                        .button(input::MouseButton::Left)
                        .buttons(buttons)
                        .click_count(1)
                        .build()
                        .map_err(|err| anyhow!(err))
                };
                page.execute(dispatch(
                    input::DispatchMouseEventType::MousePressed,
                    *from,
                    1,
                )?)
                .await?;
                let delay = Duration::from_millis(*delay_millis);
                let steps = (*steps).max(1);
                for step in 1..=steps {
                    let progress = step as f64 / steps as f64;
                    let point = Point {
                        x: from.x + (to.x - from.x) * progress,
                        y: from.y + (to.y - from.y) * progress,
                    };
                    if !delay.is_zero() {
                        sleep(delay).await;
                    }
                    page.execute(dispatch(
                        input::DispatchMouseEventType::MouseMoved,
                        point,
                        1,
                    )?)
                    .await?;
                }
                page.execute(dispatch(
                    input::DispatchMouseEventType::MouseReleased,
                    *to,
                    0,
                )?)
                .await?;
            }
            BrowserAction::SetViewport { width, height } => {
                page.execute(
                    emulation::SetDeviceMetricsOverrideParams::builder()
                        .width(u32::from(*width))
                        .height(u32::from(*height))
                        .device_scale_factor(options.device_scale_factor)
                        .mobile(false)
                        .scale(1)
                        .build()
                        .map_err(|err| anyhow!(err))?,
                )
                .await?;
            }
        };
        Ok(())
    }
}
