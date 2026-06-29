use std::ops::RangeInclusive;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use bombadil::driver::FromGeneratedAction;
use bombadil::specification::generators::StringGenerator;
use bombadil_schema::browser::Fingerprint;
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
pub enum BrowserAction<U8 = u8, U16 = u16, U64 = u64, F64 = f64, Text = String>
{
    Back,
    Forward,
    Click {
        fingerprint: Fingerprint,
        point: Point<F64>,
    },
    DoubleClick {
        fingerprint: Fingerprint,
        point: Point<F64>,
        delay_millis: U64,
    },
    TypeText {
        text: Text,
        delay_millis: U64,
    },
    PressKey {
        code: u8,
    },
    ScrollUp {
        origin: Point<F64>,
        distance: F64,
    },
    ScrollDown {
        origin: Point<F64>,
        distance: F64,
    },
    Reload,
    Wait,
    SetFileInputFiles {
        selector: String,
        files: Vec<String>,
    },
    MouseDrag {
        from: Point<F64>,
        to: Point<F64>,
        steps: U8,
        delay_millis: U64,
    },
    SetViewport {
        width: U16,
        height: U16,
    },
    Custom {
        name: String,
        state: json::Value,
    },
}

pub type BrowserActionTemplate = BrowserAction<
    RangeInclusive<u8>,
    RangeInclusive<u16>,
    RangeInclusive<u64>,
    RangeInclusive<f64>,
    StringGenerator,
>;

impl FromGeneratedAction for BrowserActionTemplate {
    fn from_generated(value: json::Value) -> Result<Self> {
        let js_action: JsAction = json::from_value(value)?;
        js_action.try_into()
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
            BrowserAction::Click {
                point: position, ..
            } => {
                page.click((*position).into()).await?;
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
            BrowserAction::Custom { name, state } => {
                let state_stringified = json::to_string(state)?;

                page.evaluate(format!(r#"(async () => {{
                    const state = JSON.parse('{state_stringified}');
                    const action = __bombadilRequire('@antithesishq/bombadil').runtime.customActions["{name}"];
                    if (!action) {{
                        throw new Error("Custom action {name} not found");
                    }}
                    try {{
                        await action.run({{ ...state, document, window }});
                    }} catch (err) {{
                        throw new Error(`Error executing custom action {name}: ${{err}}`);
                    }}
                }})()"#)).await?;
            }
        };
        Ok(())
    }
}

impl BrowserActionTemplate {
    pub fn generate<Rng: rand::TryRng + rand::RngExt>(
        &self,
        rng: &mut Rng,
    ) -> BrowserAction {
        match self {
            BrowserAction::Back => BrowserAction::Back,
            BrowserAction::Forward => BrowserAction::Forward,
            BrowserAction::Click { fingerprint, point } => {
                BrowserAction::Click {
                    fingerprint: fingerprint.clone(),
                    point: point.generate(rng),
                }
            }
            BrowserAction::DoubleClick {
                fingerprint,
                point,
                delay_millis,
            } => BrowserAction::DoubleClick {
                fingerprint: fingerprint.clone(),
                point: point.generate(rng),
                delay_millis: rng.random_range(delay_millis.clone()),
            },
            BrowserAction::TypeText { text, delay_millis } => {
                BrowserAction::TypeText {
                    text: text.generate(rng),
                    delay_millis: rng.random_range(delay_millis.clone()),
                }
            }
            BrowserAction::PressKey { code } => {
                BrowserAction::PressKey { code: *code }
            }
            BrowserAction::ScrollUp { origin, distance } => {
                let distance = rng.random_range(distance.clone());
                BrowserAction::ScrollUp {
                    origin: origin.generate(rng),
                    distance,
                }
            }
            BrowserAction::ScrollDown { origin, distance } => {
                let distance = rng.random_range(distance.clone());
                BrowserAction::ScrollDown {
                    origin: origin.generate(rng),
                    distance,
                }
            }
            BrowserAction::Reload => BrowserAction::Reload,
            BrowserAction::Wait => BrowserAction::Wait,
            BrowserAction::SetFileInputFiles { selector, files } => {
                BrowserAction::SetFileInputFiles {
                    selector: selector.clone(),
                    files: files.clone(),
                }
            }
            BrowserAction::MouseDrag {
                from,
                to,
                steps,
                delay_millis,
            } => BrowserAction::MouseDrag {
                from: from.generate(rng),
                to: to.generate(rng),
                steps: rng.random_range(steps.clone()),
                delay_millis: rng.random_range(delay_millis.clone()),
            },
            BrowserAction::SetViewport { width, height } => {
                BrowserAction::SetViewport {
                    width: rng.random_range(width.clone()),
                    height: rng.random_range(height.clone()),
                }
            }
            BrowserAction::Custom { name, state } => BrowserAction::Custom {
                name: name.clone(),
                state: state.clone(),
            },
        }
    }

    pub fn accepts(&self, original: &BrowserAction) -> bool {
        match (self, original) {
            (BrowserAction::Back, BrowserAction::Back) => true,
            (BrowserAction::Forward, BrowserAction::Forward) => true,
            (
                BrowserAction::Click {
                    fingerprint: candidate_fingerprint,
                    ..
                },
                BrowserAction::Click {
                    fingerprint: original_fingerprint,
                    ..
                },
            ) => candidate_fingerprint.matches(original_fingerprint),
            (
                BrowserAction::DoubleClick {
                    fingerprint: candidate_fingerprint,
                    ..
                },
                BrowserAction::DoubleClick {
                    fingerprint: original_fingerprint,
                    ..
                },
            ) => candidate_fingerprint.matches(original_fingerprint),
            (
                BrowserAction::TypeText {
                    text: generator, ..
                },
                BrowserAction::TypeText { text: original, .. },
            ) => generator.accepts(original),
            (
                BrowserAction::PressKey {
                    code: code_candidate,
                },
                BrowserAction::PressKey {
                    code: code_original,
                },
            ) => code_candidate == code_original,
            (
                BrowserAction::ScrollUp {
                    origin: origin_candidate,
                    distance: distance_candidate,
                },
                BrowserAction::ScrollUp {
                    origin: origin_original,
                    distance: distance_original,
                },
            ) => {
                origin_candidate.accepts(origin_original)
                    && distance_candidate.contains(distance_original)
            }

            (
                BrowserAction::ScrollDown {
                    origin: origin_candidate,
                    distance: distance_candidate,
                },
                BrowserAction::ScrollDown {
                    origin: origin_original,
                    distance: distance_original,
                },
            ) => {
                origin_candidate.accepts(origin_original)
                    && distance_candidate.contains(distance_original)
            }
            (BrowserAction::Wait, BrowserAction::Wait) => true,
            (
                BrowserAction::SetFileInputFiles {
                    selector: candidate_selector,
                    ..
                },
                BrowserAction::SetFileInputFiles {
                    selector: original_selector,
                    ..
                },
            ) => candidate_selector == original_selector,
            _ => false,
        }
    }
}
