use std::{collections::VecDeque, path::PathBuf, process::exit};

use anyhow::{Result, anyhow, bail};
use bombadil::runner::{ControlFlow, PropertyViolation, RunStrategy, Runner};
use bombadil::specification::convert::ToSchema;
use bombadil::specification::domain::Snapshot;
use bombadil::specification::verifier::Specification;
use bombadil::styled;
use bombadil::tree::Tree;
use tempfile::TempDir;
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::driver::{Size, TerminalAction, TerminalDriver, TerminalState};
use crate::trace::{TerminalTraceEntry, TraceWriter};

pub mod driver;
pub mod pty;
pub mod render;
pub mod trace;

const DEFAULT_COLUMNS: u16 = 100;
const DEFAULT_ROWS: u16 = 40;
const MAX_SCROLLBACK: usize = 1_000;

#[derive(clap::Subcommand)]
pub enum Command {
    /// [EXPERIMENTAL] Test the given program against a TypeScript specification
    Test {
        /// Path to a TypeScript specification file (uses the
        /// `@antithesishq/bombadil/terminal` API). Required: there is no
        /// default terminal specification yet.
        #[arg(long = "specification")]
        specification_file: PathBuf,
        /// Terminal columns at startup
        #[arg(long, default_value_t = DEFAULT_COLUMNS)]
        columns: u16,
        /// Terminal rows at startup
        #[arg(long, default_value_t = DEFAULT_ROWS)]
        rows: u16,
        /// Where to store output data (trace.jsonl). Defaults to a
        /// fresh temporary directory.
        #[arg(long)]
        output_path: Option<PathBuf>,
        /// Reproduce a previous test run from a trace file (file path
        /// or directory containing `trace.jsonl`). Replays the recorded
        /// actions in order instead of generating new ones.
        #[arg(long, value_name = "TRACE_FILE")]
        reproduce: Option<PathBuf>,
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
            output_path,
            reproduce,
            command,
        } => {
            if let Err(error) = run_test(
                specification_file,
                Size { columns, rows },
                output_path,
                reproduce,
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
    output_path: Option<PathBuf>,
    reproduce: Option<PathBuf>,
    command: &[String],
) -> Result<()> {
    let (program, args) = match command {
        [program, args @ ..] => (program.as_str(), args),
        _ => bail!("expected `<program> [args...]` after `--`"),
    };

    // Prepend "./" for relative paths that don't already start with "."
    // so the bundler treats them as paths rather than bare specifiers.
    let specification_file = if specification_file.is_relative()
        && !specification_file.starts_with(".")
    {
        PathBuf::from(".").join(specification_file)
    } else {
        specification_file
    };

    let specification = Specification {
        module_specifier: specification_file.display().to_string(),
        runtime_module: "@antithesishq/bombadil/terminal".to_string(),
    };

    let output_path = resolve_output_path(output_path)?;
    let writer = TraceWriter::initialize(output_path.clone()).await?;

    let mode = match reproduce {
        Some(path) => {
            TerminalTestMode::Reproduce(load_reproduce_actions(&path).await?)
        }
        None => TerminalTestMode::RandomWalk,
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
    let mut strategy = TerminalStrategy {
        mode,
        writer: Some(writer),
        test_start: None,
        violations_count: 0,
    };
    let _ = runner.run(&mut strategy).await?;

    println!("\nTrace written to: {}", output_path.display());

    if strategy.violations_count > 0 {
        bail!("{} violation(s) reported", strategy.violations_count);
    }
    Ok(())
}

fn resolve_output_path(output_path: Option<PathBuf>) -> Result<PathBuf> {
    match output_path {
        Some(path) => Ok(path),
        None => Ok(TempDir::with_prefix("bombadil_terminal_")?
            .keep()
            .to_path_buf()),
    }
}

async fn load_reproduce_actions(
    path: &std::path::Path,
) -> Result<VecDeque<TerminalAction>> {
    let trace_file_path = if path.is_dir() {
        path.join("trace.jsonl")
    } else {
        path.to_path_buf()
    };
    let file = File::open(&trace_file_path).await.map_err(|error| {
        anyhow!(
            "failed to open trace file {}: {}",
            trace_file_path.display(),
            error
        )
    })?;
    let mut lines = BufReader::new(file).lines();
    let mut actions: VecDeque<TerminalAction> = VecDeque::new();
    while let Some(line) = lines.next_line().await? {
        let entry: TerminalTraceEntry = serde_json::from_str(&line)?;
        if let Some(action) = entry.action {
            actions.push_back(action);
        }
    }
    Ok(actions)
}

enum TerminalTestMode {
    RandomWalk,
    Reproduce(VecDeque<TerminalAction>),
}

struct TerminalStrategy {
    mode: TerminalTestMode,
    writer: Option<TraceWriter>,
    test_start: Option<bombadil_schema::Time>,
    violations_count: u64,
}

impl RunStrategy<TerminalDriver> for TerminalStrategy {
    type StopValue = ();

    async fn on_new_state(
        &mut self,
        state: &TerminalState,
        last_action: Option<&TerminalAction>,
        snapshots: &[Snapshot],
        violations: &[PropertyViolation],
    ) -> Result<ControlFlow<()>> {
        let test_start = *self.test_start.get_or_insert(
            bombadil_schema::Time::from_system_time(state.timestamp),
        );

        if let Some(action) = last_action {
            println!(
                "{} {}",
                render::format_timestamp(state.timestamp, test_start),
                render::format_action(action),
            );
        }

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

        if let Some(writer) = self.writer.as_mut() {
            writer
                .write(state, last_action, snapshots, violations)
                .await?;
        }

        if let TerminalTestMode::Reproduce(remaining) = &self.mode
            && remaining.is_empty()
        {
            log::info!("reproduction complete, stopping");
            return Ok(ControlFlow::Stop(()));
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
        match &mut self.mode {
            TerminalTestMode::RandomWalk => {
                Ok(tree.pick(&mut rand::rng())?.clone())
            }
            TerminalTestMode::Reproduce(actions) => {
                let original = actions.pop_front().ok_or_else(|| {
                    anyhow!("no remaining actions in reproduce queue")
                })?;
                // Terminal actions are pure data; reproduction succeeds
                // whenever the spec's current generator includes an
                // identical action. If it doesn't, bail rather than
                // silently diverging.
                let available = tree.values();
                if available.iter().any(|a| actions_match(a, &original)) {
                    Ok(original)
                } else {
                    bail!(
                        "reproduce: action {:?} not produced by the spec at this state",
                        original
                    );
                }
            }
        }
    }

    async fn on_interrupted(&mut self) -> Result<()> {
        Ok(())
    }
}

fn actions_match(a: &TerminalAction, b: &TerminalAction) -> bool {
    serde_json::to_value(a).ok() == serde_json::to_value(b).ok()
}
