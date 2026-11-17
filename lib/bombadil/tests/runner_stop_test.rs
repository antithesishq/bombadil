use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use anyhow::Result;
use bombadil::render::Format;
use bombadil_schema::Time;
use serde::{Deserialize, Serialize};
use serde_json as json;
use tempfile::NamedTempFile;

use bombadil::driver::{
    ActionTemplate, DriverEvent, FromGeneratedAction, InterfaceDriver,
    InterfaceSession, NoopTraceWriter, RunId, RunState,
};
use bombadil::model::Picked;
use bombadil::runner::{self, ControlFlow, PropertiesState, RunStrategy};
use bombadil::specification::bundler::bundle;
use bombadil::specification::domain::Snapshot;
use bombadil::specification::verifier::Verifier;
use bombadil::tree::Leaves;

#[derive(
    Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord,
)]
struct FakeAction;

impl Format for FakeAction {
    fn format(
        &self,
        f: &mut std::fmt::Formatter,
    ) -> std::prelude::v1::Result<(), std::fmt::Error> {
        write!(f, "Fake action")
    }
}

impl ActionTemplate<FakeAction> for FakeAction {
    fn generate<Rng: rand::TryRng + rand::RngExt>(
        &self,
        _rng: &mut Rng,
    ) -> FakeAction {
        self.clone()
    }

    fn accepts(&self, _original: &FakeAction) -> bool {
        true
    }

    fn category_hash<H: std::hash::Hasher>(&self, hasher: &mut H) {
        hasher.write_u8(0)
    }
}

impl FromGeneratedAction for FakeAction {
    fn from_generated(_value: json::Value) -> Result<Self> {
        Ok(FakeAction)
    }
}

#[derive(Debug)]
struct FakeState;

impl RunState for FakeState {
    fn timestamp(&self) -> Time {
        Time::from_system_time(SystemTime::UNIX_EPOCH)
    }
}

struct FakeDriver {
    initiated: Arc<AtomicBool>,
    terminated: Arc<AtomicBool>,
    next_event_calls: Arc<AtomicUsize>,
}

impl InterfaceDriver for FakeDriver {
    type Session = FakeSession;

    fn new_session(
        &self,
        _run_id: RunId,
    ) -> std::result::Result<(FakeSession, Verifier), anyhow::Error> {
        self.initiated.store(true, Ordering::SeqCst);
        Ok((
            FakeSession {
                terminated: self.terminated.clone(),
                next_event_calls: self.next_event_calls.clone(),
                terminate_fails: false,
            },
            dummy_verifier(),
        ))
    }
}

struct FakeSession {
    terminated: Arc<AtomicBool>,
    next_event_calls: Arc<AtomicUsize>,
    terminate_fails: bool,
}

impl InterfaceSession for FakeSession {
    type Action = FakeAction;
    type ActionTemplate = FakeAction;
    type State = FakeState;

    const STALL_TIMEOUT: Duration = Duration::MAX;

    fn terminate(&mut self) -> Result<()> {
        self.terminated.store(true, Ordering::SeqCst);
        if self.terminate_fails {
            anyhow::bail!("terminate failed");
        }
        Ok(())
    }

    fn next_event(
        &mut self,
        _timeout: Duration,
    ) -> Option<DriverEvent<Self::State>> {
        self.next_event_calls.fetch_add(1, Ordering::SeqCst);
        Some(DriverEvent::StateChanged(Arc::new(FakeState)))
    }

    fn apply(
        &mut self,
        _action: FakeAction,
        _state: Arc<FakeState>,
    ) -> Result<()> {
        Ok(())
    }

    fn extract_snapshots(
        &mut self,
        _state: Arc<FakeState>,
        _last_action: Option<&FakeAction>,
    ) -> Result<Vec<Snapshot>> {
        Ok(vec![])
    }

    fn state_timestamp(_state: &FakeState) -> SystemTime {
        SystemTime::UNIX_EPOCH
    }
}

struct FakeStrategy {
    on_interrupted_calls: Arc<AtomicUsize>,
    run_fails: bool,
}

impl RunStrategy<FakeSession> for FakeStrategy {
    type StopValue = ();

    fn on_new_state(
        &mut self,
        _state: &FakeState,
        _leaves: &Leaves<'_, FakeAction>,
        _last_action: Option<&FakeAction>,
        _snapshots: &[Snapshot],
        _properties: PropertiesState,
    ) -> Result<ControlFlow<(), Picked<FakeAction>>> {
        panic!("on_new_state must not run once interrupted");
    }

