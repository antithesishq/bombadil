//! End-to-end tests of the SwiftUI driver against a mock agent.
//!
//! The mock agent is this very test binary re-executed with a filter
//! for [`mock_agent`] (see [`spawn_target`]): the driver launches it
//! like a real app, the agent connects back over TCP and simulates a
//! counter app with an increment and a decrement button.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write as _};
use std::net::TcpStream;
use std::sync::Once;
use std::sync::mpsc;
use std::time::{Duration, SystemTime};

use anyhow::Result;
use bombadil::runner::{ControlFlow, PropertiesState, RunStrategy, Runner};
use bombadil::specification::domain::Snapshot;
use bombadil::specification::verifier::Specification;
use bombadil::tree::Tree;
use bombadil_schema::swiftui::SwiftUITraceEntry;
use bombadil_swiftui::agent::{CONNECT_ENV_VAR, SwiftUITarget};
use bombadil_swiftui::driver::{
    SwiftUIAction, SwiftUIActionTemplate, SwiftUIDriver,
};
use bombadil_swiftui::state::SwiftUIState;
use bombadil_swiftui::trace::TraceWriter;
use bombadil_swiftui::{SwiftUIStrategy, SwiftUITestMode};
use rand::rngs::ThreadRng;
use serde_json::{Value, json};

const TEST_TIMEOUT: Duration = Duration::from_secs(60);

static INIT: Once = Once::new();

fn setup() {
    INIT.call_once(|| {
        let env = env_logger::Env::default().default_filter_or("info");
        env_logger::Builder::from_env(env)
            .format_timestamp_millis()
            .is_test(true)
            .try_init()
            .ok();
    });
}

/// The launch target for the driver: this test binary, filtered down
/// to just the `mock_agent` "test", which acts as the app under test.
fn spawn_target() -> SwiftUITarget {
    SwiftUITarget::Spawn {
        program: std::env::current_exe()
            .expect("current_exe")
            .display()
            .to_string(),
        arguments: vec!["mock_agent".to_string(), "--exact".to_string()],
    }
}

/// Not a test: the mock agent, run as a child process by the other
/// tests. Does nothing when run as part of a regular test sweep.
#[test]
fn mock_agent() {
    let Ok(address) = std::env::var(CONNECT_ENV_VAR) else {
        return;
    };
    run_mock_agent(&address).expect("mock agent failed");
}

mod counter {
    use bombadil_schema::Rect;

    pub const INCREMENT: Rect = Rect {
        x: 120.0,
        y: 150.0,
        width: 100.0,
        height: 40.0,
    };
    pub const DECREMENT: Rect = Rect {
        x: 280.0,
        y: 150.0,
        width: 100.0,
        height: 40.0,
    };

    pub fn contains(rect: &Rect, x: f64, y: f64) -> bool {
        x >= rect.x
            && x <= rect.x + rect.width
            && y >= rect.y
            && y <= rect.y + rect.height
    }
}

/// A counter app: two buttons and a value display. Decrementing below
/// zero is allowed, which the non-negative property catches.
fn run_mock_agent(address: &str) -> Result<()> {
    let stream = TcpStream::connect(address)?;
    stream.set_nodelay(true)?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;

    let mut send = move |value: Value| -> Result<()> {
        let mut bytes = serde_json::to_vec(&value)?;
        bytes.push(b'\n');
        writer.write_all(&bytes)?;
        Ok(())
    };

    send(json!({"type": "hello", "protocolVersion": 1}))?;

    let mut count: i64 = 0;
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Ok(());
        }
        let message: Value = serde_json::from_str(&line)?;
        match message["type"].as_str() {
            Some("getState") => {
                send(json!({"type": "state", "root": state_tree(count)}))?;
            }
            Some("apply") => {
                if let Some(tap) = message["action"]["Tap"].as_object() {
                    let x = tap["x"].as_f64().unwrap();
                    let y = tap["y"].as_f64().unwrap();
                    if counter::contains(&counter::INCREMENT, x, y) {
                        count += 1;
                    } else if counter::contains(&counter::DECREMENT, x, y) {
                        count -= 1;
                    }
                }
                send(json!({"type": "applied"}))?;
            }
            other => {
                send(json!({
                    "type": "error",
                    "message": format!("unexpected message: {other:?}"),
                }))?;
            }
        }
    }
}

fn state_tree(count: i64) -> Value {
    let button =
        |identifier: &str, label: &str, frame: &bombadil_schema::Rect| {
            json!({
                "role": "Button",
                "identifier": identifier,
                "label": label,
                "frame": {
                    "x": frame.x,
                    "y": frame.y,
                    "width": frame.width,
                    "height": frame.height,
                },
                "enabled": true,
                "selected": false,
                "focused": false,
                "children": [],
            })
        };
    let count_text = json!({
        "role": "StaticText",
        "identifier": "count",
        "value": count.to_string(),
        "frame": {"x": 120, "y": 220, "width": 100, "height": 30},
        "enabled": true,
        "selected": false,
        "focused": false,
        "children": [],
    });
    json!({
        "role": "Application",
        "label": "Counter",
        "frame": {"x": 0, "y": 0, "width": 800, "height": 600},
        "enabled": true,
        "selected": false,
        "focused": false,
        "children": [{
            "role": "Window",
            "label": "Counter",
            "frame": {"x": 100, "y": 100, "width": 400, "height": 300},
            "enabled": true,
            "selected": false,
            "focused": false,
            "children": [
                button("increment", "Increment", &counter::INCREMENT),
                button("decrement", "Decrement", &counter::DECREMENT),
                count_text,
            ],
        }],
    })
}

