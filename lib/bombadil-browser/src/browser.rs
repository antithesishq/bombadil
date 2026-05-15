use anyhow::{Context, Result, anyhow, bail};
use chromiumoxide::browser::BrowserConfigBuilder;
use chromiumoxide::cdp::browser_protocol::browser;
use chromiumoxide::cdp::browser_protocol::emulation;
use chromiumoxide::cdp::browser_protocol::network;
use chromiumoxide::cdp::browser_protocol::page::{
    self, ClientNavigationReason, FrameId, NavigationType,
};
use chromiumoxide::cdp::browser_protocol::target::{self, TargetId};
use chromiumoxide::cdp::js_protocol::debugger::{self, CallFrameId};
use chromiumoxide::cdp::js_protocol::runtime::{self};
use chromiumoxide::{BrowserConfig, Page};
use futures::{StreamExt, stream};
use log;
use serde_json as json;
use std::collections::HashMap;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};
use tempfile::TempDir;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::broadcast::{Receiver, Sender, channel};
use tokio::sync::oneshot;
use tokio::time::sleep;
use tokio::{select, spawn};
use tokio_stream::wrappers::BroadcastStream;
use url::Url;

use crate::browser::actions::BrowserAction;
use crate::browser::state::{
    BrowserState, CallFrame, ConsoleEntry, Exception, Screenshot,
    ScreenshotFormat,
};

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
}

#[derive(Debug)]
struct InnerState {
    kind: InnerStateKind,
    shared: InnerStateShared,
}

