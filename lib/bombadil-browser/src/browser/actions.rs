use std::ops::RangeInclusive;
use std::thread;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use bombadil::driver::FromGeneratedAction;
use bombadil::specification::generators::StringGenerator;
use bombadil_schema::browser::Fingerprint;
use cdp_protocol::cdp::browser_protocol::target::SessionId;
use cdp_protocol::cdp::browser_protocol::{dom, emulation, input, page};
use cdp_protocol::cdp::js_protocol::runtime::{
    CallArgument, CallFunctionOnParamsBuilder,
};
use serde::{Deserialize, Serialize};
use serde_json as json;

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
        arguments: Vec<json::Value>,
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
    #[hotpath::measure]
    pub fn apply(
        &self,
        connection: &cdp::Connection,
        session_id: &SessionId,
        unique_context_id: Option<String>,
        options: ActionOptions,
    ) -> Result<()> {
        match self {
            BrowserAction::Back => {
                let history = connection.send(
                    page::GetNavigationHistoryParams {},
                    Some(session_id),
                )?;
                if history.current_index == 0 {
                    bail!("can't go back from first navigation entry");
                }
                let last: page::NavigationEntry = history.entries
                    [(history.current_index - 1) as usize]
                    .clone();
                connection.post(
                    page::NavigateToHistoryEntryParams::builder()
                        .entry_id(last.id)
                        .build()
                        .map_err(|err| anyhow!(err))?,
                    Some(session_id),
                )?;
            }
            BrowserAction::Forward => {
                let history = connection.send(
                    page::GetNavigationHistoryParams {},
                    Some(session_id),
                )?;
                let next_index = (history.current_index + 1) as usize;
                if next_index >= history.entries.len() {
                    bail!("can't go forward from last navigation entry");
                }
                let next: page::NavigationEntry =
                    history.entries[next_index].clone();
                connection.post(
                    page::NavigateToHistoryEntryParams::builder()
                        .entry_id(next.id)
                        .build()
                        .map_err(|err| anyhow!(err))?,
                    Some(session_id),
                )?;
            }
            BrowserAction::Reload => {
                connection
                    .post(page::ReloadParams::default(), Some(session_id))?;
            }
            BrowserAction::Wait => {}
            BrowserAction::ScrollUp { origin, distance } => {
                connection.post(
                    input::SynthesizeScrollGestureParams::builder()
                        .x(origin.x)
                        .y(origin.y)
                        .y_distance(*distance)
                        .speed((distance.abs() * 10.0) as i64)
                        .build()
                        .map_err(|err| anyhow!(err))?,
                    Some(session_id),
                )?;
            }
            BrowserAction::ScrollDown { origin, distance } => {
                connection.post(
                    input::SynthesizeScrollGestureParams::builder()
                        .x(origin.x)
                        .y(origin.y)
                        .y_distance(-distance)
                        .speed((distance.abs() * 10.0) as i64)
                        .build()
                        .map_err(|err| anyhow!(err))?,
                    Some(session_id),
                )?;
            }
            BrowserAction::Click { point, .. } => {
                let builder = input::DispatchMouseEventParams::builder()
                    .x(point.x)
                    .y(point.y)
                    .button(input::MouseButton::Left)
                    .click_count(1);
                connection.post(
                    input::DispatchMouseEventParams::new(
                        input::DispatchMouseEventType::MouseMoved,
                        point.x,
                        point.y,
                    ),
                    Some(session_id),
                )?;
                connection.post(
                    builder
                        .clone()
                        .r#type(input::DispatchMouseEventType::MousePressed)
                        .build()
                        .map_err(|err| anyhow!(err))?,
                    Some(session_id),
                )?;
                connection.post(
                    builder
                        .r#type(input::DispatchMouseEventType::MouseReleased)
                        .build()
                        .map_err(|err| anyhow!(err))?,
                    Some(session_id),
                )?;
            }
            BrowserAction::DoubleClick {
                point,
                fingerprint: _,
            } => {
                let builder = input::DispatchMouseEventParams::builder()
                    .x(point.x)
                    .y(point.y)
                    .button(input::MouseButton::Left)
                    .click_count(2);
                connection.send(
                    input::DispatchMouseEventParams::new(
                        input::DispatchMouseEventType::MouseMoved,
                        point.x,
                        point.y,
                    ),
                    Some(session_id),
                )?;
                connection.post(
                    builder
                        .clone()
                        .r#type(input::DispatchMouseEventType::MousePressed)
                        .build()
                        .map_err(|err| anyhow!(err))?,
                    Some(session_id),
                )?;
                connection.post(
                    builder
                        .r#type(input::DispatchMouseEventType::MouseReleased)
                        .build()
                        .map_err(|err| anyhow!(err))?,
                    Some(session_id),
                )?;
            }
            BrowserAction::TypeText { text, delay_millis } => {
                let delay = Duration::from_millis(*delay_millis);
                for char in text.chars() {
                    thread::sleep(delay);
                    connection.post(
                        input::InsertTextParams::new(char),
                        Some(session_id),
                    )?;
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
                connection.post(
                    build_params(
                        input::DispatchKeyEventType::RawKeyDown,
                        None,
                    )?,
                    Some(session_id),
                )?;
                if let Some(text) = text {
                    connection.post(
                        build_params(
                            input::DispatchKeyEventType::Char,
                            Some(text),
                        )?,
                        Some(session_id),
                    )?;
                }
                connection.post(
                    build_params(input::DispatchKeyEventType::KeyUp, None)?,
                    Some(session_id),
                )?;
            }
            BrowserAction::SetFileInputFiles { selector, files } => {
                let document = connection.send(
                    dom::GetDocumentParams::default(),
                    Some(session_id),
                )?;
                let node = connection.send(
                    dom::QuerySelectorParams::builder()
                        .node_id(document.root.node_id)
                        .selector(selector)
                        .build()
                        .map_err(|err| anyhow!(err))?,
                    Some(session_id),
                )?;
                if node.node_id.inner() == &0 {
                    bail!("element not found for selector: {:?}", selector);
                }
                connection.post(
                    dom::SetFileInputFilesParams::builder()
                        .files(files.clone())
                        .node_id(node.node_id)
                        .build()
                        .map_err(|err| anyhow!(err))?,
                    Some(session_id),
                )?;
            }
            BrowserAction::MouseDrag {
                from,
                to,
                steps,
                delay_millis,
            } => {
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
                connection.post(
                    dispatch(
                        input::DispatchMouseEventType::MousePressed,
                        *from,
                        1,
                    )?,
                    Some(session_id),
                )?;
                let delay = Duration::from_millis(*delay_millis);
                let steps = (*steps).max(1);
                for step in 1..=steps {
                    let progress = step as f64 / steps as f64;
                    let point = Point {
                        x: from.x + (to.x - from.x) * progress,
                        y: from.y + (to.y - from.y) * progress,
                    };
                    if !delay.is_zero() {
                        thread::sleep(delay);
                    }
                    connection.post(
                        dispatch(
                            input::DispatchMouseEventType::MouseMoved,
                            point,
                            1,
                        )?,
                        Some(session_id),
                    )?;
                }
                connection.post(
                    dispatch(
                        input::DispatchMouseEventType::MouseReleased,
                        *to,
                        0,
                    )?,
                    Some(session_id),
                )?;
            }
            BrowserAction::SetViewport { width, height } => {
                connection.post(
                    emulation::SetDeviceMetricsOverrideParams::builder()
                        .width(u32::from(*width))
                        .height(u32::from(*height))
                        .device_scale_factor(options.device_scale_factor)
                        .mobile(false)
                        .scale(1)
                        .build()
                        .map_err(|err| anyhow!(err))?,
                    Some(session_id),
                )?;
            }
            BrowserAction::Custom {
                name,
                arguments: options,
            } => {
                let call = CallFunctionOnParamsBuilder::default().function_declaration(
                    r#"async (name, options) => {
                        try {
                            await __bombadilRequire('@antithesishq/bombadil').runtime.runCustomAction(name, options);
                        } catch (err) {
                            throw new Error(`Error executing custom action ${JSON.stringify(name)}: ${err}`);
                        }
                    }"#
                )
                    .argument(CallArgument::builder().value(json::json!(name)).build())
                    .argument(CallArgument::builder().value(options.clone()).build())
                    .unique_context_id(unique_context_id.ok_or(anyhow!("no unique_context_id available, can't apply custom action"))?)
                .build().map_err(|err| anyhow!(err))?;
                connection.send(call, Some(session_id))?;
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
            BrowserAction::DoubleClick { fingerprint, point } => {
                BrowserAction::DoubleClick {
                    fingerprint: fingerprint.clone(),
                    point: point.generate(rng),
                }
            }
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
            BrowserAction::Custom {
                name,
                arguments: options,
            } => BrowserAction::Custom {
                name: name.clone(),
                arguments: options.clone(),
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
