use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Result, anyhow};
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

    fn write_action(&mut self, _picked: &Picked<FakeAction>) -> Result<()> {
        Ok(())
    }

    fn write_error(&mut self, _error: bombadil_trace::StepError) -> Result<()> {
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

type Steps = Arc<std::sync::Mutex<Vec<String>>>;

enum Next {
    State,
    /// No event; with `true`, the run is interrupted.
    Wait(bool),
    Error,
    Closed,
}

struct ScriptedSession {
    steps: Steps,
    script: std::collections::VecDeque<Next>,
    interrupted: Arc<AtomicBool>,
    apply_fails: bool,
    extract_fails_at: Option<usize>,
    states: usize,
}

impl InterfaceSession for ScriptedSession {
    type Action = FakeAction;
    type ActionTemplate = FakeAction;
    type State = FakeState;

    const STALL_TIMEOUT: Duration = Duration::from_millis(300);

    fn terminate(&mut self) -> Result<()> {
        Ok(())
    }

    fn next_event(
        &mut self,
        timeout: Duration,
    ) -> Option<DriverEvent<Self::State>> {
        match self.script.pop_front() {
            Some(Next::State) => {
                Some(DriverEvent::StateChanged(Arc::new(FakeState)))
            }
            Some(Next::Wait(interrupt)) => {
                self.script.push_front(Next::Wait(interrupt));
                self.interrupted.store(interrupt, Ordering::SeqCst);
                thread::sleep(timeout.min(Duration::from_millis(20)));
                Some(DriverEvent::Timeout)
            }
            Some(Next::Error) => {
                Some(DriverEvent::Error(Arc::new(anyhow!("boom"))))
            }
            Some(Next::Closed) | None => None,
        }
    }

    fn apply(
        &mut self,
        _action: FakeAction,
        _state: Arc<FakeState>,
    ) -> Result<()> {
        self.steps.lock().unwrap().push("apply".into());
        if self.apply_fails {
            anyhow::bail!("apply failed");
        }
        Ok(())
    }

    fn extract_snapshots(
        &mut self,
        _state: Arc<FakeState>,
        _last_action: Option<&FakeAction>,
    ) -> Result<Vec<Snapshot>> {
        self.states += 1;
        if self.extract_fails_at == Some(self.states) {
            anyhow::bail!("extractor failed");
        }
        Ok(vec![])
    }

    fn state_timestamp(_state: &FakeState) -> SystemTime {
        SystemTime::UNIX_EPOCH
    }
}

/// Picks an action in each of its first `actions` states, then stops.
struct PickingStrategy {
    actions: usize,
    fails: bool,
    deadline: Option<Time>,
}

impl RunStrategy<ScriptedSession> for PickingStrategy {
    type StopValue = &'static str;

    fn on_new_state(
        &mut self,
        _state: &FakeState,
        _leaves: &Leaves<'_, FakeAction>,
        _last_action: Option<&FakeAction>,
        _snapshots: &[Snapshot],
        _properties: PropertiesState,
    ) -> Result<ControlFlow<&'static str, Picked<FakeAction>>> {
        let Some(actions) = self.actions.checked_sub(1) else {
            if self.fails {
                anyhow::bail!("strategy failed");
            }
            return Ok(ControlFlow::Stop("stopped"));
        };
        self.actions = actions;
        Ok(ControlFlow::Continue(Picked {
            action: FakeAction,
            template: bombadil_schema::ActionKey(0),
        }))
    }

    fn deadline(&self) -> Option<Time> {
        self.deadline
    }

    fn on_time_limit(&mut self) -> Result<&'static str> {
        Ok("time limit")
    }

    fn on_stalled(&mut self) -> Result<&'static str> {
        Ok("stalled")
    }

    fn on_interrupted(&mut self) -> Result<&'static str> {
        Ok("interrupted")
    }
}

struct RecordingWriter {
    steps: Steps,
    write_fails_at: Option<usize>,
    states: usize,
}

impl bombadil::driver::TraceWriter<ScriptedSession> for RecordingWriter {
    fn write(
        &mut self,
        _state: &FakeState,
        _snapshots: &[Snapshot],
        _violations: &[bombadil::runner::PropertyViolation],
        _actions_available: &Leaves<'_, FakeAction>,
        _action_previous: Option<&Picked<FakeAction>>,
    ) -> Result<()> {
        self.states += 1;
        if self.write_fails_at == Some(self.states) {
            anyhow::bail!("write failed");
        }
        self.steps.lock().unwrap().push("state".into());
        Ok(())
    }

    fn write_action(&mut self, _picked: &Picked<FakeAction>) -> Result<()> {
        self.steps.lock().unwrap().push("action".into());
        Ok(())
    }

    fn write_error(&mut self, error: bombadil_trace::StepError) -> Result<()> {
        self.steps.lock().unwrap().push(format!("error: {error}"));
        Ok(())
    }

    fn finish(&mut self) -> Result<()> {
        self.steps.lock().unwrap().push("finish".into());
        Ok(())
    }
}