enum InnerStateKind {
    Pausing,
    Paused,
    Resuming(BrowserAction),
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
    TargetDestroyed(TargetId),
    ConsoleEntry(ConsoleEntry),
    ActionAccepted(BrowserAction),
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

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
struct Generation(u64);

impl Generation {
    fn next(self) -> Self {
        Generation(self.0 + 1)
    }
}

impl std::fmt::Display for Generation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Initial idle timeout before the first activity signal arrives.
/// Deliberately long so we don't fire before the browser has produced
/// any frames; the first activity event will replace this with a much shorter
/// deadline.
const QUIESCENCE_INITIAL_IDLE: Duration = Duration::from_millis(250);
const QUIESCENCE_TIMEOUT: Duration = Duration::from_secs(10);
const NAVIGATION_TIMEOUT: Duration = Duration::from_secs(30);

struct BrowserContext {
    sender: Sender<BrowserEvent>,
    actions_sender: Sender<BrowserAction>,
    inner_events_sender: Sender<InnerEvent>,
    shutdown_receiver: oneshot::Receiver<()>,
    page: Arc<Page>,
    frame_id: FrameId,
    network_activity: activity::NetworkActivity,
    screencast_activity: activity::ScreencastActivity,
    latest_frame: Arc<Mutex<Option<Arc<[u8]>>>>,
    #[allow(unused, reason = "this is going into the scripts soon")]
    origin: Url,
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
}

#[derive(Clone)]
pub enum DebuggerOptions {
    External { remote_debugger: Url },
    Managed { launch_options: LaunchOptions },
}

pub struct Browser {
    receiver: Receiver<BrowserEvent>,
    inner_events_sender: Sender<InnerEvent>,
    actions_sender: Sender<BrowserAction>,
    shutdown_sender: Option<oneshot::Sender<()>>,
    done_receiver: Option<oneshot::Receiver<()>>,
    browser: Option<chromiumoxide::Browser>,
    page: Arc<Page>,
    origin: Url,
    go_to_origin_on_init: bool,
}

impl Drop for Browser {
    fn drop(&mut self) {
        if let Some(sender) = self.shutdown_sender.take() {
            let _else = sender.send(());
        }
        if let Some(browser) = self.browser.take() {
            // Drop should already have been called by an explicit browser.close() in
            // terminate(), but we do this as a last resort.
            drop(browser);
        }
    }
}

impl Browser {
    pub async fn new(
        origin: Url,
        browser_options: BrowserOptions,
        debugger_options: DebuggerOptions,
    ) -> Result<Self> {
        let (mut browser, mut handler) = match debugger_options {
            DebuggerOptions::External {
                ref remote_debugger,
            } => {
                chromiumoxide::Browser::connect(remote_debugger.as_str())
                    .await?
            }
            DebuggerOptions::Managed { ref launch_options } => {
                let browser_config = launch_options_to_config(
                    launch_options,
                    &browser_options.emulation,
                )?;
                chromiumoxide::Browser::launch(browser_config).await?
            }
        };

        let _handle = tokio::spawn(async move {
            loop {
                let _ = handler.next().await;
            }
        });

        let (sender, receiver) = channel::<BrowserEvent>(1);

        let (actions_sender, _) = channel::<BrowserAction>(1);

        let page = if browser_options.create_target {
            Arc::new(browser.new_page("about:blank").await.context(
                "could not create target (is this supported by the CDP host?)",
            )?)
        } else {
            Arc::new(find_page(&mut browser).await?)
        };

        page.enable_dom().await?;
        page.enable_css().await?;
        page.enable_runtime().await?;
        page.enable_debugger().await?;
        page.execute(network::EnableParams::default()).await?;

        if !browser_options.extra_headers.is_empty() {
            page.execute(network::SetExtraHttpHeadersParams::new(
                network::Headers::new(json::to_value(
                    &browser_options.extra_headers,
                )?),
            ))
            .await?;
        }

        // Prevent file downloads to avoid getting stuck
        page.execute(
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
        )
        .await?;

        for permission in &browser_options.grant_permissions {
            page.execute(
                browser::SetPermissionParams::builder()
                    .permission(browser::PermissionDescriptor::new(permission))
                    .setting(browser::PermissionSetting::Granted)
                    .build()
                    .map_err(|s| {
                        anyhow!(s).context("build SetPermissionParams failed")
                    })?,
            )
            .await?;
        }

        page.execute(
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
        )
        .await?;

        auto_accept_dialogs(page.clone()).await?;

        let (inner_events_sender, inner_events_receiver) =
            channel::<InnerEvent>(1024);

        let (shutdown_sender, shutdown_receiver) = oneshot::channel::<()>();
        let (done_sender, done_receiver) = oneshot::channel::<()>();

        let frame_id = page
            .mainframe()
            .await?
            .ok_or(anyhow!("no main frame available"))?;

        let network_activity =
            activity::NetworkActivity::subscribe(&page).await?;
        let screencast = Arc::new(
            activity::Screencast::start(
                &page,
                browser_options.emulation.width,
                browser_options.emulation.height,
            )
            .await?,
        );
        let screencast_activity =
            activity::ScreencastActivity::new(screencast.clone());

        let latest_frame: Arc<Mutex<Option<Arc<[u8]>>>> =
            Arc::new(Mutex::new(None));

        // Background task to keep the latest screencast frame updated.
        {
            let latest_frame = latest_frame.clone();
            let mut receiver = screencast.subscribe();
            spawn(async move {
                loop {
                    match receiver.recv().await {
                        Ok(frame) => {
                            *latest_frame.lock().unwrap() = Some(frame);
                        }
                        Err(
                            tokio::sync::broadcast::error::RecvError::Lagged(n),
                        ) => {
                            log::debug!(
                                "screencast frame receiver lagged by {}",
                                n
                            );
                        }
                        Err(
                            tokio::sync::broadcast::error::RecvError::Closed,
                        ) => break,
                    }
                }
            });
        }

        let context = BrowserContext {
            sender,
            actions_sender: actions_sender.clone(),
            inner_events_sender: inner_events_sender.clone(),
            shutdown_receiver,
            page: page.clone(),
            frame_id,
            network_activity,
            screencast_activity,
            latest_frame,
            origin: origin.clone(),
        };

        instrumentation::instrument_js_coverage(
            page.clone(),
            browser_options.instrumentation.clone(),
        )
        .await?;

        let browser_events = browser
            .event_listener::<target::EventTargetDestroyed>()
            .await?
            .map(|event| InnerEvent::TargetDestroyed(event.target_id.clone()));

        let events_all = stream::select_all(vec![
            inner_events(&context).await?,
            Box::pin(browser_events),
            receiver_to_stream(inner_events_receiver),
        ]);
        run_state_machine(context, events_all, done_sender);

        Ok(Browser {
            browser: Some(browser),
            receiver,
            inner_events_sender,
            actions_sender,
            shutdown_sender: Some(shutdown_sender),
            done_receiver: Some(done_receiver),
            page,
            origin,
            go_to_origin_on_init: browser_options.create_target,
        })
    }

