use std::hash::Hash;
use std::ops::RangeInclusive;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use bombadil::driver::{ActionTemplate, FromGeneratedAction};
use bombadil::specification::generators::StringGenerator;
use bombadil_schema::browser::Fingerprint;
use cdp_protocol::cdp::browser_protocol::target::SessionId;
use cdp_protocol::cdp::browser_protocol::{dom, emulation, input, page};
use cdp_protocol::cdp::js_protocol::runtime::{
    CallArgument, CallFunctionOnParamsBuilder,
};
use serde::{Deserialize, Serialize};
use serde_json as json;

use crate::browser::screenshots::FrameControl;
use crate::geometry::Point;
use crate::js_action::JsAction;
use bombadil_browser_keys::{key_name, key_text};

#[derive(Clone, Debug)]
pub struct ActionOptions {
    pub device_scale_factor: f64,
    pub frame_control: Option<FrameControl>,
}

/// Scroll with a single wheel event.
fn mouse_wheel(
    connection: &cdp::Connection,
    session_id: &SessionId,
    options: &ActionOptions,
    origin: Point,
    delta_y: f64,
) -> Result<()> {
    mouse_event(
        connection,
        session_id,
        options,
        input::DispatchMouseEventParams::builder()
            .r#type(input::DispatchMouseEventType::MouseWheel)
            .x(origin.x)
            .y(origin.y)
            .delta_x(0.0)
            .delta_y(delta_y)
            .build()
            .map_err(|err| anyhow!(err))?,
    )
}

