use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use bombadil::driver::{
    DriverEvent, InterfaceDriver, InterfaceSession, OutputWriter, RunId,
    TraceWriter,
};
use bombadil::model::Recorded;
use bombadil::runner;
use bombadil::specification::bundler::bundle;
use bombadil::specification::verifier::Verifier;
use bombadil_rpc::action::ExternalAction;
use bombadil_rpc::driver::ExternalDriver;
use bombadil_rpc::trace::{
    ExternalOutputWriter, ExternalTraceWriter, load_trace_actions,
};
use bombadil_rpc::{ExitReason, ExternalStrategy, ExternalTestMode};
use bombadil_trace::{TraceDriver, TraceFileWriter, open_trace};
use hegel::{
    TestCase,
    generators::{integers, vecs},
};
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
        None,
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

#[hegel::test(test_cases = 50)]
fn counter_runs_match_the_model(tc: TestCase) {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    let png = b"\x89PNG\r\n\x1a\nexternal screenshot fixture";
    let screenshot = format!("SCREENSHOT_PNG={}", STANDARD.encode(png));
    let epoch = tc.draw(
        integers::<u64>()
            .min_value(1_000_000)
            .max_value(2_000_000_000_000_000),
    );
    let increments = tc.draw(vecs(integers::<u64>().max_value(2)).max_size(30));
    let (driver, specification_bundle, _specification) =
        driver(&[&format!("EXTRAS={epoch}"), &screenshot]);
    let output = tempfile::tempdir().unwrap();
    let path = output.path().join("trace.bin");
    let mut trace_writer =
        ExternalTraceWriter::initialize(path.clone(), false).unwrap();
    let mut count = 0;
    let mut expected = Vec::new();
    let mut actions = Vec::new();
    for (step, increment) in std::iter::once(None)
        .chain(increments.iter().copied().map(Some))
        .enumerate()
    {
        let action = increment.map(|by| {
            if by == 0 {
                json!("Reset")
            } else {
                json!({"Increment": {"by": by}})
            }
        });
        let not_applied =
            (increment == Some(0) && count == 0).then_some("already zero");
        count = match increment {
            None | Some(0) => 0,
            Some(by) => count + by,
        };
        expected.push((
            json!({"count": count}),
            json!(action),
            epoch + step as u64 * 1000,
            increment.filter(|&by| by > 0).map(|_| false),
            not_applied,
        ));
        if let Some(action) = action {
            actions.push(Recorded {
                action: ExternalAction(action),
                template: None,
            });
        }
        if count > 3 {
            break;
        }
    }
    let mut strategy = ExternalStrategy {
        rng: StdRng::seed_from_u64(0),
        mode: ExternalTestMode::Reproduce(actions.clone().into()),
        test_start: None,
        violations_count: 0,
        exit_on_violation: true,
        deadline: None,
        quiet: true,
    };
    let mut session = driver.new_session(RunId::default()).unwrap();
    let exit = runner::run(
        &mut session,
        &mut strategy,
        Verifier::new(&specification_bundle).unwrap(),
        &mut trace_writer,
        None,
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap();
    assert_eq!(matches!(exit, ExitReason::ExitOnViolation), count > 3);
    assert!(matches!(
        exit,
        ExitReason::ExitOnViolation | ExitReason::Reproduced
    ));
    drop(trace_writer);

    let mut reader = open_trace::<json::Value>(&path).unwrap();
    assert_eq!(reader.driver, TraceDriver::Unknown(0xFFFF));
    let states: Vec<_> = reader.by_ref().map(|state| state.unwrap()).collect();
    assert!(reader.finished());
    assert_eq!(states.len(), expected.len());
    for (state, (value, action, timestamp, settled, not_applied)) in
        states.iter().zip(expected)
    {
        assert_eq!(state.metadata["state"], value);
        assert_eq!(state.metadata["action"], action);
        assert_eq!(state.metadata["timestamp"], json!(timestamp));
        assert_eq!(state.metadata["settled"], json!(settled));
        assert_eq!(state.metadata["not_applied"], json!(not_applied));
        assert!(state.metadata.get("screenshot_png").is_none());
        assert_eq!(
            state.payload.as_deref(),
            (value["count"] == 0).then_some(png.as_slice())
        );
    }
    let trace = load_trace_actions(&path).unwrap();
    assert_eq!(trace.actions, actions);
    assert_eq!(
        trace.violations_earliest.get("countAtMostThree").copied(),
        (count > 3).then_some(actions.len())
    );
    assert_eq!(strategy.violations_count, u64::from(count > 3));
}

#[test]
fn rejects_invalid_screenshots() {
    for (value, expected) in
        [("!", "invalid base64"), ("bm90IHBuZw==", "not a PNG")]
    {
        let (driver, _, _specification) =
            driver(&[&format!("SCREENSHOT_PNG={value}")]);
        let mut session = driver.new_session(RunId::default()).unwrap();
        let Some(DriverEvent::Error(error)) =
            session.next_event(Duration::from_secs(10))
        else {
            panic!("invalid screenshot accepted");
        };
        assert!(error.to_string().contains(expected), "{error}");
        session.terminate().unwrap();
    }
}

#[test]
fn refuses_other_drivers_and_empty_traces() {
    for driver in [
        TraceDriver::Browser,
        TraceDriver::Terminal,
        TraceDriver::Unknown(0xFFFF),
    ] {
        let output = tempfile::tempdir().unwrap();
        let mut writer = TraceFileWriter::<json::Value>::create(
            &output.path().join("trace.bin"),
            driver,
            false,
        )
        .unwrap();
        writer.finish().unwrap();
        let error = load_trace_actions(&output.path().join("trace.bin"))
            .unwrap_err()
            .to_string();
        if driver == TraceDriver::Unknown(0xFFFF) {
            assert!(error.contains("holds no states"), "{error}");
        } else {
            assert!(error.contains("external"), "{error}");
        }
    }
}

#[hegel::test]
fn outputs_use_the_run_number(tc: TestCase) {
    let run = tc.draw(integers::<u64>());
    let output = tempfile::tempdir().unwrap();
    let mut writer = ExternalOutputWriter {
        root_path: output.path().join("campaign.custom"),
        overwrite: false,
    };
    writer.trace_writer(RunId(run)).unwrap().finish().unwrap();
    let path = output.path().join(format!("campaign-{run}.custom"));
    assert_eq!(
        open_trace::<json::Value>(&path).unwrap().driver,
        TraceDriver::Unknown(0xFFFF)
    );
    assert!(writer.trace_writer(RunId(run)).is_err());
}
