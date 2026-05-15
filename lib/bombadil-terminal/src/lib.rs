use std::{path::PathBuf, process::exit};

use anyhow::{Result, bail};
use bombadil::runner::{
    ControlFlow, PropertyViolation, RunStrategy, Runner,
};
use bombadil::specification::convert::ToSchema;
use bombadil::specification::domain::Snapshot;
use bombadil::specification::verifier::Specification;
use bombadil::styled;
use bombadil::tree::Tree;

use crate::driver::{Size, TerminalAction, TerminalDriver, TerminalState};

pub mod driver;
pub mod pty;

const DEFAULT_COLUMNS: u16 = 100;
const DEFAULT_ROWS: u16 = 40;
const MAX_SCROLLBACK: u32 = 1_000;

#[derive(clap::Subcommand)]
pub enum Command {
    /// [EXPERIMENTAL] Test the given program against a TypeScript specification
    Test {
        /// Path to a TypeScript specification file (uses the
        /// `@antithesishq/bombadil/terminal` API). Required: there is no
        /// default terminal spec yet.
        #[arg(long = "spec")]
        specification_file: PathBuf,
        /// Terminal columns at startup
        #[arg(long, default_value_t = DEFAULT_COLUMNS)]
        columns: u16,
        /// Terminal rows at startup
        #[arg(long, default_value_t = DEFAULT_ROWS)]
        rows: u16,
        /// The command to run as the system under test. Everything after
        /// `--` is forwarded as program + arguments.
        #[clap(trailing_var_arg = true)]
        command: Vec<String>,
    },
}

pub async fn run(command: Command) {
    match command {
        Command::Test {
            specification_file,
            columns,
            rows,
            command,
        } => {
            if let Err(error) = run_test(
                specification_file,
                Size { columns, rows },
                &command,
            )
            .await
            {
                eprintln!("\n\nterminal test failed: {error}");
                exit(1);
            }
        }
    }
}

async fn run_test(
    specification_file: PathBuf,
    size: Size,
    command: &[String],
) -> Result<()> {
    let (program, args) = match command {
        [program, args @ ..] => (program.as_str(), args),
        _ => bail!("expected `<program> [args...]` after `--`"),
    };

    let specification = Specification {
        module_specifier: specification_file.display().to_string(),
        runtime_module: "@antithesishq/bombadil/terminal".to_string(),
    };

    let (driver, verifier) = TerminalDriver::launch(
        specification,
        size,
        MAX_SCROLLBACK,
        program,
        args,
    )
    .await?;

    let runner = Runner::new(driver, verifier);
    let mut strategy = TerminalStrategy::default();
    let _ = runner.run(&mut strategy).await?;

    if strategy.violations_count > 0 {
        bail!("{} violation(s) reported", strategy.violations_count);
    }
    Ok(())
}

#[derive(Default)]
struct TerminalStrategy {
    test_start: Option<bombadil_schema::Time>,
    violations_count: u64,
}

impl RunStrategy<TerminalDriver> for TerminalStrategy {
    type StopValue = ();

    async fn on_new_state(
        &mut self,
        state: &TerminalState,
        _last_action: Option<&TerminalAction>,
        _snapshots: &[Snapshot],
        violations: &[PropertyViolation],
    ) -> Result<ControlFlow<()>> {
        let test_start = *self.test_start.get_or_insert(
            bombadil_schema::Time::from_system_time(state.timestamp),
        );

        self.violations_count += violations.len() as u64;
        for violation in violations {
            log::info!("violation of property `{}`", violation.name);
            let schema_violation = violation.to_schema();
            let markup =
                bombadil_schema::markup::render_violation(&schema_violation);
            let text = styled::markup_to_styled(&markup, test_start);
            println!(
                "\n{}\n\n{}\n",
                styled::maybe_red(styled::maybe_bold(format!(
                    "{} was violated:",
                    violation.name
                ))),
                text
            );
        }

        if state.finished {
            log::info!("terminal process exited, stopping");
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
