use std::io::Write;
use std::sync::Once;
use std::time::Duration;

use anyhow::{Result, anyhow};
use bombadil::runner::{ControlFlow, PropertyViolation, RunStrategy, Runner};
use bombadil::specification::domain::Snapshot;
use bombadil::specification::verifier::Specification;
use bombadil::tree::Tree;
use bombadil_terminal::driver::{
    Size, TerminalAction, TerminalDriver, TerminalState,
};
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

/// Smoke test: a trivial PTY program prints "ready" and exits. The
/// specification's `eventually` property should resolve true on the
/// first observed state (the rendered grid contains "ready"), letting
/// the runner stop without violations.
#[tokio::test]
async fn smoke_eventually_ready() -> Result<()> {
    setup();

    let specification_source = r#"
import { eventually } from "@antithesishq/bombadil";
import { actions, extract } from "@antithesishq/bombadil/terminal";

const screen = extract((state) => state.rows.join("\n"));

export const eventuallyReady = eventually(
    () => screen.current.includes("ready"),
);

// A no-op action so the specification satisfies the verifier's "at
// least one action generator" requirement. The smoke property should
// resolve before any action is picked, but if it doesn't, this lets the
// runner keep ticking without applying real input.
export const noop = actions(() => [{ TypeText: { text: "" } }]);
"#;

    let mut specification_file = NamedTempFile::with_suffix(".ts")?;
    specification_file.write_all(specification_source.as_bytes())?;

    let specification = Specification {
        module_specifier: specification_file.path().display().to_string(),
        runtime_module: "@antithesishq/bombadil".to_string(),
    };

    let size = Size {
        columns: 80,
        rows: 24,
    };
    let program = "sh";
    let args = vec!["-c".to_string(), "printf 'ready\\n'".to_string()];

    let (driver, verifier) = TerminalDriver::launch(
        specification,
        size,
        MAX_SCROLLBACK,
        program,
        &args,
    )
    .await?;

    let runner = Runner::new(driver, verifier);
    let mut strategy = SmokeStrategy::default();

    let result = tokio::time::timeout(TEST_TIMEOUT, runner.run(&mut strategy))
        .await
        .map_err(|_| {
            anyhow!("terminal smoke test hung past {TEST_TIMEOUT:?}")
        })?;
    result?;

    assert_eq!(
        strategy.violations_count, 0,
        "expected zero violations, got {}",
        strategy.violations_count
    );
    assert!(
        strategy.observed_ready,
        "expected at least one state with `ready` rendered, but the property never observed it"
    );
    Ok(())
}

#[derive(Default)]
struct SmokeStrategy {
    violations_count: u64,
    observed_ready: bool,
}

impl RunStrategy<TerminalDriver> for SmokeStrategy {
    type StopValue = ();

    async fn on_new_state(
        &mut self,
        state: &TerminalState,
        _last_action: Option<&TerminalAction>,
        _snapshots: &[Snapshot],
        violations: &[PropertyViolation],
    ) -> Result<ControlFlow<()>> {
        self.violations_count += violations.len() as u64;
        if state.rows.iter().any(|row| row.contains("ready")) {
            self.observed_ready = true;
        }
        if state.finished {
            return Ok(ControlFlow::Stop(()));
        }
        Ok(ControlFlow::Continue)
    }

    async fn pick_action(
        &mut self,
        tree: Tree<TerminalAction>,
    ) -> Result<TerminalAction> {
        Ok(tree.pick(&mut rand::rng())?.clone())
    }

    async fn on_interrupted(&mut self) -> Result<()> {
        Ok(())
    }
}