    fn deadline(&self) -> Option<Time> {
        None
    }

    fn on_time_limit(&mut self) -> Result<()> {
        Ok(())
    }

    fn on_stalled(&mut self) -> Result<()> {
        Ok(())
    }

    fn on_interrupted(&mut self) -> Result<()> {
        self.on_interrupted_calls.fetch_add(1, Ordering::SeqCst);
        if self.run_fails {
            anyhow::bail!("run failed");
        }
        Ok(())
    }
}

fn dummy_verifier() -> Verifier {
    // Verifier::new requires at least one action generator, but no
    // property is ever evaluated because the runner never enters the
    // loop when `interrupted` is already set.
    let mut file = NamedTempFile::with_suffix(".ts").unwrap();
    file.write_all(
        b"import { actions } from \"@antithesishq/bombadil\";\n\
          export const _actions = actions(() => []);\n",
    )
    .unwrap();
    let bundle_code = bundle(".", &file.path().display().to_string()).unwrap();
    Verifier::new(&bundle_code).unwrap()
}

#[test]
fn interrupt_before_run_terminates_driver_and_invokes_on_interrupted() {
    let interrupted = Arc::new(AtomicBool::new(true));
    let initiated = Arc::new(AtomicBool::new(false));
    let terminated = Arc::new(AtomicBool::new(false));
    let next_event_calls = Arc::new(AtomicUsize::new(0));
    let on_interrupted_calls = Arc::new(AtomicUsize::new(0));

    let driver = FakeDriver {
        initiated: initiated.clone(),
        terminated: terminated.clone(),
        next_event_calls: next_event_calls.clone(),
    };

    let mut strategy = FakeStrategy {
        on_interrupted_calls: on_interrupted_calls.clone(),
        run_fails: false,
    };

    let mut trace_writer = NoopTraceWriter;

    let (mut session, verifier) = driver
        .new_session(RunId::default())
        .expect("driver failed to initiate");
    runner::run(
        &mut session,
        &mut strategy,
        verifier,
        &mut trace_writer,
        interrupted,
    )
    .expect("runner returned Err");

    assert!(
        initiated.load(Ordering::SeqCst),
        "driver.initiate was never called",
    );
    assert!(
        terminated.load(Ordering::SeqCst),
        "driver.terminate was never called after interrupt",
    );
    assert_eq!(
        on_interrupted_calls.load(Ordering::SeqCst),
        1,
        "strategy.on_interrupted should be called exactly once",
    );
    assert_eq!(
        next_event_calls.load(Ordering::SeqCst),
        0,
        "driver.next_event must not be called when interrupted before loop",
    );
}

/// A session whose driver never produces an event.
struct StuckSession<const STALL_MILLIS: u64> {
    terminated: Arc<AtomicBool>,
}

impl<const STALL_MILLIS: u64> InterfaceSession for StuckSession<STALL_MILLIS> {
    type Action = FakeAction;
    type ActionTemplate = FakeAction;
    type State = FakeState;

    const STALL_TIMEOUT: Duration = Duration::from_millis(STALL_MILLIS);

