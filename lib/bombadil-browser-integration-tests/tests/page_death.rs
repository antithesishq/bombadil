//! Runs whose last action no state follows: it kills the page, the
//! browser, or bombadil is killed. The trace must hold it; each run also
//! checks, on every `apply`, that the trace file has the action already.

use std::{
    collections::VecDeque,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    os::unix::process::{CommandExt, ExitStatusExt},
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Once, OnceLock,
        atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, SystemTime},
};

use anyhow::{Context, Result};
use bombadil::{
    driver::{
        DriverEvent, InterfaceDriver, InterfaceSession, RunId, TraceWriter,
    },
    model::{Picked, Recorded},
    runner::{ControlFlow, PropertiesState, PropertyViolation, RunStrategy},
    shrink::load_trace_actions,
    specification::{bundler::bundle, domain::Snapshot},
    tree::{Leaves, Tree},
};
use bombadil_browser::{
    browser::{
        actions::{BrowserAction as Applied, BrowserActionTemplate},
        state::BrowserState,
    },
    browser_options::{BrowserOptions, Emulation, VirtualTimePolicy},
    chromium::locate::{self, ChromeVariant},
    convert::{ToInternal, ToSchema},
    driver::{BrowserDriver, BrowserSession, DebuggerOptions},
    instrumentation::BlockRegistry,
    match_pattern::MatchPattern,
    strategy::{ExitReason, TestMode, TestResult, TestStrategy},
    trace::writer::FileTraceWriter,
};
use bombadil_schema::{
    Time,
    browser::{BrowserAction, BrowserTraceEntry},
};
use bombadil_trace::{Record, StepError, open_trace};
use cdp::{
    Connection,
    cdp_protocol::cdp::browser_protocol::{page, target},
};
use rand::SeedableRng;
use serde::de::IgnoredAny;
use tempfile::TempDir;
use url::Url;

const FIXTURE: &str = include_str!("page-death/index.html");

/// The stall timeout would end a run too, but only after a minute.
const INTERRUPT_DELAY: Duration = Duration::from_secs(1);

const CHILD_ORIGIN: &str = "BOMBADIL_PAGE_DEATH_ORIGIN";
const CHILD_REMOTE_DEBUGGER: &str = "BOMBADIL_PAGE_DEATH_REMOTE_DEBUGGER";
const CHILD_TRACE: &str = "BOMBADIL_PAGE_DEATH_TRACE";

fn setup() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let env = env_logger::Env::default().default_filter_or("debug");
        env_logger::Builder::from_env(env)
            .format_timestamp_millis()
            .format_target(true)
            .is_test(true)
            .filter_module("html5ever", log::LevelFilter::Warn)
            .init();
    });
}

struct Chrome {
    child: Child,
    remote_debugger: Url,
    web_socket: String,
    _user_data_directory: TempDir,
}

impl Chrome {
    fn launch() -> Self {
        let variant =
            match std::env::var("BOMBADIL_TEST_CHROME_VARIANT").as_deref() {
                Ok("headless-shell") => ChromeVariant::HeadlessShell,
                Ok("full") | Err(_) => ChromeVariant::Full,
                Ok(other) => {
                    panic!("unknown BOMBADIL_TEST_CHROME_VARIANT: {other}")
                }
            };
        let user_data_directory = TempDir::with_prefix("page_death_").unwrap();
        let mut child = Command::new(locate::executable(variant).unwrap())
            .args([
                "--headless",
                "--no-sandbox",
                "--disable-setuid-sandbox",
                "--disable-dev-shm-usage",
                "--no-crashpad",
                "--disable-crash-reporter",
                "--disable-background-networking",
                "--remote-debugging-port=0",
            ])
            .arg(format!(
                "--user-data-dir={}",
                user_data_directory.path().display()
            ))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .unwrap();
        let stderr = BufReader::new(child.stderr.take().unwrap());
        let (listening, listening_receiver) = mpsc::channel();
        thread::spawn(move || {
            for line in stderr.lines().map_while(Result::ok) {
                if let Some(url) = line.strip_prefix("DevTools listening on ") {
                    let _ = listening.send(url.to_string());
                }
            }
        });
        let web_socket = listening_receiver
            .recv_timeout(Duration::from_secs(30))
            .expect("the browser never listened for a debugger");
        let port = Url::parse(&web_socket).unwrap().port().unwrap();
        Chrome {
            child,
            remote_debugger: Url::parse(&format!("http://127.0.0.1:{port}"))
                .unwrap(),
            web_socket,
            _user_data_directory: user_data_directory,
        }
    }

