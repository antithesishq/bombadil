use anyhow::{anyhow, bail, Context, Result};
use chromiumoxide::browser::{BrowserConfigBuilder, HeadlessMode};
use chromiumoxide::cdp::browser_protocol::page::{
    self, ClientNavigationReason, FrameId, NavigationType,
};
use chromiumoxide::cdp::browser_protocol::target::{self, TargetId};
use chromiumoxide::cdp::browser_protocol::{dom, emulation};
use chromiumoxide::cdp::js_protocol::debugger::{self, CallFrameId};
use chromiumoxide::cdp::js_protocol::runtime::{self};
use chromiumoxide::{BrowserConfig, Page};
use futures::{stream, StreamExt};
use log;
use serde_json as json;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};
use tempfile::TempDir;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::broadcast::{channel, Receiver, Sender};
use tokio::sync::oneshot;
use tokio::time::sleep;
use tokio::{select, spawn};
use tokio_stream::wrappers::BroadcastStream;
use url::Url;

use crate::browser::actions::BrowserAction;
use crate::browser::state::{BrowserState, ConsoleEntry, Exception};

pub mod actions;
pub mod evaluation;
pub mod instrumentation;
pub mod keys;
pub mod random;
pub mod state;

#[derive(Debug, Clone)]
pub enum BrowserEvent {
    StateChanged(BrowserState),
    Error(Arc<anyhow::Error>),
}

#[derive(Debug)]
enum InnerState {
    Pausing(Vec<ConsoleEntry>),
    Paused,
    Resuming(BrowserAction),
    Navigating,
    Loading(Vec<ConsoleEntry>),
    Running(Vec<ConsoleEntry>),
}

#[derive(Clone, Debug)]
pub enum InnerEvent {
    StateRequested,
    Loaded,
    Paused {
        reason: debugger::PausedReason,
        exception: Option<json::Value>,
        call_frame_id: Option<CallFrameId>,
    },
    Resumed,
    FrameRequestedNavigation(FrameId, ClientNavigationReason, String),
    FrameNavigated(FrameId, NavigationType),
    TargetDestroyed(TargetId),
    NodeTreeModified(NodeModification),
    ConsoleEntry(ConsoleEntry),
    ActionApplied(BrowserAction),
}

#[derive(Clone, Debug)]
pub enum NodeModification {
    ChildNodeInserted {
        parent: dom::NodeId,
        child: dom::Node,
    },
    ChildNodeCountUpdated {
        parent: dom::NodeId,
        count: u64,
    },
    ChildNodeRemoved {
        parent: dom::NodeId,
        child: dom::NodeId,
    },
    AttributeModified {
        node: dom::NodeId,
        name: String,
        value: String,
    },
}

struct BrowserContext {
    sender: Sender<BrowserEvent>,
    actions_sender: Sender<BrowserAction>,
    inner_events_sender: Sender<InnerEvent>,
    shutdown_receiver: oneshot::Receiver<()>,
    page: Arc<Page>,
    frame_id: FrameId,
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
}

#[derive(Clone)]
pub enum DebuggerOptions {
    External { remote_debugger: Url },
    Managed { launch_options: LaunchOptions },
}

pub struct Browser {
    receiver: Receiver<BrowserEvent>,
    actions_sender: Sender<BrowserAction>,
    inner_events_sender: Sender<InnerEvent>,
    shutdown_sender: oneshot::Sender<()>,
    done_receiver: oneshot::Receiver<()>,
    browser: chromiumoxide::Browser,
    page: Arc<Page>,
    origin: Url,
    go_to_origin_on_init: bool,
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

        page.execute(
            debugger::SetPauseOnExceptionsParams::builder()
                .state(debugger::SetPauseOnExceptionsState::Uncaught)
                .build()
                .map_err(|err| {
                    anyhow!(err)
                        .context("build SetPauseOnExceptionsState failed")
                })?,
        )
        .await?;

        let (inner_events_sender, inner_events_receiver) =
            channel::<InnerEvent>(1024);

        let (shutdown_sender, shutdown_receiver) = oneshot::channel::<()>();
        let (done_sender, done_receiver) = oneshot::channel::<()>();

        let frame_id = page
            .mainframe()
            .await?
            .ok_or(anyhow!("no main frame available"))?;

        let context = BrowserContext {
            sender,
            actions_sender: actions_sender.clone(),
            inner_events_sender: inner_events_sender.clone(),
            shutdown_receiver,
            page: page.clone(),
            frame_id,
            origin: origin.clone(),
        };

