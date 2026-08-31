use anyhow::ensure;
use anyhow::{Result, anyhow, bail};
use cdp::types::try_match;
use cdp_protocol::cdp::browser_protocol::css;
use cdp_protocol::cdp::browser_protocol::emulation;
use cdp_protocol::cdp::browser_protocol::network;
use cdp_protocol::cdp::browser_protocol::page::{
    self, ClientNavigationReason, FrameId, NavigationType,
};
use cdp_protocol::cdp::browser_protocol::target::{self, SessionId, TargetId};
use cdp_protocol::cdp::browser_protocol::{browser, dom};
use cdp_protocol::cdp::js_protocol::debugger::{self, CallFrameId};
use cdp_protocol::cdp::js_protocol::runtime::{self};
use crossbeam_channel as mpmc;
use log;
use serde_json as json;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, UNIX_EPOCH};
use tempfile::TempDir;
use url::Url;

use crate::browser::actions::{ActionOptions, BrowserAction};
use crate::browser::state::Generation;
use crate::browser::state::{
    BrowserState, CallFrame, ConsoleEntry, Exception, Screenshot,
    ScreenshotFormat,
};
use crate::cookie::{BrowserCookie, build_cookie_param};

pub mod actions;
pub mod activity;
pub mod evaluation;
pub mod instrumentation;
pub mod quiescence;
pub mod state;

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum BrowserEvent {
    StateChanged(BrowserState),
    Error(Arc<anyhow::Error>),
}

#[derive(Debug, Default)]
struct InnerStateShared {
    generation: Generation,
    console_entries: Vec<ConsoleEntry>,
    exceptions: Vec<Exception>,
    screenshot: Option<Screenshot>,
    execution_context_id: Option<String>,
}

#[derive(Debug)]
struct InnerState {
    kind: InnerStateKind,
    shared: InnerStateShared,
}

enum InnerStateKind {
    Pausing,
    Paused,
    Resuming(Box<BrowserAction>),
    Navigating { url: String },
    Loading,
    Running(quiescence::QuiescenceTimer),
    Acting(quiescence::QuiescenceSubscription),
}

impl std::fmt::Debug for InnerStateKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pausing => write!(f, "Pausing"),
            Self::Paused => write!(f, "Paused"),
            Self::Resuming(action) => {
                f.debug_tuple("Resuming").field(action).finish()
            }
            Self::Navigating { url } => {
                f.debug_struct("Navigating").field("url", url).finish()
            }
            Self::Loading => write!(f, "Loading"),
            Self::Running(_) => write!(f, "Running"),
            Self::Acting(_) => write!(f, "Acting"),
        }
    }
}

