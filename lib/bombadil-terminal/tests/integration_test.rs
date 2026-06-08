use std::io::Write;
use std::sync::Once;
use std::sync::mpsc;
use std::time::Duration;

use anyhow::Result;
use bombadil::runner::{ControlFlow, PropertiesState, RunStrategy, Runner};
use bombadil::specification::domain::Snapshot;
use bombadil::specification::verifier::Specification;
use bombadil::tree::Tree;
use bombadil_schema::TerminalSize;
use bombadil_terminal::driver::{TerminalAction, TerminalDriver};
use bombadil_terminal::state::TerminalState;
use tempfile::NamedTempFile;

const MAX_SCROLLBACK: usize = 1_000;
const TEST_TIMEOUT: Duration = Duration::from_secs(60);

static INIT: Once = Once::new();

fn setup() {
    INIT.call_once(|| {
        let env = env_logger::Env::default().default_filter_or("debug");
        env_logger::Builder::from_env(env)
            .format_timestamp_millis()
            .is_test(true)
            .try_init()
            .ok();
    });
}

struct TerminalIntegrationTest {
    program: String,
    args: Vec<String>,
    size: TerminalSize,
    max_scrollback: usize,
    specification_source: String,
}

impl TerminalIntegrationTest {
    fn new(program: &str, args: &[&str]) -> Self {
        Self {
            program: program.to_string(),
            args: args.iter().map(|arg| arg.to_string()).collect(),
            size: TerminalSize {
                columns: 80,
                rows: 24,
            },
            max_scrollback: MAX_SCROLLBACK,
            specification_source: String::new(),
        }
    }

    fn specification(mut self, source: &str) -> Self {
        self.specification_source = source.to_string();
        self
    }

    /// Runs the specification against the program and asserts zero property
    /// violations.
    fn run(self) {
        setup();
        let TerminalIntegrationTest {
            program,
            args,
            size,
            max_scrollback,
            specification_source,
        } = self;

        let mut specification_file = NamedTempFile::with_suffix(".ts").unwrap();
        specification_file
            .write_all(specification_source.as_bytes())
            .unwrap();
        let specification = Specification {
            module_specifier: specification_file.path().display().to_string(),
        };

        let (sender, receiver) = mpsc::channel();
        let _ = std::thread::spawn(move || {
            // Keep the spec file alive for the whole run.
            let _specification_file = specification_file;
            let result = (|| -> Result<u64> {
                let (driver, verifier) = TerminalDriver::launch(
                    specification,
                    size,
                    max_scrollback,
                    &program,
                    &args,
                )?;
                let runner = Runner::new(driver, verifier);
                let mut strategy = IntegrationTestStrategy::default();
                runner.run(&mut strategy)?;
                Ok(strategy.violations_count)
            })();
            let _ = sender.send(result);
        });

        let violations_count = receiver
            .recv_timeout(TEST_TIMEOUT)
            .unwrap_or_else(|_| {
                panic!("terminal integration test hung past {TEST_TIMEOUT:?}")
            })
            .expect("terminal runner failed");

        assert_eq!(
            violations_count, 0,
            "expected zero violations, got {violations_count}"
        );
    }
}

#[test]
fn test_eventually_ready() {
    TerminalIntegrationTest::new("sh", &["-c", "printf 'ready\\n'"])
        .specification(
            r#"
import { eventually } from "@antithesishq/bombadil";
import { actions, extract } from "@antithesishq/bombadil/terminal";

function cellToString(cell) {
    switch (cell) {
        case "Empty":
            return " ";
        case "Continuation":
            return "";
        default:
            return cell.Occupied.contents;
    }
}

// Exercises the fast `rowText` path.
const screen = extract((state) => {
    const lines = [];
    for (let index = 0; index < state.grid.size.rows; index++) {
        lines.push(state.grid.rowText(index));
    }
    return lines.join("\n");
});

// Exercises the cell-level `row` path so both grid APIs stay covered.
const screenFromCells = extract((state) => {
    const lines = [];
    for (let index = 0; index < state.grid.size.rows; index++) {
        lines.push(state.grid.row(index).map(cellToString).join(""));
    }
    return lines.join("\n");
});

export const eventuallyReady = eventually(
    () => screen.current.includes("ready"),
);

export const eventuallyReadyFromCells = eventually(
    () => screenFromCells.current.includes("ready"),
);

export const noop = actions(() => [{ TypeText: { text: "" } }]);
"#,
        )
        .run();
}

#[test]
fn test_styled_cells() {
    TerminalIntegrationTest::new(
        "sh",
        // Bold (SGR 1) red (SGR 31) text, then reset.
        &["-c", "printf '\\033[1;31mERROR\\033[0m\\n'"],
    )
    .specification(
        r#"
import { eventually } from "@antithesishq/bombadil";
import { actions, extract, Attributes } from "@antithesishq/bombadil/terminal";

const firstStyledCell = extract((state) => {
    for (let row = 0; row < state.grid.size.rows; row++) {
        for (const cell of state.grid.row(row)) {
            if (cell !== "Empty" && cell !== "Continuation") {
                return cell.Occupied.style;
            }
        }
    }
    return null;
});

export const eventuallyBoldRed = eventually(() => {
    const style = firstStyledCell.current;
    return (
        style !== null &&
        Attributes.has(style, Attributes.Bold) &&
        !Attributes.has(style, Attributes.Italic) &&
        typeof style.foregroundColor === "object" &&
        "Palette" in style.foregroundColor
    );
});

export const noop = actions(() => [{ TypeText: { text: "" } }]);
"#,
    )
    .run();
}

#[derive(Default)]
struct IntegrationTestStrategy {
    violations_count: u64,
}

impl RunStrategy<TerminalDriver> for IntegrationTestStrategy {
    type StopValue = ();

    fn on_new_state(
        &mut self,
        state: &TerminalState,
        tree: Tree<TerminalAction>,
        _last_action: Option<&TerminalAction>,
        _snapshots: &[Snapshot],
        properties: PropertiesState<'_>,
    ) -> Result<ControlFlow<(), TerminalAction>> {
        self.violations_count += properties.violations.len() as u64;
        if properties.all_definite {
            return Ok(ControlFlow::Stop(()));
        }
        if state.terminated {
            return Ok(ControlFlow::Stop(()));
        }
        Ok(ControlFlow::Continue(tree.pick(&mut rand::rng())?.clone()))
    }

    fn on_interrupted(&mut self) -> Result<()> {
        Ok(())
    }
}