    fn pid(&self) -> libc::pid_t {
        self.child.id() as libc::pid_t
    }
}

fn kill_browser(pid: libc::pid_t) {
    // SAFETY: the browser leads its own process group.
    unsafe { libc::killpg(pid, libc::SIGKILL) };
}

impl Drop for Chrome {
    fn drop(&mut self) {
        kill_browser(self.pid());
        let _ = self.child.wait();
    }
}

struct Renderer {
    connection: Connection,
    session: target::SessionId,
    crashed: crossbeam_channel::Receiver<target::EventTargetCrashed>,
}

impl Renderer {
    /// Attached while the page loads: a session attached while the page
    /// is blocked in the click of "Die" wouldn't reach its renderer.
    fn attach(web_socket: &str, origin: &Url) -> Result<Self> {
        let connection = Connection::connect(web_socket)?;
        connection.send(
            target::SetDiscoverTargetsParams {
                discover: true,
                filter: None,
            },
            None,
        )?;
        let targets = connection
            .send(target::GetTargetsParams { filter: None }, None)?
            .target_infos;
        let page = targets
            .iter()
            .find(|target| target.url.starts_with(origin.as_str()))
            .with_context(|| format!("no page at {origin} in {targets:?}"))?;
        let session = connection
            .send(
                target::AttachToTargetParams {
                    target_id: page.target_id.clone(),
                    flatten: Some(true),
                },
                None,
            )?
            .session_id;
        let (crashed_sender, crashed) = crossbeam_channel::unbounded();
        let crashes =
            connection.events.subscribe::<target::EventTargetCrashed>();
        thread::spawn(move || {
            while let Ok(Some(crash)) = crashes.next() {
                let _ = crashed_sender.send(crash);
            }
        });
        Ok(Renderer {
            connection,
            session,
            crashed,
        })
    }

    fn crash(&self) -> Result<()> {
        drop(
            self.connection
                .request(page::CrashParams {}, Some(&self.session))?,
        );
        let crash = self
            .crashed
            .recv_timeout(Duration::from_secs(30))
            .context("the renderer didn't crash")?;
        log::info!("the renderer crashed: {crash:?}");
        Ok(())
    }
}

struct Handlers<Ready, Die> {
    on_ready: Ready,
    on_die: Die,
}