#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
enum InnerEvent {
    StateRequested(StateRequestReason, Generation),
    Loaded,
    Paused {
        reason: debugger::PausedReason,
        exception: Option<json::Value>,
        call_frame_id: Option<CallFrameId>,
    },
    Resumed,
    FrameRequestedNavigation {
        frame_id: FrameId,
        reason: ClientNavigationReason,
        url: String,
    },
    FrameNavigated(FrameId, NavigationType),
    DownloadWillBegin {
        frame_id: FrameId,
        url: String,
    },
    ExecutionContextCreated(String),
    ExecutionContextDestroyed(String),
    TargetDestroyed(TargetId),
    ConsoleEntry(ConsoleEntry),
    ActionAccepted(BrowserAction, Generation),
    ActionApplied(Generation),
    ExceptionThrown(Exception),
    Quiesced(Generation),
    NavigationTimedOut(Generation),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum StateRequestReason {
    Start,
    Quiesced,
}

/// Initial idle timeout before the first activity signal arrives.
/// Deliberately long so we don't fire before the browser has produced
/// any frames; the first activity event will replace this with a much shorter
/// deadline.
const QUIESCENCE_INITIAL_IDLE: Duration = Duration::from_millis(250);
const QUIESCENCE_TIMEOUT: Duration = Duration::from_secs(10);
const NAVIGATION_TIMEOUT: Duration = Duration::from_secs(30);

struct BrowserContext {
    sender: mpmc::Sender<BrowserEvent>,
    events_tx: mpmc::Sender<InnerEvent>,
    connection: cdp::Connection,
    target_id: TargetId,
    frame_id: FrameId,
    session_id: SessionId,
    latest_frame: Arc<Mutex<Option<Arc<[u8]>>>>,
    #[allow(unused, reason = "this is going into the scripts soon")]
    origin: Url,
    browser_options: BrowserOptions,
}

#[derive(Clone)]
pub struct LaunchOptions {
    pub headless: bool,
    pub user_data_directory: PathBuf,
    pub no_sandbox: bool,
}

#[derive(Clone)]
pub struct Emulation {
    pub width: u16,
    pub height: u16,
    pub device_scale_factor: f64,
}

#[derive(Clone)]
pub struct BrowserOptions {
    pub emulation: Emulation,
    pub create_target: bool,
    pub instrumentation: crate::instrumentation::InstrumentationConfig,
    pub downloads_directory: PathBuf,
    pub grant_permissions: Vec<String>,
    pub extra_headers: HashMap<String, String>,
    pub cookies: Vec<BrowserCookie>,
}

#[derive(Clone)]
pub enum DebuggerOptions {
    External { remote_debugger: Url },
    Managed { launch_options: LaunchOptions },
}

pub struct Browser {
    browser_events_rx: mpmc::Receiver<BrowserEvent>,
    events_tx: mpmc::Sender<InnerEvent>,
    done_receiver: Option<mpmc::Receiver<()>>,
    connection: cdp::Connection,
    session_id: SessionId,
    frame_id: FrameId,
    origin: Url,
    go_to_origin_on_init: bool,
}

impl Drop for Browser {
    fn drop(&mut self) {
        let _ = self.connection.close();
    }
}

impl Browser {
    pub fn new(
        origin: Url,
        browser_options: BrowserOptions,
        debugger_options: DebuggerOptions,
    ) -> Result<Self> {
        let connection = match debugger_options {
            DebuggerOptions::External {
                ref remote_debugger,
            } => cdp::Connection::connect(remote_debugger.as_str())?,
            DebuggerOptions::Managed { ref launch_options } => {
                todo!("managed browser unsupported for now");
                // let browser_config = launch_options_to_config(
                //     launch_options,
                //     &browser_options.emulation,
                // )?;
                // chromiumoxide::Browser::launch(browser_config)?
            }
        };

        let (events_tx, events_rx) = mpmc::bounded(1024);
        let (browser_events_tx, browser_events_rx) =
            mpmc::bounded::<BrowserEvent>(1);

        let (target_id, session_id) = if browser_options.create_target {
            let target_id = connection
                .send(target::CreateTargetParams::default(), None)?
                .target_id;

            let session_id = connection
                .send(
                    target::AttachToTargetParams {
                        target_id: target_id.clone(),
                        flatten: Some(true),
                    },
                    None,
                )?
                .session_id;

            (target_id, session_id)
        } else {
            find_page(&connection)?
        };

        connection.send(dom::EnableParams::default(), Some(&session_id))?;
        connection.send(css::EnableParams::default(), Some(&session_id))?;
        connection.send(runtime::EnableParams::default(), Some(&session_id))?;
        connection
            .send(debugger::EnableParams::default(), Some(&session_id))?;
        connection.send(network::EnableParams::default(), Some(&session_id))?;

        if !browser_options.extra_headers.is_empty() {
            connection.send(
                network::SetExtraHttpHeadersParams::new(network::Headers::new(
                    json::to_value(&browser_options.extra_headers)?,
                )),
                Some(&session_id),
            )?;
        }

        if !browser_options.cookies.is_empty() {
            // Unlike a static Cookie request header, these become real browser
            // cookies and are sent on every navigation, which is what client-side
            // auth flows (e.g. MSAL) rely on. Plain NAME=VALUE scopes to the
            // origin URL; Set-Cookie attributes (Domain, Path, etc.) override that.
            let cookies = browser_options
                .cookies
                .iter()
                .map(|cookie| build_cookie_param(cookie, &origin))
                .collect::<Result<Vec<_>>>()?;
            connection.send(
                network::SetCookiesParams::new(cookies),
                Some(&session_id),
            )?;
        }

        // Prevent file downloads to avoid getting stuck
        connection.send(
            browser::SetDownloadBehaviorParams::builder()
                .behavior(browser::SetDownloadBehaviorBehavior::AllowAndName)
                .events_enabled(true)
                .download_path(
                    browser_options.downloads_directory.to_string_lossy(),
                )
                .build()
                .map_err(|s| {
                    anyhow!(s).context("build SetDownloadBehaviorParams failed")
                })?,
            Some(&session_id),
        )?;

        for permission in &browser_options.grant_permissions {
            connection.send(
                browser::SetPermissionParams::builder()
                    .permission(browser::PermissionDescriptor::new(permission))
                    .setting(browser::PermissionSetting::Granted)
                    .build()
                    .map_err(|s| {
                        anyhow!(s).context("build SetPermissionParams failed")
                    })?,
                Some(&session_id),
            )?;
        }

        connection.send(
            emulation::SetDeviceMetricsOverrideParams::builder()
                .width(browser_options.emulation.width)
                .height(browser_options.emulation.height)
                .device_scale_factor(
                    browser_options.emulation.device_scale_factor,
                )
                .mobile(false)
                .scale(1)
                .build()
                .map_err(|err| {
                    anyhow!(err)
                        .context("build SetDeviceMetricsOverrideParams failed")
                })?,
            Some(&session_id),
        )?;

        auto_accept_dialogs(connection.clone(), &session_id)?;

        let (done_sender, done_receiver) = mpmc::bounded::<()>(1);

        let frame_id = connection
            .send(page::GetFrameTreeParams::default(), Some(&session_id))?
            .frame_tree
            .frame
            .id;

        let screencast = activity::screencast_start(
            &connection,
            &session_id,
            browser_options.emulation.width,
            browser_options.emulation.height,
        )?;

        let latest_frame: Arc<Mutex<Option<Arc<[u8]>>>> =
            Arc::new(Mutex::new(None));

        // Background task to keep the latest screencast frame updated.
        {
            let latest_frame = latest_frame.clone();
            thread::spawn(move || {
                while let Ok(frame) = screencast.recv() {
                    *latest_frame.lock().unwrap() = Some(frame);
                }
            });
        }

        let context = BrowserContext {
            sender: browser_events_tx,
            events_tx: events_tx.clone(),
            connection: connection.clone(),
            target_id,
            frame_id: frame_id.clone(),
            session_id: session_id.clone(),
            latest_frame,
            origin: origin.clone(),
            browser_options: browser_options.clone(),
        };

        instrumentation::instrument_js_coverage(
            connection.clone(),
            &session_id,
            browser_options.instrumentation.clone(),
        )?;

        forward_inner_events(&context, events_tx.clone())?;
        run_state_machine(context, events_rx, done_sender);

        Ok(Browser {
            browser_events_rx,
            events_tx,
            done_receiver: Some(done_receiver),
            connection,
            session_id,
            frame_id,
            origin,
            go_to_origin_on_init: browser_options.create_target,
        })
    }