/// Dispatch a mouse event. When we do explicit frame control we request new frames
/// until the event request is finished.
fn mouse_event(
    connection: &cdp::Connection,
    session_id: &SessionId,
    options: &ActionOptions,
    params: input::DispatchMouseEventParams,
) -> Result<()> {
    match &options.frame_control {
        Some(frame_control) => {
            let pending = connection.request(params, Some(session_id))?;
            loop {
                frame_control.frame()?;
                match pending
                    .wait_deadline(Instant::now() + Duration::from_millis(1))
                {
                    Ok(Some(_)) => {
                        break;
                    }
                    Ok(None) => continue, // timeout
                    Err(error) => {
                        bail!("mouse move response failed: {error}")
                    }
                }
            }
        }
        None => {
            connection.send(params, Some(session_id))?;
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[cfg_attr(test, derive(stdx_derive::VariantCount))]
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
                connection.send(
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
                connection.send(
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
                mouse_wheel(
                    connection, session_id, &options, *origin, -distance,
                )?;
            }
            BrowserAction::ScrollDown { origin, distance } => {
                mouse_wheel(
                    connection, session_id, &options, *origin, *distance,
                )?;
            }
            BrowserAction::Click { point, .. } => {
                let builder = input::DispatchMouseEventParams::builder()
                    .x(point.x)
                    .y(point.y)
                    .button(input::MouseButton::Left)
                    .click_count(1);
                mouse_event(
                    connection,
                    session_id,
                    &options,
                    input::DispatchMouseEventParams::new(
                        input::DispatchMouseEventType::MouseMoved,
                        point.x,
                        point.y,
                    ),
                )?;
                connection.send(
                    builder
                        .clone()
                        .r#type(input::DispatchMouseEventType::MousePressed)
                        .build()
                        .map_err(|err| anyhow!(err))?,
                    Some(session_id),
                )?;
                connection.send(
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
                mouse_event(
                    connection,
                    session_id,
                    &options,
                    input::DispatchMouseEventParams::new(
                        input::DispatchMouseEventType::MouseMoved,
                        point.x,
                        point.y,
                    ),
                )?;
                connection.send(
                    builder
                        .clone()
                        .r#type(input::DispatchMouseEventType::MousePressed)
                        .build()
                        .map_err(|err| anyhow!(err))?,
                    Some(session_id),
                )?;
                connection.send(
                    builder
                        .r#type(input::DispatchMouseEventType::MouseReleased)
                        .build()
                        .map_err(|err| anyhow!(err))?,
                    Some(session_id),
                )?;
            }
            // TODO: experiment, insert all text at once.
            BrowserAction::TypeText { text } => {
                connection.send(
                    input::InsertTextParams::new(text.as_str()),
                    Some(session_id),
                )?;
            }
            BrowserAction::PressKey { code } => {
                let Some(name) = key_name(*code) else {
                    bail!("unknown key with code: {:?}", code);
                };
                let text = key_text(*code);
                let build_params = |event_type, text: Option<&str>| {
                    let mut builder = input::DispatchKeyEventParams::builder()
                        .r#type(event_type)
                        .windows_virtual_key_code(*code as i64)
                        .code(name)
                        .key(name);
                    if let Some(text) = text {
                        builder = builder.unmodified_text(text).text(text);
                    }
                    builder.build().map_err(|err| anyhow!(err))
                };
                connection.send(
                    build_params(
                        input::DispatchKeyEventType::RawKeyDown,
                        None,
                    )?,
                    Some(session_id),
                )?;
                if let Some(text) = text {
                    connection.send(
                        build_params(
                            input::DispatchKeyEventType::Char,
                            Some(text),
                        )?,
                        Some(session_id),
                    )?;
                }
                connection.send(
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
                connection.send(
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
                connection.send(
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
                    mouse_event(
                        connection,
                        session_id,
                        &options,
                        dispatch(
                            input::DispatchMouseEventType::MouseMoved,
                            point,
                            1,
                        )?,
                    )?;
                }
                connection.send(
                    dispatch(
                        input::DispatchMouseEventType::MouseReleased,
                        *to,
                        0,
                    )?,
                    Some(session_id),
                )?;
            }
            BrowserAction::SetViewport { width, height } => {
                connection.send(
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
            BrowserAction::Custom { name, arguments } => {
                let call = CallFunctionOnParamsBuilder::default().function_declaration(
                    r#"async (name, args) => {
                        try {
                            await __bombadilRequire('@antithesishq/bombadil').runtime.runCustomAction(name, args);
                        } catch (err) {
                            throw new Error(`Error executing custom action ${JSON.stringify(name)}(${args.join(", ")}): ${err}`);
                        }
                    }"#
                )
                    .argument(CallArgument::builder().value(json::json!(name)).build())
                    .argument(CallArgument::builder().value(arguments.clone()).build())
                    .await_promise(true)
                    .return_by_value(true)
                    .unique_context_id(unique_context_id.ok_or(anyhow!("no unique_context_id available, can't apply custom action"))?)
                .build().map_err(|err| anyhow!(err))?;
                let result = connection.send(call, Some(session_id))?;
                if let Some(exception) = result.exception_details {
                    bail!("{}", exception)
                }
            }
        };
        Ok(())
    }
}

impl ActionTemplate<BrowserAction> for BrowserActionTemplate {
    fn generate<Rng: rand::TryRng + rand::RngExt>(
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
            BrowserAction::TypeText { text } => BrowserAction::TypeText {
                text: text.generate(rng),
            },
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

    fn accepts(&self, original: &BrowserAction) -> bool {
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
                BrowserAction::TypeText { text: generator },
                BrowserAction::TypeText { text: original },
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
            (BrowserAction::Reload, BrowserAction::Reload) => true,
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
            (
                BrowserAction::MouseDrag {
                    from,
                    to,
                    steps,
                    delay_millis,
                },
                BrowserAction::MouseDrag {
                    from: original_from,
                    to: original_to,
                    steps: original_steps,
                    delay_millis: original_delay,
                },
            ) => {
                from.accepts(original_from)
                    && to.accepts(original_to)
                    && steps.contains(original_steps)
                    && delay_millis.contains(original_delay)
            }
            (
                BrowserAction::SetViewport { width, height },
                BrowserAction::SetViewport {
                    width: original_width,
                    height: original_height,
                },
            ) => {
                width.contains(original_width)
                    && height.contains(original_height)
            }
            (
                BrowserAction::Custom { name, arguments },
                BrowserAction::Custom {
                    name: original_name,
                    arguments: original_arguments,
                },
            ) => name == original_name && arguments == original_arguments,
            _ => false,
        }
    }

    fn category_hash<H: std::hash::Hasher>(&self, hasher: &mut H) {
        match self {
            BrowserAction::Back => "Back".hash(hasher),
            BrowserAction::Forward => "Forward".hash(hasher),
            BrowserAction::Click { fingerprint, .. } => {
                ("Click", fingerprint).hash(hasher);
            }
            BrowserAction::DoubleClick { fingerprint, .. } => {
                ("DoubleClick", fingerprint).hash(hasher);
            }
            BrowserAction::TypeText { text } => {
                ("TypeText", text).hash(hasher);
            }
            BrowserAction::PressKey { code } => {
                ("PressKey", code).hash(hasher);
            }
            BrowserAction::ScrollUp { .. } => {
                "ScrollUp".hash(hasher);
            }
            BrowserAction::ScrollDown { .. } => {
                "ScrollDown".hash(hasher);
            }
            BrowserAction::Reload => {
                "Reload".hash(hasher);
            }
            BrowserAction::Wait => {
                "Wait".hash(hasher);
            }
            BrowserAction::SetFileInputFiles { selector, .. } => {
                ("SetFileInputFiles", selector).hash(hasher);
            }
            BrowserAction::MouseDrag { .. } => {
                "MouseDrag".hash(hasher);
            }
            BrowserAction::SetViewport { width, height } => {
                ("SetViewport", width, height).hash(hasher);
            }
            BrowserAction::Custom { name, .. } => {
                ("Custom", name).hash(hasher);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    use hegel::{
        Generator, TestCase,
        generators::{integers, text},
    };
    use rand::{SeedableRng, rngs::StdRng};

    fn small_range(tc: &TestCase, minimum: u8) -> RangeInclusive<u8> {
        let start = tc.draw(integers().min_value(minimum).max_value(100));
        let end = tc.draw(integers().min_value(start).max_value(100));
        start..=end
    }

    fn coordinate_range(tc: &TestCase) -> RangeInclusive<f64> {
        let range = small_range(tc, 0);
        f64::from(*range.start())..=f64::from(*range.end())
    }

    fn point(tc: &TestCase) -> Point<RangeInclusive<f64>> {
        Point {
            x: coordinate_range(tc),
            y: coordinate_range(tc),
        }
    }

    fn viewport_range(tc: &TestCase) -> RangeInclusive<u16> {
        let start = tc.draw(integers().min_value(1_u16).max_value(10_000));
        let end = tc.draw(integers().min_value(start).max_value(10_000));
        start..=end
    }

    #[hegel::composite]
    fn action_templates(tc: &TestCase) -> Vec<BrowserActionTemplate> {
        use BrowserAction::*;

        let fingerprint = Fingerprint {
            id: Some(format!(
                "id-{}",
                tc.draw(text().alphabet("abc").max_size(8))
            )),
            tag: "button".into(),
            ..Default::default()
        };
        let length = small_range(tc, 0);
        let drag_delay = small_range(tc, 0);

        vec![
            Back,
            Forward,
            Click {
                fingerprint: fingerprint.clone(),
                point: point(tc),
            },
            DoubleClick {
                fingerprint,
                point: point(tc),
            },
            TypeText {
                text: StringGenerator::Text {
                    length: u16::from(*length.start())
                        ..=u16::from(*length.end()),
                },
            },
            PressKey {
                code: tc.draw(integers().min_value(65).max_value(90)),
            },
            ScrollUp {
                origin: point(tc),
                distance: coordinate_range(tc),
            },
            ScrollDown {
                origin: point(tc),
                distance: coordinate_range(tc),
            },
            Reload,
            Wait,
            SetFileInputFiles {
                selector: format!(
                    "input#{}",
                    tc.draw(text().alphabet("abc").min_size(1).max_size(8))
                ),
                files: vec!["/tmp/example.txt".into()],
            },
            MouseDrag {
                from: point(tc),
                to: point(tc),
                steps: small_range(tc, 1),
                delay_millis: u64::from(*drag_delay.start())
                    ..=u64::from(*drag_delay.end()),
            },
            SetViewport {
                width: viewport_range(tc),
                height: viewport_range(tc),
            },
            Custom {
                name: format!(
                    "action-{}",
                    tc.draw(text().alphabet("abc").max_size(8))
                ),
                arguments: vec![json::json!(tc.draw(text().max_size(16)))],
            },
        ]
    }

    // Replay invariant: for every valid template T and RNG state r,
    // T.accepts(&T.generate(r)) == true.
    #[hegel::test]
    fn templates_accept_their_generated_actions(tc: TestCase) {
        let seed = tc.draw(integers());
        let templates = tc.draw(action_templates().print_as_debug());
        let variants: HashSet<_> =
            templates.iter().map(std::mem::discriminant).collect();
        assert_eq!(
            variants.len(),
            BrowserActionTemplate::VARIANT_COUNT,
            "template generator must cover every BrowserAction variant"
        );
        for template in templates {
            let mut rng = StdRng::seed_from_u64(seed);
            let action = template.generate(&mut rng);
            assert!(
                template.accepts(&action),
                "template {template:?} rejected {action:?} with seed {seed}"
            );
        }
    }
}
