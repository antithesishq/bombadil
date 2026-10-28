use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime};

use bombadil::driver::{
    DriverEvent, InterfaceDriver, InterfaceSession, RunId, TraceWriter,
};
use bombadil::runner;
use bombadil::shrink::load_trace_actions;
use bombadil::specification::bundler::bundle;
use bombadil::specification::verifier::Verifier;
use bombadil_rpc::action::ExternalAction;
use bombadil_rpc::driver::ExternalDriver;
use bombadil_rpc::trace::ExternalTraceWriter;
use bombadil_rpc::{ExitReason, ExternalStrategy, ExternalTestMode};
use rand::{SeedableRng, rngs::StdRng};
use serde_json::{self as json, json};
use tempfile::NamedTempFile;

const SPECIFICATION: &str = r#"
import { always } from "@antithesishq/bombadil";
import { actions, extract, integer } from "@antithesishq/bombadil/external";

const count = extract((state: { count: number }) => state.count);

export const countAtMostThree = always(() => count.current <= 3);

export const counter = actions(() => [
  { Increment: { by: integer([1, 2]) } },
  "Reset",
]);
"#;

/// The counter driver, run with the given environment variable assignments.
fn driver(env: &[&str]) -> (ExternalDriver, Arc<str>, NamedTempFile) {
    let mut specification = NamedTempFile::with_suffix(".ts").unwrap();
    specification.write_all(SPECIFICATION.as_bytes()).unwrap();
    let specification_bundle: Arc<str> =
        bundle(".", &specification.path().display().to_string())
            .unwrap()
            .into();
    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/counter-driver.sh");
    let driver = ExternalDriver {
        specification_bundle: specification_bundle.clone(),
        program: "env".to_string(),
        arguments: env
            .iter()
            .map(|assignment| assignment.to_string())
            .chain(["sh".to_string(), script.display().to_string()])
            .collect(),
    };
    (driver, specification_bundle, specification)
}

#[test]
fn finds_violation_through_external_driver() {
    let (driver, specification_bundle, _specification) = driver(&[]);
    let output = tempfile::tempdir().unwrap();
    let mut trace_writer =
        ExternalTraceWriter::initialize(output.path().to_path_buf(), false)
            .unwrap();
    let mut strategy = ExternalStrategy {
        rng: StdRng::seed_from_u64(1),
        mode: ExternalTestMode::RandomWalk,
        test_start: None,
        violations_count: 0,
        exit_on_violation: true,
        deadline: Some(SystemTime::now() + Duration::from_secs(30)),
        quiet: true,
    };
    let mut session = driver.new_session(RunId::default()).unwrap();
    let exit_reason = runner::run(
        &mut session,
        &mut strategy,
        Verifier::new(&specification_bundle).unwrap(),
        &mut trace_writer,
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap();
    drop(trace_writer);

    assert!(matches!(exit_reason, ExitReason::ExitOnViolation));
    let trace =
        load_trace_actions::<json::Value>(&output.path().join("trace.jsonl"))
            .unwrap();
    assert!(trace.violations_earliest.contains_key("countAtMostThree"));
    // The count only exceeds three after at least two increments.
    assert!(trace.actions.len() >= 2);
    for action in &trace.actions {
        assert!(
            *action == json!("Reset")
                || *action == json!({ "Increment": { "by": 1 } })
                || *action == json!({ "Increment": { "by": 2 } }),
            "unexpected action {action}"
        );
    }
}

#[test]
fn rejects_driver_with_other_protocol_version() {
    let (driver, _, _specification) = driver(&["PROTOCOL=7"]);
    let error = match driver.new_session(RunId::default()) {
        Ok(_) => panic!("session started despite protocol mismatch"),
        Err(error) => error.to_string(),
    };
    assert!(error.contains("protocol version 7"), "{error}");
}

#[test]
fn stops_when_driver_exceeds_its_declared_stall_timeout() {
    let (driver, specification_bundle, _specification) =
        driver(&["STALL_MS=300"]);
    let mut strategy = ExternalStrategy {
        rng: StdRng::seed_from_u64(1),
        mode: ExternalTestMode::RandomWalk,
        test_start: None,
        violations_count: 0,
        exit_on_violation: true,
        deadline: None,
        quiet: true,
    };
    let started = std::time::Instant::now();
    let mut session = driver.new_session(RunId::default()).unwrap();
    let exit_reason = runner::run(
        &mut session,
        &mut strategy,
        Verifier::new(&specification_bundle).unwrap(),
        &mut bombadil::driver::NoopTraceWriter,
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap();
    assert!(matches!(exit_reason, ExitReason::Stalled));
    // Far below the default stall timeout of 30 seconds.
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[test]
fn terminating_a_session_stops_what_its_driver_started() {
    let pidfile = NamedTempFile::new().unwrap();
    let assignment = format!("CHILD_PIDFILE={}", pidfile.path().display());
    let (driver, _specification_bundle, _specification) =
        driver(&[&assignment]);
    let mut session = driver.new_session(RunId::default()).unwrap();
    let child: libc::pid_t = std::fs::read_to_string(pidfile.path())
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    // SAFETY: `kill` has no memory safety requirements.
    assert_eq!(unsafe { libc::kill(child, 0) }, 0, "child is running");
    session.terminate().unwrap();
    // The driver exits on `terminate`, but its child would live on.
    assert_ne!(unsafe { libc::kill(child, 0) }, 0, "child is stopped");
}

#[test]
fn records_what_the_driver_says_about_each_state() {
    let (driver, _specification_bundle, _specification) =
        driver(&["EXTRAS=1700000000000000"]);
    let output = tempfile::tempdir().unwrap();
    let mut trace_writer =
        ExternalTraceWriter::initialize(output.path().to_path_buf(), false)
            .unwrap();
    let mut session = driver.new_session(RunId::default()).unwrap();
    let mut previous = None;
    for action in [
        None,
        Some(json!("Reset")),
        Some(json!({ "Increment": { "by": 2 } })),
        Some(json!("Reset")),
    ] {
        if let (Some(action), Some(previous)) = (&action, previous.take()) {
            session
                .apply(ExternalAction(action.clone()), previous)
                .unwrap();
        }
        let Some(DriverEvent::StateChanged(state)) =
            session.next_event(Duration::from_secs(10))
        else {
            panic!("no state");
        };
        trace_writer
            .write(&state, action.map(ExternalAction).as_ref(), &[], &[])
            .unwrap();
        previous = Some(state);
    }
    session.terminate().unwrap();
    drop(trace_writer);

    let entries: Vec<json::Value> =
        std::fs::read_to_string(output.path().join("trace.jsonl"))
            .unwrap()
            .lines()
            .map(|line| json::from_str(line).unwrap())
            .collect();
    let field = |key: &str| -> Vec<json::Value> {
        entries
            .iter()
            .map(|entry| entry.get(key).cloned().unwrap_or(json::Value::Null))
            .collect()
    };
    // The driver's own timestamps, not when Bombadil received the states.
    assert_eq!(
        field("timestamp"),
        [0, 1000, 2000, 3000]
            .map(|offset| json!(1_700_000_000_000_000u64 + offset))
    );
    // Only the reset of a zero count wasn't applied; the second reset was.
    assert_eq!(
        field("not_applied"),
        [json!(null), json!("already zero"), json!(null), json!(null)]
    );
    // Only the state after the increment wasn't settled.
    assert_eq!(
        field("settled"),
        [json!(null), json!(null), json!(false), json!(null)]
    );
}