/// The shipped example specification, so the example stays exercised.
const COUNTER_SPECIFICATION: &str =
    include_str!("../../../examples/swiftui_counter.ts");

fn launch(
    specification_source: &str,
) -> Result<(
    SwiftUIDriver,
    bombadil::specification::verifier::Verifier,
    tempfile::NamedTempFile,
)> {
    let mut specification_file =
        tempfile::NamedTempFile::with_suffix(".ts").unwrap();
    specification_file
        .write_all(specification_source.as_bytes())
        .unwrap();
    let specification = Specification {
        module_specifier: specification_file.path().display().to_string(),
    };
    let (driver, verifier) = SwiftUIDriver::launch(
        specification,
        spawn_target(),
        Duration::from_secs(20),
        Duration::from_millis(1),
        Duration::from_secs(10),
    )?;
    Ok((driver, verifier, specification_file))
}

/// Runs `test` on a worker thread with a hang timeout.
fn with_timeout<T: Send + 'static>(
    test: impl FnOnce() -> Result<T> + Send + 'static,
) -> T {
    let (sender, receiver) = mpsc::channel();
    let _ = std::thread::spawn(move || {
        let _ = sender.send(test());
    });
    receiver
        .recv_timeout(TEST_TIMEOUT)
        .unwrap_or_else(|_| panic!("test hung past {TEST_TIMEOUT:?}"))
        .expect("test failed")
}

/// Random taps must eventually push the counter below zero, violating
/// the `countNonNegative` property; the trace must reproduce it.
#[test]
fn finds_and_reproduces_negative_counter() {
    setup();

    let output_path = with_timeout(|| {
        let output_path = tempfile::TempDir::with_prefix("bombadil_swiftui_")?
            .keep()
            .to_path_buf();
        let (driver, verifier, _specification_file) =
            launch(COUNTER_SPECIFICATION)?;
        let runner = Runner::new(driver, verifier);
        let mut strategy = SwiftUIStrategy {
            rng: rand::rng(),
            mode: SwiftUITestMode::RandomWalk,
            writer: Some(TraceWriter::initialize(output_path.clone(), false)?),
            test_start: None,
            violations_count: 0,
            exit_on_violation: true,
            deadline: Some(SystemTime::now() + Duration::from_secs(30)),
            states_seen: 0,
        };
        runner.run(&mut strategy)?;
        anyhow::ensure!(
            strategy.violations_count > 0,
            "expected a violation of countNonNegative"
        );
        Ok(output_path)
    });

    // Reproduce the recorded run against a fresh mock agent and expect
    // the same violation.
    let trace = std::fs::read_to_string(output_path.join("trace.jsonl"))
        .expect("trace.jsonl");
    let mut actions: VecDeque<SwiftUIAction> = VecDeque::new();
    for line in trace.lines() {
        let entry: SwiftUITraceEntry =
            serde_json::from_str(line).expect("trace entry parses");
        if let Some(action) = entry.action {
            use bombadil::specification::convert::ToInternal;
            actions.push_back(action.to_internal());
        }
    }
    assert!(!actions.is_empty(), "trace contains actions");

    let violations_count = with_timeout(move || {
        let (driver, verifier, _specification_file) =
            launch(COUNTER_SPECIFICATION)?;
        let runner = Runner::new(driver, verifier);
        let mut strategy = SwiftUIStrategy {
            rng: rand::rng(),
            mode: SwiftUITestMode::Reproduce(actions),
            writer: None,
            test_start: None,
            violations_count: 0,
            exit_on_violation: true,
            deadline: Some(SystemTime::now() + Duration::from_secs(30)),
            states_seen: 0,
        };
        runner.run(&mut strategy)?;
        Ok(strategy.violations_count)
    });
    assert!(
        violations_count > 0,
        "reproduction should find the violation again"
    );
}

/// The bundled default specification must drive the mock app without
/// violations or driver errors.
#[test]
fn default_specification_smoke() {
    setup();

    let violations_count = with_timeout(|| {
        let specification = Specification {
            module_specifier: "@antithesishq/bombadil/swiftui/defaults"
                .to_string(),
        };
        let (driver, verifier) = SwiftUIDriver::launch(
            specification,
            spawn_target(),
            Duration::from_secs(20),
            Duration::from_millis(1),
            Duration::from_secs(10),
        )?;
        let runner = Runner::new(driver, verifier);
        let mut strategy = StepCappedStrategy {
            rng: rand::rng(),
            violations_count: 0,
            steps_remaining: 30,
        };
        runner.run(&mut strategy)?;
        Ok(strategy.violations_count)
    });
    assert_eq!(violations_count, 0);
}

struct StepCappedStrategy {
    rng: ThreadRng,
    violations_count: u64,
    steps_remaining: usize,
}

impl RunStrategy<SwiftUIDriver> for StepCappedStrategy {
    type StopValue = ();

    fn on_new_state(
        &mut self,
        state: &SwiftUIState,
        tree: Tree<SwiftUIActionTemplate>,
        _last_action: Option<&SwiftUIAction>,
        _snapshots: &[Snapshot],
        properties: PropertiesState<'_>,
    ) -> Result<ControlFlow<(), SwiftUIAction>> {
        self.violations_count += properties.violations.len() as u64;
        if state.exit_status.is_some() || self.steps_remaining == 0 {
            return Ok(ControlFlow::Stop(()));
        }
        self.steps_remaining -= 1;
        let tree = tree
            .prune()
            .ok_or_else(|| anyhow::anyhow!("no actions available"))?;
        Ok(ControlFlow::Continue(
            tree.pick(&mut self.rng)?.generate(&mut self.rng),
        ))
    }

    fn on_interrupted(&mut self) -> Result<()> {
        Ok(())
    }
}
