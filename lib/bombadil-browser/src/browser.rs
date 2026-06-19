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
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};
use tempfile::TempDir;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::broadcast::{Receiver, Sender, channel};
use tokio::sync::{mpsc, oneshot};
use tokio::task::AbortHandle;
use tokio::time::sleep;
use tokio::{select, spawn};
use tokio_stream::wrappers::BroadcastStream;
use url::Url;

use crate::browser::actions::{ActionOptions, BrowserAction};
use crate::browser::state::{
    BrowserState, CallFrame, ConsoleEntry, Exception, Screenshot,
    ScreenshotFormat,
};
use crate::url::is_within_domain;

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
    /// A new browser target (tab/window) appeared. Carries its opener so we can
    /// tell whether the tab we are currently driving spawned it.
    TargetCreated {
        target_id: TargetId,
        opener_id: Option<TargetId>,
    },
    /// A tab opened by the active page has committed to an in-domain URL and is
    /// ready to be driven; switch the state machine onto it.
    FollowTab {
        page: Arc<Page>,
        opener_id: Option<TargetId>,
    },
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

/// Ceiling on simultaneously-open tabs; over this we close the oldest tab that
/// isn't active or on the fallback stack, so a tab-spawning page can't run away.
const MAX_OPEN_TABS: usize = 20;

/// Max depth of the opener fallback stack. Below [`MAX_OPEN_TABS`] so orphan
/// tabs still have room to be reclaimed before the stack fills up.
const MAX_FOLLOW_DEPTH: usize = 8;

/// How long to wait for a freshly-opened tab to commit to a real (non-blank)
/// URL before giving up on following it.
const FOLLOW_NAVIGATION_TIMEOUT: Duration = Duration::from_secs(10);

/// Commands for the task that owns the [`chromiumoxide::Browser`] handle, which
/// isn't `Clone` and whose `close` needs `&mut`, so one task serializes access.
enum ActorCommand {
    GetPage {
        target_id: TargetId,
        reply: oneshot::Sender<Result<Page>>,
    },
    CloseTarget {
        target_id: TargetId,
    },
    Close {
        reply: oneshot::Sender<()>,
    },
}

/// Aborts a tab's spawned tasks when dropped, so a tab we no longer drive stops
/// feeding the state machine.
struct ListenerGuard {
    handles: Vec<AbortHandle>,
}

impl Drop for ListenerGuard {
    fn drop(&mut self) {
        for handle in &self.handles {
            handle.abort();
        }
    }
}

/// Everything tied to a single tab we are actively driving.
struct PageSession {
    page: Arc<Page>,
    frame_id: FrameId,
    network_activity: activity::NetworkActivity,
    screencast_activity: activity::ScreencastActivity,
    opener_id: Option<TargetId>,
    _guard: ListenerGuard,
}

/// A suspended opener tab we can fall back to when the tab we followed into is
/// closed. Listeners are torn down while suspended and rebuilt on fallback.
struct StackedTab {
    page: Arc<Page>,
    opener_id: Option<TargetId>,
}

/// Insertion-ordered set of every open page target we know about, used to bound
/// the total number of tabs.
#[derive(Default)]
struct TabRegistry {
    order: Vec<TargetId>,
}

impl TabRegistry {
    fn insert(&mut self, target_id: TargetId) {
        if !self.order.contains(&target_id) {
            self.order.push(target_id);
        }
    }

    fn remove(&mut self, target_id: &TargetId) {
        self.order.retain(|id| id != target_id);
    }

    fn len(&self) -> usize {
        self.order.len()
    }
}

