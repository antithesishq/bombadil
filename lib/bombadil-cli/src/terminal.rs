use std::time::{Duration, SystemTime};
use std::{collections::VecDeque, path::PathBuf, process::exit};

use anyhow::{Result, anyhow, bail};
use bombadil::runner::Runner;
use bombadil::specification::verifier::Specification;
use bombadil_schema::Time;
use bombadil_terminal::driver::{Size, TerminalAction, TerminalDriver};
use bombadil_terminal::trace::{TerminalTraceEntry, TraceWriter};
use bombadil_terminal::{TerminalStrategy, TerminalTestMode};
use tempfile::TempDir;
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::duration;

mod defaults {
    pub const COLUMNS: u16 = 100;
    pub const ROWS: u16 = 40;
    pub const SCROLLBACK_LINES_MAX: u16 = 100;
}

#[derive(clap::Subcommand)]
pub enum Command {
    /// [EXPERIMENTAL] Test the given program against a TypeScript specification
    Test {
        /// Path to a TypeScript specification file (uses the
        /// `@antithesishq/bombadil/terminal` API). Required: there is no
        /// default terminal specification yet.
        #[arg(long = "specification")]
        specification_file: PathBuf,
        /// Whether to exit the test when first failing property is found (useful in development and CI)
        #[arg(long)]
        exit_on_violation: bool,
        /// Maximum time to run the test. Accepts a number with a unit suffix:
        /// s (seconds), m (minutes), h (hours), or d (days). Examples: 30s, 5m, 2h, 1d.
        #[arg(long, value_parser = duration::parse_duration)]
        time_limit: Option<Duration>,
        /// Terminal columns at startup
        #[arg(long, default_value_t = defaults::COLUMNS)]
        columns: u16,
        /// Terminal rows at startup
        #[arg(long, default_value_t = defaults::ROWS)]
        rows: u16,
        /// Maximum line count to keep in scrollback buffer
        #[arg(long, default_value_t = defaults::SCROLLBACK_LINES_MAX)]
        scrollback_lines_max: u16,
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
            exit_on_violation,
            time_limit,
            columns,
            rows,
            scrollback_lines_max,
            output_path,
            reproduce,
            command,
        } => {
            let run_test = async || -> Result<()> {
                let (program, args) = match &command[..] {
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
                };

                let output_path = resolve_output_path(output_path)?;
                let writer =
                    TraceWriter::initialize(output_path.clone()).await?;

                let mode = match reproduce {
                    Some(path) => TerminalTestMode::Reproduce(
                        load_reproduce_actions(&path).await?,
                    ),
                    None => TerminalTestMode::RandomWalk,
                };

                let (driver, verifier) = TerminalDriver::launch(
                    specification,
                    Size { columns, rows },
                    scrollback_lines_max as usize,
                    program,
                    args,
                )
                .await?;

                let test_start = SystemTime::now();
                let deadline = time_limit.map(|d| test_start + d);

                let runner = Runner::new(driver, verifier);
                let mut strategy = TerminalStrategy {
                    mode,
                    writer: Some(writer),
                    test_start: Some(Time::from_system_time(test_start)),
                    violations_count: 0,
                    exit_on_violation,
                    deadline,
                };
                let _ = runner.run(&mut strategy).await?;

                println!("\nTrace written to: {}", output_path.display());

                if strategy.violations_count > 0 {
                    bail!(
                        "{} violation(s) reported",
                        strategy.violations_count
                    );
                }
                Ok(())
            };

            if let Err(error) = run_test().await {
                eprintln!("\n\nterminal test failed: {error}");
                exit(1);
            }
        }
    }
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