    fn terminate(&mut self) -> Result<()> {
        self.terminated.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn next_event(
        &mut self,
        timeout: Duration,
    ) -> Option<DriverEvent<Self::State>> {
        thread::sleep(timeout);
        Some(DriverEvent::Timeout)
    }

    fn apply(
        &mut self,
        _action: FakeAction,
        _state: Arc<FakeState>,
    ) -> Result<()> {
        Ok(())
    }

    fn extract_snapshots(
        &mut self,
        _state: Arc<FakeState>,
        _last_action: Option<&FakeAction>,
    ) -> Result<Vec<Snapshot>> {
        Ok(vec![])
    }

    fn state_timestamp(_state: &FakeState) -> SystemTime {
        SystemTime::UNIX_EPOCH
    }
}

#[derive(Debug, PartialEq, Eq)]
enum StuckStop {
    TimeLimit,
    Stalled,
    Interrupted,
}

struct StuckStrategy {
    deadline: Option<Time>,
}

impl<const STALL_MILLIS: u64> RunStrategy<StuckSession<STALL_MILLIS>>
    for StuckStrategy
{
    type StopValue = StuckStop;

    fn on_new_state(
        &mut self,
        _state: &FakeState,
        _leaves: &Leaves<'_, FakeAction>,
        _last_action: Option<&FakeAction>,
        _snapshots: &[Snapshot],
        _properties: PropertiesState,
    ) -> Result<ControlFlow<StuckStop, Picked<FakeAction>>> {
        panic!("stuck driver produced a state");
    }

    fn deadline(&self) -> Option<Time> {
        self.deadline
    }

    fn on_time_limit(&mut self) -> Result<StuckStop> {
        Ok(StuckStop::TimeLimit)
    }

    fn on_stalled(&mut self) -> Result<StuckStop> {
        Ok(StuckStop::Stalled)
    }

    fn on_interrupted(&mut self) -> Result<StuckStop> {
        Ok(StuckStop::Interrupted)
    }
}

fn run_stuck<const STALL_MILLIS: u64>(
    deadline: Option<Duration>,
    interrupted: Arc<AtomicBool>,
) -> (Result<StuckStop>, bool) {
    let terminated = Arc::new(AtomicBool::new(false));
    let mut session = StuckSession::<STALL_MILLIS> {
        terminated: terminated.clone(),
    };
    let mut strategy = StuckStrategy {
        deadline: deadline
            .map(|d| Time::from_system_time(SystemTime::now() + d)),
    };
    let result = runner::run(
        &mut session,
        &mut strategy,
        dummy_verifier(),
        &mut NoopTraceWriter,
        interrupted,
    );
    (result, terminated.load(Ordering::SeqCst))
}

#[test]
fn stuck_driver_stops_at_deadline() {
    let start = Instant::now();
    let (result, terminated) = run_stuck::<{ u64::MAX }>(
        Some(Duration::from_millis(500)),
        Arc::new(AtomicBool::new(false)),
    );
    assert_eq!(result.unwrap(), StuckStop::TimeLimit);
    assert!(terminated);
    assert!(start.elapsed() < Duration::from_secs(5));
}

#[test]
fn stuck_driver_stops_after_stall_timeout() {
    let (result, terminated) =
        run_stuck::<500>(None, Arc::new(AtomicBool::new(false)));
    assert_eq!(result.unwrap(), StuckStop::Stalled);
    assert!(terminated);
}

#[test]
fn stuck_driver_stops_on_interrupt() {
    let interrupted = Arc::new(AtomicBool::new(false));
    thread::spawn({
        let interrupted = interrupted.clone();
        move || {
            thread::sleep(Duration::from_millis(500));
            interrupted.store(true, Ordering::SeqCst);
        }
    });
    let start = Instant::now();
    let (result, terminated) = run_stuck::<{ u64::MAX }>(None, interrupted);
    assert_eq!(result.unwrap(), StuckStop::Interrupted);
    assert!(terminated);
    assert!(start.elapsed() < Duration::from_secs(5));
}

/// A trace writer whose `finish` fails with `failure`, if any, counting
/// calls.
struct FinishingWriter {
    failure: Option<&'static str>,
    finish_calls: usize,
}

impl bombadil::driver::TraceWriter<FakeSession> for FinishingWriter {
    fn write(
        &mut self,
        _state: &FakeState,
        _snapshots: &[Snapshot],
        _violations: &[bombadil::runner::PropertyViolation],
        _actions_available: &Leaves<'_, FakeAction>,
        _action_previous: Option<&Picked<FakeAction>>,
    ) -> Result<()> {
        Ok(())
    }

    fn finish(&mut self) -> Result<()> {
        self.finish_calls += 1;
        match self.failure {
            Some(failure) => anyhow::bail!("{failure}"),
            None => Ok(()),
        }
    }
}

/// Keeps the messages logged at error level, from every test in this
/// binary, so checks look for messages unique to their test.
struct ErrorLog(std::sync::Mutex<Vec<String>>);

impl log::Log for ErrorLog {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Error
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            self.0.lock().unwrap().push(record.args().to_string());
        }
    }

    fn flush(&self) {}
}

fn logged_errors() -> Vec<String> {
    static LOG: std::sync::OnceLock<&'static ErrorLog> =
        std::sync::OnceLock::new();
    let log = LOG.get_or_init(|| {
        let log: &'static ErrorLog =
            Box::leak(Box::new(ErrorLog(Default::default())));
        log::set_logger(log).unwrap();
        log::set_max_level(log::LevelFilter::Error);
        log
    });
    log.0.lock().unwrap().clone()
}