    pub async fn initiate(&mut self) -> Result<()> {
        if self.go_to_origin_on_init {
            let connection = self.connection.clone();
            let session_id = self.session_id.clone();
            let frame_id = self.frame_id.clone();
            let origin = self.origin.to_string();
            thread::spawn(move || {
                log::info!("going to origin");
                let _ = connection.send(
                    page::NavigateParams {
                        url: origin,
                        referrer: None,
                        transition_type: None,
                        frame_id: Some(frame_id),
                        referrer_policy: None,
                    },
                    Some(&session_id),
                );
            });
        } else {
            let _ = self.events_tx.send(InnerEvent::StateRequested(
                StateRequestReason::Start,
                Generation::default(),
            ));
            log::debug!(
                "using externally managed debugger, not doing anything on init"
            )
        }
        Ok(())
    }

    pub async fn terminate(mut self) -> Result<()> {
        // Close the browser before waiting for the state machine. Any CDP calls
        // in-flight inside process_event will fail once the connection drops,
        // unblocking the state machine so it can exit. Without this ordering,
        // terminate() could deadlock: the state machine waits for a CDP response
        // and the browser never closes because we're waiting for the state machine.
        let _ = self.connection.close();

        // Wait for the state machine to confirm it has exited. The done signal
        // is always sent now (even on error), so this should resolve promptly.
        if let Some(done_receiver) = self.done_receiver.take() {
            let _ = done_receiver.recv();
        }

        Ok(())
    }

    pub async fn next_event(&mut self) -> Option<BrowserEvent> {
        self.browser_events_rx.recv().ok()
    }

    pub fn apply(
        &mut self,
        action: BrowserAction,
        state: Arc<BrowserState>,
    ) -> Result<()> {
        self.events_tx
            .send(InnerEvent::ActionAccepted(action, state.generation))?;
        Ok(())
    }

    pub fn origin(&self) -> &Url {
        &self.origin
    }