    pub async fn initiate(&mut self) -> Result<()> {
        if self.go_to_origin_on_init {
            let page = self.page.clone();
            let origin = self.origin.to_string();
            spawn(async move {
                log::info!("going to origin");
                let _ = page.goto(origin).await;
            });
        } else {
            let _ = self.inner_events_sender.send(InnerEvent::StateRequested(
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
        // Send the shutdown signal first so the state machine can exit cleanly
        // if it is between events.
        if let Some(sender) = self.shutdown_sender.take() {
            let _ = sender.send(());
        }

        // Close the browser before waiting for the state machine. Any CDP calls
        // in-flight inside process_event will fail once the connection drops,
        // unblocking the state machine so it can exit. Without this ordering,
        // terminate() could deadlock: the state machine waits for a CDP response
        // and the browser never closes because we're waiting for the state machine.
        if let Some(mut browser) = self.browser.take() {
            if let Err(error) = browser.close().await {
                log::warn!("browser close error: {:?}", error);
            }
            // Drop explicitly; browser.close() may log a websocket error but
            // the process is cleaned up here.
            // Reported: https://github.com/mattsse/chromiumoxide/issues/287
            drop(browser);
        }

        // Wait for the state machine to confirm it has exited. The done signal
        // is always sent now (even on error), so this should resolve promptly.
        if let Some(done_receiver) = self.done_receiver.take() {
            let _ = done_receiver.await;
        }

        Ok(())
    }

    pub async fn next_event(&mut self) -> Option<BrowserEvent> {
        match self.receiver.recv().await {
            Ok(event) => Some(event),
            Err(RecvError::Closed) => None,
            Err(error) => Some(BrowserEvent::Error(Arc::new(anyhow!(error)))),
        }
    }

    pub fn apply(&mut self, action: BrowserAction) -> Result<()> {
        self.actions_sender.send(action)?;
        Ok(())
    }

    pub fn origin(&self) -> &Url {
        &self.origin
    }

    pub async fn ensure_script_evaluated(&self, script: &str) -> Result<()> {
        let _ = self.page.evaluate_on_new_document(script).await?;

        let main_execution_context_id = self
            .page
            .execution_context()
            .await?
            .ok_or(anyhow!("no execution context available"))?;
        let _ = self
            .page
            .execute(
                runtime::EvaluateParams::builder()
                    .expression(script)
                    .context_id(main_execution_context_id)
                    .await_promise(true)
                    .build()
                    .expect("failed to build EvaluateParams"),
            )
            .await;
        Ok(())
    }
}

/// Auto-accept JavaScript dialogs (alert, confirm, prompt, beforeunload)
/// so they never block the test run.
async fn auto_accept_dialogs(page: Arc<Page>) -> Result<()> {
    let mut events = page
        .event_listener::<page::EventJavascriptDialogOpening>()
        .await?;
    spawn(async move {
        while let Some(event) = events.next().await {
            log::debug!(
                "auto-accepting JavaScript dialog: \
                 type={:?} message={:?}",
                event.r#type,
                event.message
            );
            let _ = page
                .execute(
                    page::HandleJavaScriptDialogParams::builder()
                        .accept(true)
                        .build()
                        .expect("build HandleJavaScriptDialogParams"),
                )
                .await;
        }
    });
    Ok(())
}

async fn inner_events(
    context: &BrowserContext,
) -> Result<Pin<Box<dyn stream::Stream<Item = InnerEvent> + Send>>> {
    type InnerEventStream =
        Pin<Box<dyn stream::Stream<Item = InnerEvent> + Send>>;

    let events_loaded = Box::pin(
        context
            .page
            .event_listener::<page::EventLoadEventFired>()
            .await?
            .map(|_| InnerEvent::Loaded),
    ) as InnerEventStream;

    let events_paused = Box::pin(
        context
            .page
            .event_listener::<debugger::EventPaused>()
            .await?
            .map(|event| InnerEvent::Paused {
                reason: event.reason.clone(),
                exception: event.data.clone(),
                call_frame_id: event
                    .call_frames
                    .first()
                    .map(|f| f.call_frame_id.clone()),
            }),
    ) as InnerEventStream;

    let events_resumed = Box::pin(
        context
            .page
            .event_listener::<debugger::EventResumed>()
            .await?
            .map(|_| InnerEvent::Resumed),
    ) as InnerEventStream;

    let events_exception_thrown = Box::pin(
        context
            .page
            .event_listener::<runtime::EventExceptionThrown>()
            .await?
            .map(|e| {
                InnerEvent::ExceptionThrown(Exception {
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
                })
            }),
    ) as InnerEventStream;

    let frame_id = context.frame_id.clone();
    let events_frame_requested_navigation = Box::pin(
        context
            .page
            .event_listener::<page::EventFrameRequestedNavigation>()
            .await?
            .filter_map(move |nav| {
                let frame_id = frame_id.clone();
                async move {
                    if nav.frame_id == frame_id {
                        Some(InnerEvent::FrameRequestedNavigation {
                            frame_id: nav.frame_id.clone(),
                            reason: nav.reason.clone(),
                            url: nav.url.clone(),
                        })
                    } else {
                        None
                    }
                }
            }),
    ) as InnerEventStream;

    let frame_id = context.frame_id.clone();
    let events_frame_navigated = Box::pin(
        context
            .page
            .event_listener::<page::EventFrameNavigated>()
            .await?
            .filter_map(move |nav| {
                let frame_id = frame_id.clone();
                async move {
                    if nav.frame.id == frame_id {
                        Some(InnerEvent::FrameNavigated(
                            nav.frame.id.clone(),
                            nav.r#type.clone(),
                        ))
                    } else {
                        None
                    }
                }
            }),
    ) as InnerEventStream;

    let frame_id = context.frame_id.clone();
    let events_download_will_begin = Box::pin(
        context
            .page
            .event_listener::<browser::EventDownloadWillBegin>()
            .await?
            .filter_map(move |event| {
                let frame_id = frame_id.clone();
                async move {
                    if event.frame_id == frame_id {
                        Some(InnerEvent::DownloadWillBegin {
                            frame_id: event.frame_id.clone(),
                            url: event.url.clone(),
                        })
                    } else {
                        None
                    }
                }
            }),
    ) as InnerEventStream;

    let events_target_destroyed = Box::pin(
        context
            .page
            .event_listener::<target::EventTargetDestroyed>()
            .await?
            .map(|event| InnerEvent::TargetDestroyed(event.target_id.clone())),
    ) as InnerEventStream;

    let events_console = Box::pin(
        context
            .page
            .event_listener::<runtime::EventConsoleApiCalled>()
            .await?
            .filter_map(async |call| {
                let level = match call.r#type {
                    runtime::ConsoleApiCalledType::Error => {
                        state::ConsoleEntryLevel::Error
                    }
                    runtime::ConsoleApiCalledType::Warning => {
                        state::ConsoleEntryLevel::Warning
                    }
                    _ => return None,
                };

                Some(InnerEvent::ConsoleEntry(ConsoleEntry {
                    timestamp: UNIX_EPOCH
                        + Duration::from_secs_f64(
                            *call.timestamp.inner() / 1000.0,
                        ),
                    level,
                    args: call.args.iter().map(remote_object_to_json).collect(),
                }))
            }),
    ) as InnerEventStream;

    let events_action_accepted = Box::pin(
        receiver_to_stream(context.actions_sender.subscribe())
            .map(InnerEvent::ActionAccepted),
    );

    Ok(Box::pin(stream::select_all(vec![
        events_loaded,
        events_paused,
        events_resumed,
        events_exception_thrown,
        events_frame_requested_navigation,
        events_frame_navigated,
        events_download_will_begin,
        events_target_destroyed,
        events_console,
        events_action_accepted,
    ])))
}

fn run_state_machine(
    mut context: BrowserContext,
    mut events: impl stream::Stream<Item = InnerEvent> + Send + Unpin + 'static,
    done_sender: oneshot::Sender<()>,
) {
    spawn(async move {
        let result = async {
            let shared = InnerStateShared::default();
            let mut state_current = InnerState {
                kind: InnerStateKind::Navigating { url: context.origin.clone().into() },
                shared,
            };
            log::info!("processing events");
            loop {
                select! {
                    _ = &mut context.shutdown_receiver => {
                        log::debug!("shutting down browser state machine");
                        break;
                    },
                    event = events.next() => match event {
                        Some(event) => {
                            state_current = if log::log_enabled!(log::Level::Debug) {
                                let before = format!("{:?} ({})", &state_current.kind, &state_current.shared.generation);
                                let event_formatted = format!("{:?}", &event);
                                let state_new = Box::pin(process_event(&context, state_current, event)).await?;
                                log::debug!("{} + {} -> {:?} ({})", before, event_formatted, &state_new.kind, &state_new.shared.generation);
                                state_new
                            } else {
                                Box::pin(process_event(&context, state_current, event)).await?
                            }
                        }
                        None => {
                            log::debug!("no more events, shutting down state machine loop");
                            break;
                        }
                    }
                }
            }
            Ok::<(), anyhow::Error>(())
        }.await;
        if let Err(error) = result {
            log::error!("state machine error: {:?}", error);
            let _ = context.sender.send(BrowserEvent::Error(Arc::new(
                anyhow!("error when processing event: {:?}", error),
            )));
        }
        // Always signal done, whether the loop exited cleanly or with an error.
        let _ = done_sender.send(());
    });
}

async fn process_event(
    context: &BrowserContext,
    state_current: InnerState,
    event: InnerEvent,
) -> Result<InnerState> {
    use InnerStateKind::*;
    Ok(match (state_current, event) {
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
                    &state.kind,
                    reason
                );
                state
            } else {
                log::debug!(
                    "forcing pause from {:?} because of {:?}",
                    &state,
                    reason
                );
                capture_browser_state(state, context).await?
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
            context
                .page
                .execute(debugger::ResumeParams::builder().build())
                .await?;
            let timer = start_quiescence_timer(
                &state.shared,
                context,
                &context.inner_events_sender,
            );
            capture_browser_state(
                InnerState {
                    kind: InnerStateKind::Running(timer),
                    shared: state.shared,
                },
                context,
            )
            .await?
        }
        (
            state,
            InnerEvent::Paused {
                reason,
                exception,
                call_frame_id: Some(call_frame_id),
            },
        ) => {
            log::debug!("got paused event: {:?}, {:?}", &reason, &exception);

            if reason != debugger::PausedReason::Other {
                bail!(
                    "unexpected pause reason {:?} when in state: {:?}",
                    reason,
                    &state
                );
            }

            let InnerStateShared {
                console_entries,
                exceptions,
                generation,
                screenshot,
                ..
            } = state.shared;

            let screenshot = screenshot
                .ok_or(anyhow!("no screenshot available for state capture"))?;

            let browser_state = BrowserState::current(
                context.page.clone(),
                &call_frame_id,
                console_entries,
                exceptions,
                screenshot,
            )
            .await?;

            context
                .sender
                .send(BrowserEvent::StateChanged(browser_state))?;

            let generation = generation.next();

            InnerState {
                kind: Paused,
                shared: InnerStateShared {
                    generation,
                    console_entries: vec![],
                    exceptions: vec![],
                    screenshot: None,
                },
            }
        }
        (
            InnerState {
                kind: Paused,
                shared,
            },
            InnerEvent::ActionAccepted(browser_action),
        ) => {
            context
                .page
                .execute(debugger::ResumeParams::builder().build())
                .await?;
            InnerState {
                kind: Resuming(browser_action),
                shared,
            }
        }
        (
            state @ InnerState {
                kind: Loading | Navigating { .. } | Pausing,
                ..
            },
            InnerEvent::ActionAccepted(action),
        ) => {
            log::debug!(
                "ignoring action {:?} received during {:?}",
                action,
                state.kind
            );
            state
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
            let page = context.page.clone();
            let sender = context.inner_events_sender.clone();
            // We can't block on running the action, in case it
            // synchronously throws an uncaught exception blocking the
            // evaluation indefinitely. This gives us a chance to
            // receive the "Debugger.paused" event and resume
            // (extracting the uncaught exception information).
            spawn(async move {
                log::debug!("applying: {:?}", browser_action);
                match browser_action.apply(&page).await {
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
                    sender.send(InnerEvent::ActionApplied(shared.generation))
                {
                    log::error!("failed to send ActionApplied: {}", error);
                }
            });

            shared.console_entries.clear();
            let activity = Box::pin(stream::select(
                context.network_activity.stream(),
                context.screencast_activity.stream(),
            )) as activity::ActivityStream;
            let subscription = quiescence::subscribe(activity);
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
                &context.inner_events_sender,
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
            let timer = start_quiescence_timer(
                &shared,
                context,
                &context.inner_events_sender,
            );
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
                let sender = context.inner_events_sender.clone();
                spawn(async move {
                    sleep(NAVIGATION_TIMEOUT).await;
                    let _ =
                        sender.send(InnerEvent::NavigationTimedOut(generation));
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
            let timer = start_quiescence_timer(
                &shared,
                context,
                &context.inner_events_sender,
            );
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
                capture_browser_state(state, context).await?
            } else {
                state
            }
        }
        (state, InnerEvent::FrameNavigated(frame_id, navigation_type)) => {
            if frame_id == context.frame_id {
                let kind = match navigation_type {
                    NavigationType::Navigation => Loading,
                    NavigationType::BackForwardCacheRestore => {
                        let timer = start_quiescence_timer(
                            &state.shared,
                            context,
                            &context.inner_events_sender,
                        );
                        Running(timer)
                    }
                };
                InnerState {
                    kind,
                    shared: state.shared,
                }
            } else {
                state
            }
        }
        (state, InnerEvent::TargetDestroyed(target_id)) => {
            if target_id == *context.page.target_id() {
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
                let _ = context.inner_events_sender.send(
                    InnerEvent::StateRequested(
                        StateRequestReason::Quiesced,
                        state.shared.generation,
                    ),
                );
                state
            } else {
                log::debug!("ignoring Quiesced during {:?}", &state.kind,);
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
                    &state.kind,
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
    inner_events_sender: &Sender<InnerEvent>,
) -> quiescence::QuiescenceTimer {
    let activity = Box::pin(stream::select(
        context.network_activity.stream(),
        context.screencast_activity.stream(),
    )) as activity::ActivityStream;
    let subscription = quiescence::subscribe(activity);
    start_quiescence_timer_from_subscription(
        shared,
        inner_events_sender,
        subscription,
    )
}

fn start_quiescence_timer_from_subscription(
    shared: &InnerStateShared,
    inner_events_sender: &Sender<InnerEvent>,
    subscription: quiescence::QuiescenceSubscription,
) -> quiescence::QuiescenceTimer {
    let (timer, quiescent) =
        subscription.start(QUIESCENCE_INITIAL_IDLE, QUIESCENCE_TIMEOUT);
    let generation = shared.generation;
    let sender = inner_events_sender.clone();
    spawn(async move {
        if quiescent.await {
            log::debug!("quiescence timer fired for generation {}", generation);
            let _ = sender.send(InnerEvent::Quiesced(generation));
        }
    });
    timer
}

async fn capture_browser_state(
    mut state: InnerState,
    context: &BrowserContext,
) -> Result<InnerState> {
    fn retry_with_timer(
        shared: InnerStateShared,
        context: &BrowserContext,
    ) -> InnerState {
        let timer = start_quiescence_timer(
            &shared,
            context,
            &context.inner_events_sender,
        );
        InnerState {
            kind: InnerStateKind::Running(timer),
            shared,
        }
    }
    log::debug!("pausing, going into next generation...");

    let page = context.page.clone();
    let main_execution_context_id = match page.execution_context().await? {
        Some(ctx) => ctx,
        None => {
            log::debug!("no execution context, skipping state capture");
            return Ok(retry_with_timer(state.shared, context));
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
            return Ok(retry_with_timer(state.shared, context));
        }
    }

    let page = context.page.clone();
    spawn(async move {
        let _ = page
            .execute(
                runtime::EvaluateParams::builder()
                    .expression("debugger;0")
                    .context_id(main_execution_context_id)
                    .await_promise(false)
                    .build()
                    .expect("failed to build EvaluateParams"),
            )
            .await;
    });

    state.shared.generation = state.shared.generation.next();
    Ok(InnerState {
        kind: InnerStateKind::Pausing,
        shared: state.shared,
    })
}

fn receiver_to_stream<T: Clone + Send + 'static>(
    receiver: Receiver<T>,
) -> Pin<Box<dyn stream::Stream<Item = T> + Send>> {
    Box::pin(BroadcastStream::new(receiver).filter_map(async |r| r.ok()))
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

fn launch_options_to_config(
    launch_options: &LaunchOptions,
    emulation: &Emulation,
) -> Result<BrowserConfig> {
    let crash_dumps_dir = TempDir::new()?;
    let apply_sandbox =
        |builder: BrowserConfigBuilder| -> BrowserConfigBuilder {
            if launch_options.no_sandbox {
                builder.no_sandbox().args([
                    "--disable-setuid-sandbox",
                    "--disable-dev-shm-usage",
                ])
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
        .args([
            &format!(
                "--crash-dumps-dir={}",
                crash_dumps_dir
                    .path()
                    .to_path_buf()
                    .to_str()
                    .expect("invalid tmp dir path")
            ),
            "--no-crashpad",
            "--disable-background-networking",
            "--disable-component-update",
            "--disable-domain-reliability",
            "--no-pings",
            "--disable-crash-reporter",
        ])
        .build()
        .map_err(|s| anyhow!(s))
}

async fn find_page(browser: &mut chromiumoxide::Browser) -> Result<Page> {
    let targets = browser.fetch_targets().await.unwrap();
    let page_targets = targets
        .iter()
        .filter(|t| t.r#type == "page")
        .collect::<Vec<_>>();

    log::debug!("targets: {:?}", page_targets);

    let target = page_targets
        .first()
        .ok_or(anyhow!("no page target available"))?;

    if page_targets.len() > 2 {
        log::warn!(
            "there are multiple open page targets, picking the first one: {}",
            &target.url
        )
    }
    for attempt in 1..=5 {
        log::debug!("attempt {attempt} at finding existing page");
        sleep(Duration::from_millis(100 * attempt)).await;
        if let Ok(page) = browser.get_page(target.target_id.clone()).await {
            return Ok(page);
        }
    }
    bail!("coulnd't find an existing page to use");
}