        instrumentation::instrument_js_coverage(page.clone()).await?;

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
            browser,
            receiver,
            actions_sender,
            inner_events_sender,
            shutdown_sender,
            done_receiver,
            page,
            origin,
            go_to_origin_on_init: matches!(
                debugger_options,
                DebuggerOptions::Managed { .. }
            ),
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
            log::debug!(
                "using externally managed debugger, not doing anything on init"
            )
        }
        Ok(())
    }

    pub async fn terminate(self) -> Result<()> {
        let Browser {
            shutdown_sender,
            done_receiver,
            browser,
            ..
        } = self;
        if let Ok(()) = shutdown_sender.send(()) {
            done_receiver.await?;
        } else {
            log::warn!("couldn't send shutdown signal and receive done signal, killing browser anyway...");
        }
        // For some reason browser.close() logs an error about the websocket connection, so we rely
        // on drop (explicit here so that it's clear) cleaning up the Chrome process.
        //
        // Reported here: https://github.com/mattsse/chromiumoxide/issues/287
        drop(browser);

        Ok(())
    }

    pub async fn next_event(&mut self) -> Option<BrowserEvent> {
        match self.receiver.recv().await {
            Ok(event) => Some(event),
            Err(RecvError::Closed) => None,
            Err(error) => Some(BrowserEvent::Error(Arc::new(anyhow!(error)))),
        }
    }

    pub async fn request_state(&mut self) {
        let _ = self.inner_events_sender.send(InnerEvent::StateRequested);
    }

    pub async fn apply(&mut self, action: BrowserAction) -> Result<()> {
        self.actions_sender.send(action)?;
        Ok(())
    }
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

    let events_frame_requested_navigation = Box::pin(
        context
            .page
            .event_listener::<page::EventFrameRequestedNavigation>()
            .await?
            .map(|nav| {
                InnerEvent::FrameRequestedNavigation(
                    nav.frame_id.clone(),
                    nav.reason.clone(),
                    nav.url.clone(),
                )
            }),
    ) as InnerEventStream;

    let events_frame_navigated = Box::pin(
        context
            .page
            .event_listener::<page::EventFrameNavigated>()
            .await?
            .map(|nav| {
                InnerEvent::FrameNavigated(
                    nav.frame.id.clone(),
                    nav.r#type.clone(),
                )
            }),
    ) as InnerEventStream;

    let events_target_destroyed = Box::pin(
        context
            .page
            .event_listener::<target::EventTargetDestroyed>()
            .await?
            .map(|event| InnerEvent::TargetDestroyed(event.target_id.clone())),
    ) as InnerEventStream;

    let events_node_inserted = Box::pin(
        context
            .page
            .event_listener::<dom::EventChildNodeInserted>()
            .await?
            .map(|event| {
                InnerEvent::NodeTreeModified(
                    NodeModification::ChildNodeInserted {
                        parent: event.parent_node_id,
                        child: event.node.clone(),
                    },
                )
            }),
    ) as InnerEventStream;

    let events_node_count_updated = Box::pin(
        context
            .page
            .event_listener::<dom::EventChildNodeCountUpdated>()
            .await?
            .map(|event| {
                InnerEvent::NodeTreeModified(
                    NodeModification::ChildNodeCountUpdated {
                        parent: event.node_id,
                        count: event.child_node_count as u64,
                    },
                )
            }),
    ) as InnerEventStream;

    let events_node_removed = Box::pin(
        context
            .page
            .event_listener::<dom::EventChildNodeRemoved>()
            .await?
            .map(|event| {
                InnerEvent::NodeTreeModified(
                    NodeModification::ChildNodeRemoved {
                        parent: event.parent_node_id,
                        child: event.node_id,
                    },
                )
            }),
    ) as InnerEventStream;

    let events_attribute_modified = Box::pin(
        context
            .page
            .event_listener::<dom::EventAttributeModified>()
            .await?
            .map(|event| {
                InnerEvent::NodeTreeModified(
                    NodeModification::AttributeModified {
                        node: event.node_id,
                        name: event.name.clone(),
                        value: event.value.clone(),
                    },
                )
            }),
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

    let events_action_applied = Box::pin(
        receiver_to_stream(context.actions_sender.subscribe())
            .map(|action| InnerEvent::ActionApplied(action)),
    );

    Ok(Box::pin(stream::select_all(vec![
        events_loaded,
        events_paused,
        events_resumed,
        events_frame_requested_navigation,
        events_frame_navigated,
        events_target_destroyed,
        events_node_inserted,
        events_node_count_updated,
        events_node_removed,
        events_attribute_modified,
        events_console,
        events_action_applied,
    ])))
}