/// An action is written before it is applied, and however the run ends
/// while it has no state, the trace gets one error saying why.
#[test]
fn writes_each_action_before_applying_it_and_why_no_state_followed() {
    use Next::*;
    #[derive(Default)]
    struct Case {
        name: &'static str,
        script: Vec<Next>,
        actions: usize,
        deadline: bool,
        apply_fails: bool,
        extract_fails_at: Option<usize>,
        write_fails_at: Option<usize>,
        strategy_fails: bool,
        result: &'static str,
        steps: &'static [&'static str],
    }
    let cases = [
        Case {
            name: "stall",
            script: vec![State, Wait(false)],
            actions: 1,
            result: "stalled",
            steps: &["state", "action", "apply", "error: the driver stalled"],
            ..Case::default()
        },
        Case {
            name: "time limit while waiting",
            script: vec![State, Wait(false)],
            actions: 1,
            deadline: true,
            result: "time limit",
            steps: &[
                "state",
                "action",
                "apply",
                "error: the time limit was reached",
            ],
            ..Case::default()
        },
        Case {
            name: "interrupt",
            script: vec![State, Wait(true)],
            actions: 1,
            result: "interrupted",
            steps: &[
                "state",
                "action",
                "apply",
                "error: the run was interrupted",
            ],
            ..Case::default()
        },
        Case {
            name: "driver error",
            script: vec![State, Error],
            actions: 1,
            result: "driver error: boom",
            steps: &["state", "action", "apply", "error: driver error: boom"],
            ..Case::default()
        },
        Case {
            name: "driver closed",
            script: vec![State, Closed],
            actions: 1,
            result: "driver closed",
            steps: &[
                "state",
                "action",
                "apply",
                "error: driver error: driver closed",
            ],
            ..Case::default()
        },
        Case {
            name: "apply fails",
            script: vec![State],
            actions: 1,
            apply_fails: true,
            result: "apply failed",
            steps: &[
                "state",
                "action",
                "apply",
                "error: driver error: apply failed",
            ],
            ..Case::default()
        },
        Case {
            name: "extractors fail on the state after",
            script: vec![State, State],
            actions: 1,
            extract_fails_at: Some(2),
            result: "extractor failed",
            steps: &[
                "state",
                "action",
                "apply",
                "error: the run failed: extractor failed",
            ],
            ..Case::default()
        },
        Case {
            name: "the state after can't be written",
            script: vec![State, State],
            actions: 1,
            write_fails_at: Some(2),
            result: "write failed",
            steps: &[
                "state",
                "action",
                "apply",
                "error: the run failed: write failed",
            ],
            ..Case::default()
        },
        Case {
            name: "a state followed",
            script: vec![State, State],
            actions: 1,
            result: "stopped",
            steps: &["state", "action", "apply", "state"],
            ..Case::default()
        },
        Case {
            name: "a state followed, then a stall",
            script: vec![State, State, Wait(false)],
            actions: 2,
            result: "stalled",
            steps: &[
                "state",
                "action",
                "apply",
                "state",
                "action",
                "apply",
                "error: the driver stalled",
            ],
            ..Case::default()
        },
        Case {
            name: "driver error before any action",
            script: vec![Error],
            result: "driver error: boom",
            steps: &[],
            ..Case::default()
        },
        Case {
            name: "first extractors fail",
            script: vec![State],
            extract_fails_at: Some(1),
            result: "extractor failed",
            steps: &[],
            ..Case::default()
        },
        Case {
            name: "the run fails after a state followed",
            script: vec![State, State],
            actions: 1,
            strategy_fails: true,
            result: "strategy failed",
            steps: &["state", "action", "apply", "state"],
            ..Case::default()
        },
        Case {
            name: "interrupt before any action",
            script: vec![Wait(true)],
            result: "interrupted",
            steps: &[],
            ..Case::default()
        },
    ];
    for case in cases {
        let steps = Steps::default();
        let interrupted = Arc::new(AtomicBool::new(false));
        let result = runner::run(
            &mut ScriptedSession {
                steps: steps.clone(),
                script: case.script.into(),
                interrupted: interrupted.clone(),
                apply_fails: case.apply_fails,
                extract_fails_at: case.extract_fails_at,
                states: 0,
            },
            &mut PickingStrategy {
                actions: case.actions,
                fails: case.strategy_fails,
                deadline: case.deadline.then(|| {
                    Time::from_system_time(
                        SystemTime::now() + Duration::from_millis(50),
                    )
                }),
            },
            dummy_verifier(),
            &mut RecordingWriter {
                steps: steps.clone(),
                write_fails_at: case.write_fails_at,
                states: 0,
            },
            interrupted,
        );
        let result = match result {
            Ok(stop) => stop.to_string(),
            Err(error) => format!("{error:#}"),
        };
        assert_eq!(result, case.result, "{}", case.name);
        let mut expected: Vec<&str> = case.steps.to_vec();
        expected.push("finish");
        assert_eq!(*steps.lock().unwrap(), expected, "{}", case.name);
    }
}