struct BrowserContext {
    sender: Sender<BrowserEvent>,
    inner_events_sender: Sender<InnerEvent>,
    shutdown_receiver: oneshot::Receiver<()>,
    /// The tab currently being driven. Swapped when we follow into a new tab or
    /// fall back to an opener.
    active: Arc<Mutex<PageSession>>,
    /// Openers we can fall back to, bottom-to-top (index 0 is the root tab).
    opener_stack: Arc<Mutex<Vec<StackedTab>>>,
    tabs: Arc<Mutex<TabRegistry>>,
    actor: mpsc::UnboundedSender<ActorCommand>,
    /// Scripts re-injected into every tab we drive (e.g. the specification
    /// bundle), so followed tabs behave like the initial one.
    document_scripts: Arc<Mutex<Vec<String>>>,
    latest_frame: Arc<Mutex<Option<Arc<[u8]>>>>,
    origin: Url,
    browser_options: BrowserOptions,
}

impl BrowserContext {
    fn page(&self) -> Arc<Page> {
        self.active.lock().unwrap().page.clone()
    }

    fn frame_id(&self) -> FrameId {
        self.active.lock().unwrap().frame_id.clone()
    }

    fn active_target_id(&self) -> TargetId {
        self.active.lock().unwrap().page.target_id().clone()
    }

    fn activity_stream(&self) -> activity::ActivityStream {
        let active = self.active.lock().unwrap();
        Box::pin(stream::select(
            active.network_activity.stream(),
            active.screencast_activity.stream(),
        ))
    }
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
    actor: mpsc::UnboundedSender<ActorCommand>,
    document_scripts: Arc<Mutex<Vec<String>>>,
    page: Arc<Page>,
    origin: Url,
    go_to_origin_on_init: bool,
}

impl Drop for Browser {
    fn drop(&mut self) {
        if let Some(sender) = self.shutdown_sender.take() {
            let _else = sender.send(());
        }
        // Dropping the last command sender lets the actor task exit and drop the
        // browser as a last resort; terminate() is the clean path.
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

        let (inner_events_sender, inner_events_receiver) =
            channel::<InnerEvent>(1024);

        let (shutdown_sender, shutdown_receiver) = oneshot::channel::<()>();
        let (done_sender, done_receiver) = oneshot::channel::<()>();

        let latest_frame: Arc<Mutex<Option<Arc<[u8]>>>> =
            Arc::new(Mutex::new(None));
        let document_scripts: Arc<Mutex<Vec<String>>> =
            Arc::new(Mutex::new(Vec::new()));

        let initial_session = setup_page(
            page.clone(),
            &browser_options,
            &latest_frame,
            &inner_events_sender,
            &[],
            None,
        )
        .await?;

        let mut tabs = TabRegistry::default();
        tabs.insert(page.target_id().clone());

        // Browser-level target events must be wired up while we still hold the
        // browser handle, before it is moved into the actor task below.
        let target_destroyed = browser
            .event_listener::<target::EventTargetDestroyed>()
            .await?
            .map(|event| InnerEvent::TargetDestroyed(event.target_id.clone()));
        let target_created = browser
            .event_listener::<target::EventTargetCreated>()
            .await?
            .filter_map(|event| async move {
                let info = &event.target_info;
                if info.r#type != "page" {
                    return None;
                }
                Some(InnerEvent::TargetCreated {
                    target_id: info.target_id.clone(),
                    opener_id: info.opener_id.clone(),
                })
            });

        let (actor, mut actor_commands) =
            mpsc::unbounded_channel::<ActorCommand>();
        spawn(async move {
            while let Some(command) = actor_commands.recv().await {
                match command {
                    ActorCommand::GetPage { target_id, reply } => {
                        let _ = reply.send(
                            browser
                                .get_page(target_id)
                                .await
                                .map_err(anyhow::Error::from),
                        );
                    }
                    ActorCommand::CloseTarget { target_id } => {
                        if let Err(error) = browser
                            .execute(target::CloseTargetParams::new(target_id))
                            .await
                        {
                            log::debug!("close target failed: {:?}", error);
                        }
                    }
                    ActorCommand::Close { reply } => {
                        if let Err(error) = browser.close().await {
                            log::warn!("browser close error: {:?}", error);
                        }
                        let _ = reply.send(());
                        break;
                    }
                }
            }
        });