fn run_state_machine(
    mut context: BrowserContext,
    mut events: impl stream::Stream<Item = InnerEvent> + Send + Unpin + 'static,
    done_sender: oneshot::Sender<()>,
) {
    spawn(async move {
        let result = (async || {
            let mut state_current = InnerState::Running(vec![]);
            log::info!("processing events");
            loop {
                select! {
                    _ = &mut context.shutdown_receiver => {
                        log::debug!("shutting down browser state machine");
                        break;
                    },
                    event = events.next() => match event {
                        Some(event) => {
                            state_current = process_event(&context, state_current, event).await?;
                        }
                        None => {
                            log::debug!("no more events, shutting down state machine loop");
                            break;
                        }
                    }
                }
            }
            let _ = done_sender.send(());
            Ok::<(), anyhow::Error>(())
        })().await;
        if let Err(error) = result {
            context
                .sender
                .send(BrowserEvent::Error(Arc::new(anyhow!(
                    "error when processing event: {:?}",
                    error
                ))))
                .expect("send state machine event failed");
        }
    });
}

async fn process_event(
    context: &BrowserContext,
    state_current: InnerState,
    event: InnerEvent,
) -> Result<InnerState> {
    Ok(match (state_current, event) {
        (InnerState::Running(console_entries), InnerEvent::StateRequested) => {
            let _ = spawn(pause(context.page.clone()));
            InnerState::Pausing(console_entries)
        }
        (
            InnerState::Running(console_entries),
            InnerEvent::NodeTreeModified(modification),
        ) => {
            handle_node_modification(&context, &modification).await?;
            let _ = spawn(pause(context.page.clone()));
            InnerState::Pausing(console_entries)
        }
        (state, InnerEvent::StateRequested) => {
            log::debug!(
                "cannot request new browser state when in state {:?}, ignoring",
                &state
            );
            state
        }
        (state, InnerEvent::NodeTreeModified(modification)) => {
            handle_node_modification(&context, &modification).await?;
            state
        }
        (
            state,
            InnerEvent::Paused {
                reason,
                exception,
                call_frame_id,
            },
        ) => {
            let console_entries = match &state {
                InnerState::Pausing(console_entries) => console_entries.clone(),
                InnerState::Paused => vec![],
                InnerState::Resuming(_) => vec![],
                InnerState::Navigating => vec![],
                InnerState::Loading(console_entries) => console_entries.clone(),
                InnerState::Running(console_entries) => console_entries.clone(),
            };
            let exception = match reason {
                debugger::PausedReason::Exception => {
                    if let Some(json::Value::Object(object)) = exception {
                        object
                            .get("description")
                            .map(|value| value.clone())
                            .or(Some(json::Value::Object(object)))
                            .map(Exception::UncaughtException)
                    } else {
                        bail!("unexpected exception data: {:?}", &exception)
                    }
                }
                debugger::PausedReason::PromiseRejection => {
                    if let Some(json::Value::Object(object)) = exception {
                        object
                            .get("value")
                            .or(object.get("description"))
                            .map(|value| value.clone())
                            .or(Some(json::Value::Object(object)))
                            .map(Exception::UnhandledPromiseRejection)
                    } else {
                        bail!(
                            "unexpected promise rejection data: {:?}",
                            &exception
                        )
                    }
                }
                debugger::PausedReason::Other => None,
                other => {
                    bail!(
                        "unexpected pause reason {:?} when in state: {:?}",
                        other,
                        &state
                    )
                }
            };

            let call_frame_id = call_frame_id
                .ok_or(anyhow!("no call frame id at breakpoint"))?;
            let browser_state = BrowserState::current(
                context.page.clone(),
                &call_frame_id,
                console_entries,
                exception,
            )
            .await?;

            context
                .sender
                .send(BrowserEvent::StateChanged(browser_state))?;

            InnerState::Paused
        }
        (InnerState::Paused, InnerEvent::ActionApplied(browser_action)) => {
            context
                .page
                .execute(debugger::ResumeParams::builder().build())
                .await?;
            InnerState::Resuming(browser_action)
        }
        (InnerState::Running(_), InnerEvent::Resumed) => {
            log::warn!("running + resumed");
            InnerState::Running(vec![])
        }
        (InnerState::Resuming(browser_action), InnerEvent::Resumed) => {
            let action = browser_action.clone();
            let page = context.page.clone();
            // We can't block on running the action, in case it synchronously
            // throws an uncaught exception blocking the evaluation indefinitely.
            // This gives us a chance to receive the "Debugger.paused" event and
            // resume (extracting the uncaught exception information).
            spawn(async move {
                log::debug!("applying: {:?}", browser_action);
                match action.apply(&page).await {
                    Ok(_) => {}
                    Err(err) => {
                        log::error!(
                            "failed to apply action {:?}: {:?}",
                            action,
                            err
                        )
                    }
                }
            });
            InnerState::Running(vec![])
        }
        (state, InnerEvent::Loaded) => {
            // We *should* only get the `Loaded` event when we're in `Loading`, but for some reason,
            // maybe something Chrome-related, we sometimes see it in `Initial` and `Navigating`
            // too. Maybe some race or that events are dropped.
            let console_entries = match &state {
                InnerState::Pausing(console_entries) => console_entries.clone(),
                InnerState::Paused => vec![],
                InnerState::Resuming(_) => vec![],
                InnerState::Navigating => vec![],
                InnerState::Loading(console_entries) => console_entries.clone(),
                InnerState::Running(console_entries) => console_entries.clone(),
            };
            context
                .inner_events_sender
                .send(InnerEvent::StateRequested)?;
            InnerState::Running(console_entries)
        }
        (
            state,
            InnerEvent::FrameRequestedNavigation(frame_id, reason, url),
        ) => {
            if frame_id == context.frame_id {
                log::debug!(
                    "navigating to {} due to {:?} (current state is {:?})",
                    url,
                    reason,
                    state
                );
                InnerState::Navigating
            } else {
                state
            }
        }
        (
            InnerState::Loading(mut console_entries),
            InnerEvent::ConsoleEntry(entry),
        ) => {
            console_entries.push(entry);
            InnerState::Loading(console_entries)
        }
        (
            InnerState::Running(mut console_entries),
            InnerEvent::ConsoleEntry(entry),
        ) => {
            console_entries.push(entry);
            InnerState::Running(console_entries)
        }
        (
            InnerState::Pausing(mut console_entries),
            InnerEvent::ConsoleEntry(entry),
        ) => {
            console_entries.push(entry);
            InnerState::Pausing(console_entries)
        }
        (InnerState::Navigating, InnerEvent::ConsoleEntry(_)) => {
            InnerState::Navigating
        }
        (state, InnerEvent::FrameNavigated(frame_id, navigation_type)) => {
            // Track all nodes.
            context
                .page
                .execute(
                    dom::GetDocumentParams::builder()
                        .depth(-1)
                        .pierce(true)
                        .build(),
                )
                .await?;
            if frame_id == context.frame_id {
                match navigation_type {
                    NavigationType::Navigation => InnerState::Loading(vec![]),
                    // Navigating history with bfcache doesn't yield a "loaded"
                    // event so we jump straight into `Running`.
                    NavigationType::BackForwardCacheRestore => {
                        context
                            .inner_events_sender
                            .send(InnerEvent::StateRequested)?;
                        InnerState::Running(vec![])
                    }
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
        (state, event) => {
            bail!("unhandled transition: {:?} + {:?}", state, event);
        }
    })
}

async fn handle_node_modification(
    context: &BrowserContext,
    modification: &NodeModification,
) -> Result<()> {
    match modification {
        NodeModification::ChildNodeInserted { parent, .. } => {
            context
                .page
                .execute(dom::RequestChildNodesParams::new(parent.clone()))
                .await?;
        }
        NodeModification::ChildNodeCountUpdated { parent, .. } => {
            context
                .page
                .execute(dom::RequestChildNodesParams::new(parent.clone()))
                .await?;
        }
        NodeModification::ChildNodeRemoved { .. } => {}
        NodeModification::AttributeModified { .. } => {}
    }
    Ok(())
}

fn receiver_to_stream<T: Clone + Send + 'static>(
    receiver: Receiver<T>,
) -> Pin<Box<dyn stream::Stream<Item = T> + Send>> {
    Box::pin(BroadcastStream::new(receiver).filter_map(async |r| {
        if let Ok(x) = r {
            Some(x)
        } else {
            None
        }
    }))
}

async fn pause(page: Arc<Page>) -> Result<()> {
    page.evaluate_function("function () { debugger; }")
        .await
        .map_err(|err| anyhow!(err).context("evaluate function call failed"))?;
    Ok(())
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
    apply_sandbox(BrowserConfig::builder())
        .headless_mode(if launch_options.headless {
            HeadlessMode::New
        } else {
            HeadlessMode::False
        })
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