/// The outcome of [`run_failing`].
struct FailingRunOutcome {
    /// The error the run returned, if any, with its causes.
    error: Option<String>,
    /// How many times the trace writer's `finish` was called, including
    /// calls that failed.
    finish_calls: usize,
    /// Whether the session's `terminate` was called, even if it failed.
    terminate_called: bool,
}

/// Runs an interrupted run, with the run, finishing the trace and
/// terminating the session failing as given.
fn run_failing(
    run_fails: bool,
    finish_failure: Option<&'static str>,
    terminate_fails: bool,
) -> FailingRunOutcome {
    let terminated = Arc::new(AtomicBool::new(false));
    let mut session = FakeSession {
        terminated: terminated.clone(),
        next_event_calls: Arc::new(AtomicUsize::new(0)),
        terminate_fails,
    };
    let mut strategy = FakeStrategy {
        on_interrupted_calls: Arc::new(AtomicUsize::new(0)),
        run_fails,
    };
    let mut writer = FinishingWriter {
        failure: finish_failure,
        finish_calls: 0,
    };
    let result = runner::run(
        &mut session,
        &mut strategy,
        dummy_verifier(),
        &mut writer,
        Arc::new(AtomicBool::new(true)),
    );
    FailingRunOutcome {
        error: result.err().map(|error| format!("{error:#}")),
        finish_calls: writer.finish_calls,
        terminate_called: terminated.load(Ordering::SeqCst),
    }
}

#[test]
fn run_reports_the_first_error_and_still_finishes_and_terminates() {
    #[derive(Debug)]
    struct Case<'a> {
        run_fails: bool,
        finish_failure: Option<&'static str>,
        terminate_fails: bool,
        expected_error: Option<&'a str>,
    }

    // Installed before running, so nothing logged is missed.
    logged_errors();
    let finish = Some("finish failed");
    // As the writer repeats it, which `run` recognizes.
    let repeated: &'static str =
        format!("{}: write failed", bombadil_trace::WRITER_FAILED_EARLIER)
            .leak();
    let finish_repeated = Some(repeated);
    let finish_repeated_error =
        format!("failed to finish the trace: {repeated}");
    let cases = [
        Case {
            run_fails: false,
            finish_failure: None,
            terminate_fails: false,
            expected_error: None,
        },
        Case {
            run_fails: true,
            finish_failure: finish,
            terminate_fails: true,
            expected_error: Some("run failed"),
        },
        Case {
            run_fails: true,
            finish_failure: None,
            terminate_fails: true,
            expected_error: Some("run failed"),
        },
        Case {
            run_fails: true,
            finish_failure: finish_repeated,
            terminate_fails: false,
            expected_error: Some("run failed"),
        },
        Case {
            run_fails: false,
            finish_failure: finish,
            terminate_fails: true,
            expected_error: Some("failed to finish the trace: finish failed"),
        },
        Case {
            run_fails: false,
            finish_failure: None,
            terminate_fails: true,
            expected_error: Some(
                "failed to terminate the session: terminate failed",
            ),
        },
        Case {
            run_fails: false,
            finish_failure: finish,
            terminate_fails: false,
            expected_error: Some("failed to finish the trace: finish failed"),
        },
        Case {
            run_fails: false,
            finish_failure: finish_repeated,
            terminate_fails: false,
            expected_error: Some(finish_repeated_error.as_str()),
        },
    ];
    for case in cases {
        let outcome = run_failing(
            case.run_fails,
            case.finish_failure,
            case.terminate_fails,
        );
        assert_eq!(outcome.error.as_deref(), case.expected_error, "{case:?}");
        assert_eq!(outcome.finish_calls, 1, "{case:?}");
        assert!(outcome.terminate_called, "{case:?}");
    }

    // The errors after the first are logged, except `finish` repeating
    // the write failure the run failed with.
    let logged = logged_errors();
    let count = |message: &str| {
        logged.iter().filter(|logged| *logged == message).count()
    };
    assert_eq!(count("failed to finish the trace: finish failed"), 1);
    assert_eq!(
        count("failed to terminate the session: terminate failed"),
        3,
        "{logged:?}"
    );
    assert!(
        !logged.iter().any(|logged| logged.contains("write failed")),
        "{logged:?}"
    );
}