        let context = BrowserContext {
            sender,
            inner_events_sender: inner_events_sender.clone(),
            shutdown_receiver,
            active: Arc::new(Mutex::new(initial_session)),
            opener_stack: Arc::new(Mutex::new(Vec::new())),
            tabs: Arc::new(Mutex::new(tabs)),
            actor: actor.clone(),
            document_scripts: document_scripts.clone(),
            latest_frame,
            origin: origin.clone(),
            browser_options: browser_options.clone(),
        };

        let events_all = stream::select_all(vec![
            receiver_to_stream(inner_events_receiver),
            Box::pin(
                receiver_to_stream(actions_sender.subscribe())
                    .map(InnerEvent::ActionAccepted),
            )
                as Pin<Box<dyn stream::Stream<Item = InnerEvent> + Send>>,
            Box::pin(target_destroyed),
            Box::pin(target_created),
        ]);
        run_state_machine(context, events_all, done_sender);

        Ok(Browser {
            actor,
            document_scripts,
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

        // Close the browser before awaiting the state machine, so any in-flight
        // CDP call there fails fast instead of the two deadlocking on each other.
        let (close_reply, close_done) = oneshot::channel::<()>();
        if self
            .actor
            .send(ActorCommand::Close { reply: close_reply })
            .is_ok()
        {
            let _ = close_done.await;
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
        // Remember the script so tabs we follow into later get it too.
        self.document_scripts
            .lock()
            .unwrap()
            .push(script.to_string());
        inject_document_script(&self.page, script).await
    }
}

/// Register `script` to run on every future document of `page` and evaluate it
/// once against the current document.
async fn inject_document_script(page: &Page, script: &str) -> Result<()> {
    let _ = page.evaluate_on_new_document(script).await?;

    let main_execution_context_id = page
        .execution_context()
        .await?
        .ok_or(anyhow!("no execution context available"))?;
    let _ = page
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

/// Auto-accept JavaScript dialogs so they never block the test run; the returned
/// handle stops the task when the tab is no longer driven.
async fn auto_accept_dialogs(page: Arc<Page>) -> Result<AbortHandle> {
    let mut events = page
        .event_listener::<page::EventJavascriptDialogOpening>()
        .await?;
    let handle = spawn(async move {
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
    })
    .abort_handle();
    Ok(handle)
}

/// Prepare a tab for driving (enable CDP domains, re-inject scripts, subscribe to
/// activity, spawn forwarders); dropping the returned session tears it all down.
async fn setup_page(
    page: Arc<Page>,
    browser_options: &BrowserOptions,
    latest_frame: &Arc<Mutex<Option<Arc<[u8]>>>>,
    inner_events_sender: &Sender<InnerEvent>,
    document_scripts: &[String],
    opener_id: Option<TargetId>,
) -> Result<PageSession> {
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
            .device_scale_factor(browser_options.emulation.device_scale_factor)
            .mobile(false)
            .scale(1)
            .build()
            .map_err(|err| {
                anyhow!(err)
                    .context("build SetDeviceMetricsOverrideParams failed")
            })?,
    )
    .await?;

    for script in document_scripts {
        inject_document_script(&page, script).await?;
    }

    let frame_id = page
        .mainframe()
        .await?
        .ok_or(anyhow!("no main frame available"))?;

    let network_activity = activity::NetworkActivity::subscribe(&page).await?;
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

    let mut handles =
        spawn_page_listeners(&page, &frame_id, inner_events_sender).await?;

    // Keep the latest screencast frame updated for state capture.
    {
        let latest_frame = latest_frame.clone();
        let mut receiver = screencast.subscribe();
        let handle = spawn(async move {
            loop {
                match receiver.recv().await {
                    Ok(frame) => {
                        *latest_frame.lock().unwrap() = Some(frame);
                    }
                    Err(RecvError::Lagged(n)) => {
                        log::debug!(
                            "screencast frame receiver lagged by {}",
                            n
                        );
                    }
                    Err(RecvError::Closed) => break,
                }
            }
        })
        .abort_handle();
        handles.push(handle);
    }

    handles.push(auto_accept_dialogs(page.clone()).await?);

    instrumentation::instrument_js_coverage(
        page.clone(),
        browser_options.instrumentation.clone(),
    )
    .await?;

    Ok(PageSession {
        page,
        frame_id,
        network_activity,
        screencast_activity,
        opener_id,
        _guard: ListenerGuard { handles },
    })
}

/// Spawn a task per page-scoped CDP event that forwards [`InnerEvent`]s into
/// `sender`; the returned handles stop them when the session is dropped.
async fn spawn_page_listeners(
    page: &Arc<Page>,
    frame_id: &FrameId,
    sender: &Sender<InnerEvent>,
) -> Result<Vec<AbortHandle>> {
    let mut handles = Vec::new();

    macro_rules! forward {
        ($stream:expr, $map:expr) => {{
            let mut stream = Box::pin($stream);
            let sender = sender.clone();
            let map = $map;
            spawn(async move {
                while let Some(event) = stream.next().await {
                    if let Some(inner) = map(event) {
                        if sender.send(inner).is_err() {
                            break;
                        }
                    }
                }
            })
            .abort_handle()
        }};
    }

    handles.push(forward!(
        page.event_listener::<page::EventLoadEventFired>().await?,
        |_| Some(InnerEvent::Loaded)
    ));

    handles.push(forward!(
        page.event_listener::<debugger::EventPaused>().await?,
        |event: Arc<debugger::EventPaused>| Some(InnerEvent::Paused {
            reason: event.reason.clone(),
            exception: event.data.clone(),
            call_frame_id: event
                .call_frames
                .first()
                .map(|f| f.call_frame_id.clone()),
        })
    ));

    handles.push(forward!(
        page.event_listener::<debugger::EventResumed>().await?,
        |_| Some(InnerEvent::Resumed)
    ));

    handles.push(forward!(
        page.event_listener::<runtime::EventExceptionThrown>()
            .await?,
        |e: Arc<runtime::EventExceptionThrown>| Some(
            InnerEvent::ExceptionThrown(Exception {
                exception_id: e.exception_details.exception_id as u32,
                timestamp: UNIX_EPOCH
                    + Duration::from_secs_f64(*e.timestamp.inner() / 1000.0),
                text: e.exception_details.text.clone(),
                line: e.exception_details.line_number as u32,
                column: e.exception_details.column_number as u32,
                url: e.exception_details.url.clone(),
                remote_object: e.exception_details.exception.as_ref().map(
                    |obj| {
                        state::ExceptionRemoteObject {
                            type_name: format!("{:?}", obj.r#type),
                            subtype: obj
                                .subtype
                                .as_ref()
                                .map(|st| format!("{:?}", st)),
                            class_name: obj.class_name.clone(),
                            description: obj.description.clone(),
                            value: obj.value.clone(),
                        }
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
        )
    ));

    let target_frame = frame_id.clone();
    handles.push(forward!(
        page.event_listener::<page::EventFrameRequestedNavigation>()
            .await?,
        move |nav: Arc<page::EventFrameRequestedNavigation>| {
            (nav.frame_id == target_frame).then(|| {
                InnerEvent::FrameRequestedNavigation {
                    frame_id: nav.frame_id.clone(),
                    reason: nav.reason.clone(),
                    url: nav.url.clone(),
                }
            })
        }
    ));

    let target_frame = frame_id.clone();
    handles.push(forward!(
        page.event_listener::<page::EventFrameNavigated>().await?,
        move |nav: Arc<page::EventFrameNavigated>| {
            (nav.frame.id == target_frame).then(|| {
                InnerEvent::FrameNavigated(
                    nav.frame.id.clone(),
                    nav.r#type.clone(),
                )
            })
        }
    ));

    let target_frame = frame_id.clone();
    handles.push(forward!(
        page.event_listener::<browser::EventDownloadWillBegin>()
            .await?,
        move |event: Arc<browser::EventDownloadWillBegin>| {
            (event.frame_id == target_frame).then(|| {
                InnerEvent::DownloadWillBegin {
                    frame_id: event.frame_id.clone(),
                    url: event.url.clone(),
                }
            })
        }
    ));

    handles.push(forward!(
        page.event_listener::<runtime::EventConsoleApiCalled>()
            .await?,
        |call: Arc<runtime::EventConsoleApiCalled>| {
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
                    + Duration::from_secs_f64(*call.timestamp.inner() / 1000.0),
                level,
                args: call.args.iter().map(remote_object_to_json).collect(),
            }))
        }
    ));

    Ok(handles)
}

/// Wait for an opened tab to commit a URL, then follow it if it's in-domain;
/// out-of-domain or never-committed tabs are closed instead.
async fn await_followable_tab(
    target_id: TargetId,
    opener_id: Option<TargetId>,
    origin: Url,
    actor: mpsc::UnboundedSender<ActorCommand>,
    inner_events_sender: Sender<InnerEvent>,
) {
    // A new target isn't resolvable until the CDP host attaches it, and only
    // then navigates off about:blank, so poll for both within a timeout.
    let resolved = tokio::time::timeout(FOLLOW_NAVIGATION_TIMEOUT, async {
        let page = loop {
            let (reply_sender, reply_receiver) = oneshot::channel();
            if actor
                .send(ActorCommand::GetPage {
                    target_id: target_id.clone(),
                    reply: reply_sender,
                })
                .is_err()
            {
                return None;
            }
            if let Ok(Ok(page)) = reply_receiver.await {
                break Arc::new(page);
            }
            sleep(Duration::from_millis(100)).await;
        };

        loop {
            if let Ok(Some(url)) = page.url().await
                && url != "about:blank"
                && !url.is_empty()
            {
                return Some((page, url));
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await;

    let Ok(Some((page, url_string))) = resolved else {
        log::debug!(
            "opened tab {:?} never became a followable page; not following",
            target_id
        );
        let _ = actor.send(ActorCommand::CloseTarget { target_id });
        return;
    };

    match Url::parse(&url_string) {
        Ok(url) if is_within_domain(&url, &origin) => {
            log::info!("following opened tab to {}", url);
            let _ = inner_events_sender
                .send(InnerEvent::FollowTab { page, opener_id });
        }
        _ => {
            log::debug!(
                "opened tab navigated out of domain ({}); leaving the \
                 current tab in place",
                url_string
            );
            let _ = actor.send(ActorCommand::CloseTarget { target_id });
        }
    }
}

/// Close the oldest tabs that are neither active nor on the opener stack until
/// we are back under [`MAX_OPEN_TABS`]. Guards against tab-spawning pages.
fn enforce_tab_budget(context: &BrowserContext) {
    let protected: HashSet<TargetId> = {
        let stack = context.opener_stack.lock().unwrap();
        std::iter::once(context.active_target_id())
            .chain(stack.iter().map(|tab| tab.page.target_id().clone()))
            .collect()
    };

    let mut tabs = context.tabs.lock().unwrap();
    while tabs.len() > MAX_OPEN_TABS {
        let victim = tabs
            .order
            .iter()
            .find(|id| !protected.contains(*id))
            .cloned();
        match victim {
            Some(target_id) => {
                tabs.remove(&target_id);
                log::debug!("tab budget exceeded; closing {:?}", target_id);
                let _ =
                    context.actor.send(ActorCommand::CloseTarget { target_id });
            }
            // Everything left is protected; the opener-stack depth cap keeps
            // the protected set itself bounded.
            None => break,
        }
    }
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
                .page()
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
                context.page(),
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
                .page()
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
            let page = context.page();
            let sender = context.inner_events_sender.clone();
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
            spawn(async move {
                log::debug!("applying: {:?}", browser_action);
                match browser_action.apply(&page, action_options).await {
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
            let subscription = quiescence::subscribe(context.activity_stream());
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
            if frame_id == context.frame_id() {
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
        ) if frame_id == context.frame_id() => {
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
            if frame_id == context.frame_id() {
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
        (
            state,
            InnerEvent::TargetCreated {
                target_id,
                opener_id,
            },
        ) => {
            context.tabs.lock().unwrap().insert(target_id.clone());
            enforce_tab_budget(context);

            if opener_id.as_ref() == Some(&context.active_target_id()) {
                // The tab we're driving spawned this one; once it commits to a
                // URL we decide whether to follow it.
                spawn(await_followable_tab(
                    target_id,
                    opener_id,
                    context.origin.clone(),
                    context.actor.clone(),
                    context.inner_events_sender.clone(),
                ));
            } else if opener_id.is_none() {
                log::debug!(
                    "ignoring tab {:?} opened with no opener (e.g. \
                     rel=noopener); cannot attribute it to the page under test",
                    target_id
                );
            }
            state
        }
        (state, InnerEvent::FollowTab { page, opener_id }) => {
            let scripts = context.document_scripts.lock().unwrap().clone();
            // The tab can close before we attach; if setup fails, keep driving
            // the current tab instead of ending the run.
            let new_session = match setup_page(
                page,
                &context.browser_options,
                &context.latest_frame,
                &context.inner_events_sender,
                &scripts,
                opener_id,
            )
            .await
            {
                Ok(session) => session,
                Err(error) => {
                    log::warn!(
                        "failed to set up opened tab; not following: {:?}",
                        error
                    );
                    return Ok(state);
                }
            };

            // Suspend the current tab and remember it so we can fall back when
            // the followed tab closes.
            let old = {
                let mut active = context.active.lock().unwrap();
                std::mem::replace(&mut *active, new_session)
            };
            let old_page = old.page.clone();
            let old_opener = old.opener_id.clone();
            drop(old);

            let stop_page = old_page.clone();
            spawn(async move {
                let _ = stop_page
                    .execute(page::StopScreencastParams::default())
                    .await;
            });

            {
                let mut stack = context.opener_stack.lock().unwrap();
                stack.push(StackedTab {
                    page: old_page,
                    opener_id: old_opener,
                });
                // Keep the root (index 0) and the most recent openers.
                while stack.len() > MAX_FOLLOW_DEPTH {
                    let dropped = stack.remove(1);
                    let _ = context.actor.send(ActorCommand::CloseTarget {
                        target_id: dropped.page.target_id().clone(),
                    });
                }
            }

            let shared = InnerStateShared {
                generation: state.shared.generation.next(),
                ..Default::default()
            };
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
        (state, InnerEvent::TargetDestroyed(target_id)) => {
            context.tabs.lock().unwrap().remove(&target_id);

            if target_id != context.active_target_id() {
                // A background or opener tab went away; forget it so we never
                // try to fall back onto a closed tab.
                context
                    .opener_stack
                    .lock()
                    .unwrap()
                    .retain(|tab| *tab.page.target_id() != target_id);
                return Ok(state);
            }

            // The tab we're driving closed; fall back to its opener if we kept
            // one alive, otherwise the run is over.
            let fallback = context.opener_stack.lock().unwrap().pop();
            match fallback {
                Some(stacked) => {
                    log::info!(
                        "followed tab {:?} closed; falling back to opener",
                        target_id
                    );
                    let scripts =
                        context.document_scripts.lock().unwrap().clone();
                    let session = setup_page(
                        stacked.page,
                        &context.browser_options,
                        &context.latest_frame,
                        &context.inner_events_sender,
                        &scripts,
                        stacked.opener_id,
                    )
                    .await?;
                    *context.active.lock().unwrap() = session;

                    let shared = InnerStateShared {
                        generation: state.shared.generation.next(),
                        ..Default::default()
                    };
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
                None => {
                    bail!("page target {:?} was destroyed", target_id)
                }
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
    let subscription = quiescence::subscribe(context.activity_stream());
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

    let page = context.page();
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

    let page = context.page();
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
