use std::io::Write;
use std::sync::Once;
use std::time::Duration;

use anyhow::{Result, anyhow};
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

#[tokio::test]
async fn test_eventually_ready() -> Result<()> {
    setup();

    let specification_source = r#"
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
"#;

    let mut specification_file = NamedTempFile::with_suffix(".ts")?;
    specification_file.write_all(specification_source.as_bytes())?;

    let specification = Specification {
        module_specifier: specification_file.path().display().to_string(),
    };

    let size = TerminalSize {
        columns: 80,
        rows: 24,
    };
    let program = "sh";
    let args = vec!["-c".to_string(), "printf 'ready\\n'".to_string()];

    // The driver and runner are synchronous and block on channels, which
    // panics inside a Tokio runtime, so run them on a blocking thread. The
    // timeout around the join handle is an infrastructure safety net.
    let run_handle = tokio::task::spawn_blocking(move || -> Result<u64> {
        let (driver, verifier) = TerminalDriver::launch(
            specification,
            size,
            MAX_SCROLLBACK,
            program,
            &args,
        )?;
        let runner = Runner::new(driver, verifier);
        let mut strategy = IntegrationTestStrategy::default();
        runner.run(&mut strategy)?;
        Ok(strategy.violations_count)
    });

    let violations_count = tokio::time::timeout(TEST_TIMEOUT, run_handle)
        .await
        .map_err(|_| {
            anyhow!("terminal integration test hung past {TEST_TIMEOUT:?}")
        })?
        .map_err(|join_error| {
            anyhow!("runner task panicked: {join_error}")
        })??;

    assert_eq!(
        violations_count, 0,
        "expected zero violations, got {violations_count}"
    );
    Ok(())
}

#[tokio::test]
async fn test_styled_cells() -> Result<()> {
    setup();

    // The spec reads styling off the first occupied cell, so it exercises the
    // `style` shape exposed in `terminal/index.ts`: the `Color` foreground and
    // the `Attributes` bit flags (via `Attributes.has`). This also verifies the
    // enum + namespace merge survives the oxc TypeScript lowering at runtime.
    let specification_source = r#"
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
"#;

    let mut specification_file = NamedTempFile::with_suffix(".ts")?;
    specification_file.write_all(specification_source.as_bytes())?;

    let specification = Specification {
        module_specifier: specification_file.path().display().to_string(),
    };

    let size = TerminalSize {
        columns: 80,
        rows: 24,
    };
    let program = "sh";
    // Bold (SGR 1) red (SGR 31) text, then reset.
    let args = vec![
        "-c".to_string(),
        "printf '\\033[1;31mERROR\\033[0m\\n'".to_string(),
    ];

    let run_handle = tokio::task::spawn_blocking(move || -> Result<u64> {
        let (driver, verifier) = TerminalDriver::launch(
            specification,
            size,
            MAX_SCROLLBACK,
            program,
            &args,
        )?;
        let runner = Runner::new(driver, verifier);
        let mut strategy = IntegrationTestStrategy::default();
        runner.run(&mut strategy)?;
        Ok(strategy.violations_count)
    });

    let violations_count = tokio::time::timeout(TEST_TIMEOUT, run_handle)
        .await
        .map_err(|_| {
            anyhow!("terminal integration test hung past {TEST_TIMEOUT:?}")
        })?
        .map_err(|join_error| {
            anyhow!("runner task panicked: {join_error}")
        })??;

    assert_eq!(
        violations_count, 0,
        "expected zero violations, got {violations_count}"
    );
    Ok(())
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