fn serve(
    handlers: Handlers<
        impl Fn() + Send + Sync + 'static,
        impl Fn() + Send + Sync + 'static,
    >,
) -> Url {
    fn respond(stream: TcpStream, handlers: &Handlers<impl Fn(), impl Fn()>) {
        let mut reader = BufReader::new(stream);
        let mut request_line = String::new();
        if reader.read_line(&mut request_line).is_err() {
            return;
        }
        let mut header = String::new();
        while reader.read_line(&mut header).is_ok_and(|read| read > 2) {
            header.clear();
        }
        let (status, content_type, body) = match request_line.split(' ').nth(1)
        {
            Some("/die") => {
                (handlers.on_die)();
                ("200 OK", "text/plain", "dead")
            }
            Some("/ready.js") => {
                (handlers.on_ready)();
                ("200 OK", "text/javascript", "// ready")
            }
            Some("/") => ("200 OK", "text/html", FIXTURE),
            _ => ("404 Not Found", "text/plain", "not found"),
        };
        let _ = write!(
            reader.get_mut(),
            "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        // Until the client closes, so the response isn't reset.
        let _ = reader.read(&mut [0; 64]);
    }

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let handlers = Arc::new(handlers);
    thread::spawn(move || {
        for stream in listener.incoming().map_while(Result::ok) {
            let handlers = handlers.clone();
            thread::spawn(move || respond(stream, &handlers));
        }
    });
    Url::parse(&format!("http://localhost:{port}/")).unwrap()
}

struct Applying<OnApply> {
    session: BrowserSession,
    on_apply: OnApply,
}

impl<OnApply: FnMut(&Applied)> InterfaceSession for Applying<OnApply> {
    type Action = Applied;
    type ActionTemplate = BrowserActionTemplate;
    type State = BrowserState;

    const STALL_TIMEOUT: Duration = BrowserSession::STALL_TIMEOUT;

    fn terminate(&mut self) -> Result<()> {
        self.session.terminate()
    }

    fn next_event(
        &mut self,
        timeout: Duration,
    ) -> Option<DriverEvent<BrowserState>> {
        self.session.next_event(timeout)
    }

    fn apply(
        &mut self,
        action: Applied,
        state: Arc<BrowserState>,
    ) -> Result<()> {
        (self.on_apply)(&action);
        self.session.apply(action, state)
    }

    fn extract_snapshots(
        &mut self,
        state: Arc<BrowserState>,
        last_action: Option<&Applied>,
    ) -> Result<Vec<Snapshot>> {
        self.session.extract_snapshots(state, last_action)
    }

    fn state_timestamp(state: &BrowserState) -> SystemTime {
        BrowserSession::state_timestamp(state)
    }
}

struct Strategy(TestStrategy<rand::prelude::StdRng>);

type Inner = TestStrategy<rand::prelude::StdRng>;

impl<OnApply: FnMut(&Applied)> RunStrategy<Applying<OnApply>> for Strategy {
    type StopValue = TestResult;

    fn on_new_state(
        &mut self,
        state: &BrowserState,
        leaves: &Leaves<'_, BrowserActionTemplate>,
        last_action: Option<&Applied>,
        snapshots: &[Snapshot],
        properties: PropertiesState,
    ) -> Result<ControlFlow<TestResult, Picked<Applied>>> {
        RunStrategy::<BrowserSession>::on_new_state(
            &mut self.0,
            state,
            leaves,
            last_action,
            snapshots,
            properties,
        )
    }

    fn deadline(&self) -> Option<Time> {
        RunStrategy::<BrowserSession>::deadline(&self.0)
    }

    fn on_time_limit(&mut self) -> Result<TestResult> {
        RunStrategy::<BrowserSession>::on_time_limit(&mut self.0)
    }

    fn on_stalled(&mut self) -> Result<TestResult> {
        RunStrategy::<BrowserSession>::on_stalled(&mut self.0)
    }

    fn on_interrupted(&mut self) -> Result<TestResult> {
        RunStrategy::<BrowserSession>::on_interrupted(&mut self.0)
    }

    fn constrain(
        &self,
        state: &BrowserState,
        tree: Tree<BrowserActionTemplate>,
    ) -> Tree<BrowserActionTemplate> {
        <Inner as RunStrategy<BrowserSession>>::constrain(&self.0, state, tree)
    }
}

struct Writer(FileTraceWriter);

impl<OnApply: FnMut(&Applied)> TraceWriter<Applying<OnApply>> for Writer {
    fn write(
        &mut self,
        state: &BrowserState,
        snapshots: &[Snapshot],
        violations: &[PropertyViolation],
        actions_available: &Leaves<'_, BrowserActionTemplate>,
        action_previous: Option<&Picked<Applied>>,
    ) -> Result<()> {
        self.0.write(
            state,
            snapshots,
            violations,
            actions_available,
            action_previous,
        )
    }

    fn write_action(&mut self, picked: &Picked<Applied>) -> Result<()> {
        self.0.write_action(picked)
    }

    fn write_error(&mut self, error: StepError) -> Result<()> {
        self.0.write_error(error)
    }

    fn finish(&mut self) -> Result<()> {
        self.0.finish()
    }
}

fn run(
    origin: Url,
    remote_debugger: Url,
    trace: &Path,
    mode: TestMode,
    interrupted: Arc<AtomicBool>,
    mut on_apply: impl FnMut(&BrowserAction),
) -> Result<TestResult> {
    let downloads_directory = TempDir::new()?;
    let seed: u64 = rand::random();
    log::info!("running with seed {seed}");
    let random_walk = matches!(mode, TestMode::RandomWalk);
    let test_start = SystemTime::now();
    let mut strategy = Strategy(TestStrategy {
        rng: rand::prelude::StdRng::seed_from_u64(seed),
        mode,
        exit_on_violation: false,
        test_start: Some(Time::from_system_time(test_start)),
        deadline: random_walk.then(|| test_start + Duration::from_secs(90)),
        allow_urls: vec![
            MatchPattern::from_origin(&origin).map_err(anyhow::Error::msg)?,
        ],
        violations_count: 0,
        quiet: false,
    });
    let driver = BrowserDriver {
        origin,
        browser_options: BrowserOptions {
            create_target: true,
            emulation: Emulation {
                width: 800,
                height: 600,
                device_scale_factor: 1.0,
            },
            instrumentation: Default::default(),
            downloads_directory: downloads_directory.path().to_path_buf(),
            grant_permissions: vec![
                "local-network-access".to_string(),
                "local-network".to_string(),
                "loopback-network".to_string(),
            ],
            extra_headers: Default::default(),
            cookies: vec![],
            virtual_time_policy: Some(VirtualTimePolicy::Advance),
        },
        debugger_options: DebuggerOptions::External { remote_debugger },
        specification_bundle: Arc::from(bundle(
            ".",
            "@antithesishq/bombadil/browser/defaults",
        )?),
        blocks: BlockRegistry::default(),
    };
    let (session, verifier) = driver.new_session(RunId::default())?;
    let mut session = Applying {
        session,
        on_apply: |action: &Applied| {
            let action = action.to_schema();
            let last = records(trace).pop();
            assert!(
                matches!(
                    &last,
                    Some(Record::Error {
                        actions,
                        error: StepError::Unknown,
                        ..
                    }) if actions.last().is_some_and(|last| last.action == action)
                ),
                "{action:?} isn't in the trace when applied, which ends \
                 with {last:?}"
            );
            on_apply(&action);
        },
    };
    bombadil::runner::run(
        &mut session,
        &mut strategy,
        verifier,
        &mut Writer(FileTraceWriter::initialize(trace.to_path_buf(), false)?),
        interrupted,
    )
}

type BrowserRecord = Record<BrowserTraceEntry, Recorded<BrowserAction>>;

fn records(trace: &Path) -> Vec<BrowserRecord> {
    open_trace(trace)
        .unwrap()
        .without_payloads()
        .collect::<Result<_>>()
        .unwrap()
}

fn is_die_click(action: &BrowserAction) -> bool {
    matches!(
        action,
        BrowserAction::Click { fingerprint, .. }
            if fingerprint.id.as_deref() == Some("die")
    )
}

fn actions_ending_with_die(trace: &Path) -> Vec<Recorded<BrowserAction>> {
    let actions = load_trace_actions::<BrowserAction>(trace).unwrap().actions;
    let die_clicks: Vec<usize> = actions
        .iter()
        .enumerate()
        .filter(|(_, recorded)| is_die_click(&recorded.action))
        .map(|(index, _)| index)
        .collect();
    assert_eq!(
        die_clicks,
        [actions.len().wrapping_sub(1)],
        "the trace doesn't end with the click of Die: {actions:#?}"
    );
    actions
}

fn check_ends_with_die(trace: &Path, expected: impl Fn(&StepError) -> bool) {
    let mut records = records(trace);
    let last = records.pop();
    assert!(
        matches!(
            &last,
            Some(Record::Error { actions, error, .. })
                if actions.len() == 1
                    && is_die_click(&actions[0].action)
                    && expected(error)
        ),
        "the trace ends with {last:?}"
    );
    for record in &records {
        assert!(
            matches!(
                record,
                Record::State { action, .. } if !action
                    .as_ref()
                    .is_some_and(|action| is_die_click(&action.action))
            ),
            "before its last record, the trace has {record:?}"
        );
    }
}

fn check_reproduces(actions: Vec<Recorded<BrowserAction>>) {
    let directory = TempDir::new().unwrap();
    let trace = directory.path().join("reproduced.bin");
    let chrome = Chrome::launch();
    let deaths = Arc::new(AtomicUsize::new(0));
    let origin = serve(Handlers {
        on_ready: || {},
        on_die: {
            let deaths = deaths.clone();
            move || {
                deaths.fetch_add(1, Ordering::SeqCst);
            }
        },
    });
    let replayed: VecDeque<_> = actions
        .iter()
        .cloned()
        .map(|recorded| recorded.map(|action| action.to_internal()))
        .collect();
    let result = run(
        origin,
        chrome.remote_debugger.clone(),
        &trace,
        TestMode::Reproduce(replayed, None),
        Arc::new(AtomicBool::new(false)),
        |_| {},
    )
    .unwrap();
    assert!(
        matches!(result.exit_reason, ExitReason::Reproduced),
        "{result:?}"
    );
    assert_eq!(deaths.load(Ordering::SeqCst), 1);
    assert!(
        records(&trace)
            .iter()
            .all(|record| matches!(record, Record::State { .. }))
    );
    assert_eq!(actions_ending_with_die(&trace), actions);
}

#[derive(Clone, Copy, PartialEq)]
enum Death {
    Renderer,
    Browser,
    BrowserBeforeClick,
}

fn check_survives(death: Death) {
    setup();
    let directory = TempDir::new().unwrap();
    let trace = directory.path().join("trace.bin");
    let chrome = Chrome::launch();
    let pid = chrome.pid();
    let interrupted = Arc::new(AtomicBool::new(false));
    let origin = Arc::new(OnceLock::<Url>::new());
    let renderer = Arc::new(OnceLock::<Renderer>::new());
    let served = serve(Handlers {
        on_ready: {
            let web_socket = chrome.web_socket.clone();
            let (origin, renderer) = (origin.clone(), renderer.clone());
            move || {
                renderer.get_or_init(|| {
                    Renderer::attach(&web_socket, origin.get().unwrap())
                        .unwrap()
                });
            }
        },
        on_die: {
            let (renderer, interrupted) =
                (renderer.clone(), interrupted.clone());
            move || {
                match death {
                    Death::Renderer => renderer.get().unwrap().crash().unwrap(),
                    Death::Browser => kill_browser(pid),
                    Death::BrowserBeforeClick => {
                        panic!("the page got the click")
                    }
                }
                thread::sleep(INTERRUPT_DELAY);
                interrupted.store(true, Ordering::SeqCst);
            }
        },
    });
    origin.set(served.clone()).unwrap();

    let result = run(
        served,
        chrome.remote_debugger.clone(),
        &trace,
        TestMode::RandomWalk,
        interrupted,
        |action| {
            if death == Death::BrowserBeforeClick && is_die_click(action) {
                kill_browser(pid);
            }
        },
    );
    log::info!("the run ended with {result:?}");

    // A dead renderer is only noticed as a stall, which isn't waited for.
    check_ends_with_die(&trace, |error| match death {
        Death::Renderer => *error == StepError::Interrupted,
        Death::Browser | Death::BrowserBeforeClick => {
            matches!(error, StepError::Driver { .. })
        }
    });
    assert!(open_finished(&trace));
    check_reproduces(actions_ending_with_die(&trace));
}

fn open_finished(trace: &Path) -> bool {
    let mut reader = open_trace::<IgnoredAny, IgnoredAny>(trace).unwrap();
    reader.by_ref().for_each(drop);
    reader.finished()
}

#[test]
fn trace_holds_the_action_that_crashed_the_renderer() {
    check_survives(Death::Renderer);
}

#[test]
fn trace_holds_the_action_that_killed_the_browser() {
    check_survives(Death::Browser);
}

/// The browser dies inside `apply`, so nothing is written after it.
#[test]
fn trace_holds_the_action_the_browser_died_before_getting() {
    check_survives(Death::BrowserBeforeClick);
}

/// Run by [`trace_holds_the_action_bombadil_was_killed_after`] only.
#[test]
#[ignore = "run by trace_holds_the_action_bombadil_was_killed_after"]
fn killed_run() {
    let (Ok(origin), Ok(remote_debugger), Ok(trace)) = (
        std::env::var(CHILD_ORIGIN),
        std::env::var(CHILD_REMOTE_DEBUGGER),
        std::env::var(CHILD_TRACE),
    ) else {
        return;
    };
    setup();
    let result = run(
        Url::parse(&origin).unwrap(),
        Url::parse(&remote_debugger).unwrap(),
        Path::new(&trace),
        TestMode::RandomWalk,
        Arc::new(AtomicBool::new(false)),
        |_| {},
    );
    panic!("the run ended instead of being killed: {result:?}");
}

/// Killed while the page handles the click: nothing buffered is written.
#[test]
fn trace_holds_the_action_bombadil_was_killed_after() {
    setup();
    let directory = TempDir::new().unwrap();
    let trace = directory.path().join("trace.bin");
    let chrome = Chrome::launch();
    let bombadil = Arc::new(AtomicI32::new(0));
    let origin = serve(Handlers {
        on_ready: || {},
        on_die: {
            let bombadil = bombadil.clone();
            move || {
                let pid = bombadil.load(Ordering::SeqCst);
                assert_ne!(pid, 0);
                // SAFETY: the child isn't waited for until it is seen dead.
                unsafe { libc::kill(pid, libc::SIGKILL) };
            }
        },
    });
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["killed_run", "--exact", "--ignored", "--nocapture"])
        .env(CHILD_ORIGIN, origin.as_str())
        .env(CHILD_REMOTE_DEBUGGER, chrome.remote_debugger.as_str())
        .env(CHILD_TRACE, &trace)
        .spawn()
        .unwrap();
    bombadil.store(child.id() as i32, Ordering::SeqCst);
    let status = child.wait().unwrap();
    assert_eq!(status.signal(), Some(libc::SIGKILL), "{status:?}");

    assert!(!open_finished(&trace));
    // The states of the actions before it may not have been written.
    let mut records = records(&trace);
    let last = records.pop();
    assert!(
        matches!(
            &last,
            Some(Record::Error {
                actions,
                error: StepError::Unknown,
                ..
            }) if actions.last().is_some_and(|last| is_die_click(&last.action))
        ),
        "the trace ends with {last:?}"
    );
    assert!(
        records
            .iter()
            .all(|record| matches!(record, Record::State { .. }))
    );
    check_reproduces(actions_ending_with_die(&trace));
}