    pub async fn ensure_script_evaluated(&self, script: &str) -> Result<()> {
        self.connection.send(
            page::AddScriptToEvaluateOnNewDocumentParams {
                source: script.into(),
                world_name: None,
                include_command_line_api: Some(false),
                run_immediately: Some(true),
            },
            Some(&self.session_id),
        )?;
        Ok(())
    }
}

/// Auto-accept JavaScript dialogs (alert, confirm, prompt, beforeunload)
/// so they never block the test run.
fn auto_accept_dialogs(
    connection: cdp::Connection,
    session_id: &SessionId,
) -> Result<()> {
    let events = connection
        .events
        .subscribe::<page::EventJavascriptDialogOpening>();
    let session_id = session_id.clone();
    thread::spawn(move || -> Result<()> {
        while let Some(event) = events.next()? {
            log::debug!(
                "auto-accepting JavaScript dialog: \
                 type={:?} message={:?}",
                event.r#type,
                event.message
            );
            let _ = connection.send(
                page::HandleJavaScriptDialogParams::builder()
                    .accept(true)
                    .build()
                    .expect("build HandleJavaScriptDialogParams"),
                Some(&session_id),
            );
        }
        Ok(())
    });
    Ok(())
}

fn forward_inner_events(
    context: &BrowserContext,
    events_tx: mpmc::Sender<InnerEvent>,
) -> Result<()> {
    let frame_id = context.frame_id.clone();
    let all = context.connection.events.all();

    let _ = thread::spawn(move || {
        while let Ok(event) = all.try_recv() {
            let inner_event = try_match!(event, {
                runtime::EventExecutionContextCreated: event => {
                    // TODO: filter these, ignoring some?
                    log::debug!("got execution context created event: {:?}", event.context);
                    Some(InnerEvent::ExecutionContextCreated(event.context.unique_id.clone()))
                },
                runtime::EventExecutionContextDestroyed: event => {
                    Some(InnerEvent::ExecutionContextDestroyed(event.execution_context_unique_id.clone()))
                },
                target::EventTargetDestroyed: event => {
                    Some(InnerEvent::TargetDestroyed(event.target_id.clone()))
                },
                page::EventLoadEventFired => {
                    Some(InnerEvent::Loaded)
                },
                debugger::EventPaused: event => {
                    Some(InnerEvent::Paused {
                        reason: event.reason.clone(),
                        exception: event.data.clone(),
                        call_frame_id: event
                            .call_frames
                            .first()
                            .map(|f| f.call_frame_id.clone()),
                    })
                },
                debugger::EventResumed => {
                    Some(InnerEvent::Resumed)
                },
                runtime::EventExceptionThrown: e => {
                    Some(InnerEvent::ExceptionThrown(Exception {
                        exception_id: e.exception_details.exception_id as u32,
                        timestamp: UNIX_EPOCH
                            + Duration::from_secs_f64(
                                *e.timestamp.inner() / 1000.0,
                            ),
                            text: e.exception_details.text.clone(),
                            line: e.exception_details.line_number as u32,
                            column: e.exception_details.column_number as u32,
                            url: e.exception_details.url.clone(),
                            remote_object: e.exception_details.exception.as_ref().map(
                                |obj| state::ExceptionRemoteObject {
                                    type_name: format!("{:?}", obj.r#type),
                                    subtype: obj
                                        .subtype
                                        .as_ref()
                                        .map(|st| format!("{:?}", st)),
                                        class_name: obj.class_name.clone(),
                                        description: obj.description.clone(),
                                        value: obj.value.clone(),
                                },
                            ),
                            stacktrace: e.exception_details.stack_trace.as_ref().map(
                                |stack_trace| {
                                    stack_trace
                                        .call_frames
                                        .iter()
                                        .map(|frame| CallFrame {
                                            name: frame.function_name.clone(),
                                            line: frame.line_number as u32,
                                            column: frame.column_number as u32,
                                            url: frame.url.clone(),
                                        })
                                    .collect()
                                },
                            ),
                    }))
                },
                page::EventFrameRequestedNavigation: nav => {
                    if nav.frame_id == frame_id {
                        Some(InnerEvent::FrameRequestedNavigation {
                            frame_id: nav.frame_id.clone(),
                            reason: nav.reason.clone(),
                            url: nav.url.clone(),
                        })
                    } else { None }
                },
                page::EventFrameNavigated: nav => {
                    if nav.frame.id == frame_id {
                        Some (InnerEvent::FrameNavigated(
                                nav.frame.id.clone(),
                                nav.r#type.clone(),
                        ))
                    } else { None }
                },
                browser::EventDownloadWillBegin: event => {
                    if event.frame_id == frame_id {
                        Some(InnerEvent::DownloadWillBegin {
                            frame_id: event.frame_id.clone(),
                            url: event.url.clone(),
                        })
                    } else { None }
                },
                target::EventTargetDestroyed: event => {
                    Some(InnerEvent::TargetDestroyed(event.target_id.clone()))
                },
                runtime::EventConsoleApiCalled: call => {
                    let level = match call.r#type {
                        runtime::ConsoleApiCalledType::Error => {
                            state::ConsoleEntryLevel::Error
                        }
                        runtime::ConsoleApiCalledType::Warning => {
                            state::ConsoleEntryLevel::Warning
                        }
                        _ => continue,
                    };

                    Some(InnerEvent::ConsoleEntry(ConsoleEntry {
                        timestamp: UNIX_EPOCH
                            + Duration::from_secs_f64(
                                *call.timestamp.inner() / 1000.0,
                            ),
                        level,
                        args: call.args.iter().map(remote_object_to_json).collect(),
                    }))
                },
            }, _ => None);
            if let Some(inner_event) = inner_event
                && let Err(err) = events_tx.send(inner_event)
            {
                log::warn!("failed to send inner event: {err}");
            }
        }
    });

    Ok(())
}

fn run_state_machine(
    context: BrowserContext,
    events: mpmc::Receiver<InnerEvent>,
    done_sender: mpmc::Sender<()>,
) {
    let _ = thread::spawn(move || -> Result<()> {
        let result = {
            let shared = InnerStateShared::default();
            let mut state_current = InnerState {
                kind: InnerStateKind::Navigating {
                    url: context.origin.clone().into(),
                },
                shared,
            };
            log::info!("processing events");
            while let Ok(event) = events.recv() {
                state_current = if log::log_enabled!(log::Level::Debug) {
                    let before = format!(
                        "{:?} ({})",
                        state_current.kind, state_current.shared.generation
                    );
                    let event_formatted = format!("{:?}", event);
                    let state_new =
                        process_event(&context, state_current, event)?;
                    log::debug!(
                        "{} + {} -> {:?} ({})",
                        before,
                        event_formatted,
                        state_new.kind,
                        state_new.shared.generation
                    );
                    state_new
                } else {
                    process_event(&context, state_current, event)?
                }
            }
            log::debug!("shutting down browser state machine");
            Ok::<(), anyhow::Error>(())
        };
        if let Err(error) = result {
            log::error!("state machine error: {:?}", error);
            let _ = context.sender.send(BrowserEvent::Error(Arc::new(
                anyhow!("error when processing event: {:?}", error),
            )));
        }
        // Always signal done, whether the loop exited cleanly or with an error.
        let _ = done_sender.send(());
        Ok(())
    });
}

fn process_event(
    context: &BrowserContext,
    state_current: InnerState,
    event: InnerEvent,
) -> Result<InnerState> {
    use InnerStateKind::*;
    Ok(match (state_current, event) {
        (mut state, InnerEvent::ExecutionContextCreated(id)) => {
            log::debug!("execution context id created: {id}");
            state.shared.execution_context_id = Some(id);
            state
        }
        (mut state, InnerEvent::ExecutionContextDestroyed(id)) => {
            log::debug!("execution context id destroyed: {id}");
            if state.shared.execution_context_id == Some(id) {
                state.shared.execution_context_id = None;
            }
            state
        }
        (state, InnerEvent::StateRequested(reason, generation)) => {
            if state.shared.generation != generation {
                log::debug!("ignoring stale state request");
                state
            } else if matches!(
                state.kind,
                Navigating { .. } | Loading | Paused | Pausing
            ) {
                log::debug!(
                    "skipping state capture during {:?} (reason: {:?})",
                    state.kind,
                    reason
                );
                state
            } else {
                log::debug!(
                    "forcing pause from {:?} because of {:?}",
                    state,
                    reason
                );
                capture_browser_state(state, context)?
            }
        }
        (
            state,
            InnerEvent::Paused {
                call_frame_id: None,
                ..
            },
        ) => {
            log::debug!(
                "paused without call frame, resuming and retrying capture"
            );
            context.connection.send(
                debugger::ResumeParams::builder().build(),
                Some(&context.session_id),
            )?;
            let timer = start_quiescence_timer(
                &state.shared,
                context,
                &context.events_tx,
            )?;
            capture_browser_state(
                InnerState {
                    kind: InnerStateKind::Running(timer),
                    shared: state.shared,
                },
                context,
            )?
        }
        (
            state,
            InnerEvent::Paused {
                reason,
                exception,
                call_frame_id: Some(call_frame_id),
            },
        ) => {
            log::debug!("got paused event: {:?}, {:?}", reason, exception);

            if reason != debugger::PausedReason::Other {
                bail!(
                    "unexpected pause reason {:?} when in state: {:?}",
                    reason,
                    state
                );
            }

            let InnerStateShared {
                console_entries,
                exceptions,
                generation,
                screenshot,
                ..
            } = state.shared;
            let generation = generation.next();

            let screenshot = screenshot
                .ok_or(anyhow!("no screenshot available for state capture"))?;

            let browser_state = BrowserState::current(
                &context.connection,
                &context.session_id,
                &call_frame_id,
                console_entries,
                exceptions,
                screenshot,
                generation,
            )?;

            context
                .sender
                .send(BrowserEvent::StateChanged(browser_state))?;

            InnerState {
                kind: Paused,
                shared: InnerStateShared {
                    generation,
                    console_entries: vec![],
                    exceptions: vec![],
                    screenshot: None,
                    execution_context_id: None,
                },
            }
        }
        (
            InnerState {
                kind: Paused,
                shared,
            },
            InnerEvent::ActionAccepted(browser_action, generation),
        ) => {
            ensure!(
                shared.generation == generation,
                "cannot accept action from stale generation {generation}"
            );
            context.connection.send(
                debugger::ResumeParams::builder().build(),
                Some(&context.session_id),
            )?;
            InnerState {
                kind: Resuming(Box::new(browser_action)),
                shared,
            }
        }
        (
            InnerState { kind, shared },
            InnerEvent::ActionAccepted(action, generation),
        ) if shared.generation >= generation => {
            log::warn!(
                "ignoring stale action {:?}({}) received during {:?}({})",
                action,
                generation,
                kind,
                shared.generation
            );
            InnerState { kind, shared }
        }
        (
            InnerState {
                kind: Pausing,
                shared,
            },
            InnerEvent::Resumed,
        ) => {
            log::debug!("resumed while pausing, ignoring");
            InnerState {
                kind: Pausing,
                shared,
            }
        }
        (
            InnerState {
                kind: Running(timer),
                mut shared,
            },
            InnerEvent::Resumed,
        ) => {
            log::warn!("running + resumed");
            shared.console_entries.clear();
            InnerState {
                kind: Running(timer),
                shared,
            }
        }
        (
            InnerState {
                kind: Resuming(browser_action),
                mut shared,
            },
            InnerEvent::Resumed,
        ) => {
            let connection = context.connection.clone();
            let session_id = context.session_id.clone();
            let events_tx = context.events_tx.clone();
            let action_options = ActionOptions {
                device_scale_factor: context
                    .browser_options
                    .emulation
                    .device_scale_factor,
            };
            // We can't block on running the action, in case it
            // synchronously throws an uncaught exception blocking the
            // evaluation indefinitely. This gives us a chance to
            // receive the "Debugger.paused" event and resume
            // (extracting the uncaught exception information).
            thread::spawn(move || {
                log::debug!("applying: {:?}", browser_action);
                match browser_action.apply(
                    &connection,
                    &session_id,
                    action_options,
                ) {
                    Ok(_) => {
                        log::debug!("applied: {:?}", browser_action);
                    }
                    Err(err) => {
                        log::error!(
                            "failed to apply action {:?}: {:?}",
                            browser_action,
                            err
                        )
                    }
                }
                if let Err(error) =
                    events_tx.send(InnerEvent::ActionApplied(shared.generation))
                {
                    log::error!("failed to send ActionApplied: {}", error);
                }
            });

            shared.console_entries.clear();
            let (activity_tx, activity_rx) = mpmc::bounded(1024);
            activity::all_activity(&context.connection.events, activity_tx)?;
            let subscription = quiescence::subscribe(activity_rx);
            InnerState {
                kind: Acting(subscription),
                shared,
            }
        }
        (
            InnerState {
                kind: Acting(subscription),
                shared,
            },
            InnerEvent::ActionApplied(generation),
        ) if shared.generation == generation => {
            let timer = start_quiescence_timer_from_subscription(
                &shared,
                &context.events_tx,
                subscription,
            );
            InnerState {
                kind: Running(timer),
                shared,
            }
        }
        (state, InnerEvent::ActionApplied(_)) => {
            log::debug!("ignoring stale ActionApplied");
            state
        }
        (InnerState { shared, .. }, InnerEvent::Loaded) => {
            let timer =
                start_quiescence_timer(&shared, context, &context.events_tx)?;
            InnerState {
                kind: Running(timer),
                shared,
            }
        }
        (
            InnerState { shared, kind },
            InnerEvent::FrameRequestedNavigation {
                frame_id,
                reason,
                url,
            },
        ) => {
            if frame_id == context.frame_id {
                log::debug!(
                    "navigating to {} due to {:?} (current state is {:?}, {})",
                    url,
                    reason,
                    kind,
                    shared.generation,
                );
                let generation = shared.generation;
                let sender = context.events_tx.clone();
                thread::spawn(move || {
                    thread::sleep(NAVIGATION_TIMEOUT);
                    if let Err(err) =
                        sender.send(InnerEvent::NavigationTimedOut(generation))
                    {
                        log::warn!("failed to send NavigationTimedOut: {err}");
                    }
                });
                InnerState {
                    kind: Navigating { url },
                    shared,
                }
            } else {
                InnerState { shared, kind }
            }
        }
        (
            InnerState {
                kind: Navigating { .. },
                shared,
            },
            InnerEvent::DownloadWillBegin { frame_id, url },
        ) if frame_id == context.frame_id => {
            log::debug!("download started: {}", url);
            let timer =
                start_quiescence_timer(&shared, context, &context.events_tx)?;
            InnerState {
                kind: Running(timer),
                shared,
            }
        }
        (state, InnerEvent::DownloadWillBegin { .. }) => state,
        (
            InnerState {
                kind: Navigating { url },
                mut shared,
            },
            InnerEvent::ConsoleEntry(_),
        ) => {
            // NOTE: clearing between page navigations, but we could retain logs
            shared.console_entries.clear();
            InnerState {
                kind: Navigating { url },
                shared,
            }
        }
        (mut state, InnerEvent::ConsoleEntry(entry)) => {
            state.shared.console_entries.push(entry);
            state
        }
        (mut state, InnerEvent::ExceptionThrown(exception)) => {
            state.shared.exceptions.push(exception);
            if matches!(state.kind, Running(_)) {
                capture_browser_state(state, context)?
            } else {
                state
            }
        }
        (state, InnerEvent::FrameNavigated(frame_id, navigation_type)) => {
            if frame_id == context.frame_id {
                let shared = InnerStateShared {
                    generation: state.shared.generation.next(),
                    ..state.shared
                };
                let kind = match navigation_type {
                    NavigationType::Navigation => Loading,
                    NavigationType::BackForwardCacheRestore => {
                        let timer = start_quiescence_timer(
                            &shared,
                            context,
                            &context.events_tx,
                        )?;
                        Running(timer)
                    }
                };
                InnerState { kind, shared }
            } else {
                state
            }
        }
        (state, InnerEvent::TargetDestroyed(target_id)) => {
            if target_id == context.target_id {
                bail!("page target {:?} was destroyed", target_id);
            } else {
                state
            }
        }
        (state, InnerEvent::Quiesced(generation)) => {
            if state.shared.generation != generation {
                log::debug!("ignoring stale Quiesced event");
                state
            } else if matches!(state.kind, Running(_)) {
                log::debug!("quiesced, requesting new state capture");
                let _ = context.events_tx.send(InnerEvent::StateRequested(
                    StateRequestReason::Quiesced,
                    state.shared.generation,
                ));
                state
            } else {
                log::debug!("ignoring Quiesced during {:?}", state.kind,);
                state
            }
        }
        (state, InnerEvent::NavigationTimedOut(generation)) => {
            if state.shared.generation != generation {
                log::debug!("ignoring stale NavigationTimedOut");
                state
            } else if matches!(state.kind, Navigating { .. } | Loading) {
                bail!(
                    "navigation timed out after {:?} during {:?}",
                    NAVIGATION_TIMEOUT,
                    state.kind,
                );
            } else {
                state
            }
        }
        (state, event) => {
            bail!("unhandled transition: {:?} + {:?}", state, event);
        }
    })
}

fn start_quiescence_timer(
    shared: &InnerStateShared,
    context: &BrowserContext,
    events_tx: &mpmc::Sender<InnerEvent>,
) -> Result<quiescence::QuiescenceTimer> {
    let (activity_tx, activity_rx) = mpmc::bounded(1024);
    activity::all_activity(&context.connection.events, activity_tx)?;
    let subscription = quiescence::subscribe(activity_rx);
    Ok(start_quiescence_timer_from_subscription(
        shared,
        events_tx,
        subscription,
    ))
}

fn start_quiescence_timer_from_subscription(
    shared: &InnerStateShared,
    events_tx: &mpmc::Sender<InnerEvent>,
    subscription: quiescence::QuiescenceSubscription,
) -> quiescence::QuiescenceTimer {
    let (timer, quiescent) =
        subscription.start(QUIESCENCE_INITIAL_IDLE, QUIESCENCE_TIMEOUT);
    let generation = shared.generation;
    let sender = events_tx.clone();
    thread::spawn(move || match quiescent.recv() {
        Ok(true) => {
            log::debug!("quiescence timer fired for generation {generation}");
            let _ = sender.send(InnerEvent::Quiesced(generation));
        }
        Ok(false) => {
            log::debug!(
                "quiescence timer cancelled for generation {generation}",
            );
        }
        Err(err) => {
            log::debug!(
                "quiescence timer failed for generation {generation} on recv: {err}",
            );
        }
    });
    timer
}

fn capture_browser_state(
    mut state: InnerState,
    context: &BrowserContext,
) -> Result<InnerState> {
    fn retry_with_timer(
        shared: InnerStateShared,
        context: &BrowserContext,
    ) -> Result<InnerState> {
        let timer =
            start_quiescence_timer(&shared, context, &context.events_tx)?;
        Ok(InnerState {
            kind: InnerStateKind::Running(timer),
            shared,
        })
    }
    log::debug!("pausing, going into next generation...");

    let execution_context_id = match state.shared.execution_context_id {
        Some(ref id) => id.clone(),
        None => {
            log::debug!(
                "no execution context id available, skipping state capture"
            );
            return retry_with_timer(state.shared, context);
        }
    };

    let frame = context
        .latest_frame
        .lock()
        .expect("failed getting latest frame from mutex")
        .clone();
    match frame {
        Some(data) => {
            state.shared.screenshot = Some(Screenshot {
                format: ScreenshotFormat::Jpeg,
                data: data.to_vec(),
            });
        }
        None => {
            log::warn!("no screencast frame available, skipping state capture");
            return retry_with_timer(state.shared, context);
        }
    }

    let connection = context.connection.clone();
    let session_id = context.session_id.clone();
    thread::spawn(move || {
        let _ = connection.send(
            runtime::EvaluateParams::builder()
                .expression("debugger;0")
                .unique_context_id(execution_context_id)
                .await_promise(false)
                .build()
                .expect("failed to build EvaluateParams"),
            Some(&session_id),
        );
    });

    state.shared.generation = state.shared.generation.next();
    Ok(InnerState {
        kind: InnerStateKind::Pausing,
        shared: state.shared,
    })
}

fn remote_object_to_json(object: &runtime::RemoteObject) -> json::Value {
    match (&object.r#type, &object.value, &object.description) {
        (_, Some(value), _) => value.clone(),
        (_, None, Some(description)) => {
            json::Value::String(description.clone())
        }
        (r#type, _, _) => {
            json::Value::String(format!("<object of type {:?}>", r#type))
        }
    }
}

/*
fn launch_options_to_config(
    launch_options: &LaunchOptions,
    emulation: &Emulation,
) -> Result<BrowserConfig> {
    let crash_dumps_dir = TempDir::new()?;

    let apply_sandbox =
        |builder: BrowserConfigBuilder| -> BrowserConfigBuilder {
            if launch_options.no_sandbox {
                builder
                    .no_sandbox()
                    .arg("disable-setuid-sandbox")
                    .arg("disable-dev-shm-usage")
            } else {
                builder
            }
        };
    let apply_headless =
        |builder: BrowserConfigBuilder| -> BrowserConfigBuilder {
            if launch_options.headless {
                builder
            } else {
                builder.with_head()
            }
        };
    apply_headless(apply_sandbox(BrowserConfig::builder()))
        .window_size(emulation.width as u32, emulation.height as u32)
        .user_data_dir(launch_options.user_data_directory.clone())
        .arg((
            "crash-dumps-dir",
            crash_dumps_dir
                .path()
                .to_path_buf()
                .to_str()
                .expect("invalid tmp dir path"),
        ))
        .arg("enable-logging")
        .arg(("v", "1"))
        .arg("no-crashpad")
        .arg("disable-background-networking")
        .arg("disable-component-update")
        .arg("disable-domain-reliability")
        .arg("no-pings")
        .arg("disable-crash-reporter")
        .build()
        .map_err(|s| anyhow!(s))
}
*/

fn find_page(connection: &cdp::Connection) -> Result<(TargetId, SessionId)> {
    let page_targets = connection
        .send(
            target::GetTargetsParams {
                filter: Some(target::TargetFilter::new(vec![
                    target::FilterEntry {
                        r#type: Some("page".into()),
                        ..Default::default()
                    },
                ])),
            },
            None,
        )?
        .target_infos;

    log::debug!("targets: {:?}", page_targets);

    let target = page_targets
        .first()
        .ok_or(anyhow!("no page target available"))?;

    if page_targets.len() >= 2 {
        log::warn!(
            "there are multiple open page targets, picking the first one: {}",
            target.url
        )
    }
    for attempt in 1..=5 {
        log::debug!("attempt {attempt} at finding existing page");
        thread::sleep(Duration::from_millis(100 * attempt));
        if let Ok(attachment) = connection.send(
            target::AttachToTargetParams {
                target_id: target.target_id.clone(),
                flatten: Some(true),
            },
            None,
        ) {
            return Ok((target.target_id.clone(), attachment.session_id));
        }
    }
    bail!("coulnd't find an existing page to use");
}
