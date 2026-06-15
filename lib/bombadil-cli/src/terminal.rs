use std::time::{Duration, SystemTime};
use std::{collections::VecDeque, path::PathBuf, process::exit};

use antithesis_sdk::random::AntithesisRng;
use anyhow::{Result, anyhow, bail};
use bombadil::runner::Runner;
use bombadil::specification::verifier::Specification;
use bombadil_schema::{ProcessExitStatus, TerminalSize, Time};
use bombadil_terminal::driver::{TerminalAction, TerminalDriver};
use bombadil_terminal::trace::{TerminalTraceEntry, TraceWriter};
use bombadil_terminal::{TerminalStrategy, TerminalTestMode};
use std::fs::File;
use std::io::{BufRead, BufReader};
use tempfile::TempDir;

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
        /// `@antithesishq/bombadil/terminal` API). Unless specified, Bombadil will
        /// use the default specification for terminal UIs.
        #[arg(long = "specification")]
        specification_file: Option<PathBuf>,
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
        /// Overwrite any existing trace at --output-path. Without this
        /// flag, Bombadil refuses to write when trace.jsonl already exists.
        #[arg(long)]
        output_path_overwrite: bool,
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

pub fn run(command: Command) {
    match command {
        Command::Test {
            specification_file,
            exit_on_violation,
            time_limit,
            columns,
            rows,
            scrollback_lines_max,
            output_path,
            output_path_overwrite,
            reproduce,
            command,
        } => {
            let run_test = || -> Result<()> {
                let (program, args) = match &command[..] {
                    [program, args @ ..] => (program.as_str(), args),
                    _ => bail!("expected `<program> [args...]` after `--`"),
                };

                let specification = if let Some(path) = specification_file {
                    // Prepend "./" for relative paths that don't already start with "."
                    // so the bundler treats them as paths rather than bare specifiers.
                    let path = if path.is_relative() && !path.starts_with(".") {
                        PathBuf::from(".").join(path)
                    } else {
                        path.clone()
                    };

                    Specification {
                        module_specifier: path.display().to_string(),
                    }
                } else {
                    log::info!("using default specification");
                    Specification {
                        module_specifier:
                            "@antithesishq/bombadil/terminal/defaults"
                                .to_string(),
                    }
                };

                let output_path = resolve_output_path(output_path)?;
                let writer = TraceWriter::initialize(
                    output_path.clone(),
                    output_path_overwrite,
                )?;

                let mode = match reproduce {
                    Some(path) => TerminalTestMode::Reproduce(
                        load_reproduce_actions(&path)?,
                    ),
                    None => TerminalTestMode::RandomWalk,
                };

                let (driver, verifier) = TerminalDriver::launch(
                    specification,
                    TerminalSize { columns, rows },
                    scrollback_lines_max as usize,
                    Duration::from_millis(100),
                    program,
                    args,
                )?;

                let test_start = SystemTime::now();
                let deadline = time_limit.map(|d| test_start + d);

                let runner = Runner::new(driver, verifier);
                let mut strategy = TerminalStrategy {
                    rng: AntithesisRng,
                    mode,
                    writer: Some(writer),
                    test_start: Some(Time::from_system_time(test_start)),
                    violations_count: 0,
                    exit_on_violation,
                    deadline,
                    states_seen: 0,
                };
                let exit_reason = runner.run(&mut strategy)?;

                println!();
                match exit_reason {
                    bombadil_terminal::ExitReason::ExitOnViolation => {
                        println!("Exited due to violation")
                    }
                    bombadil_terminal::ExitReason::TimeLimit => {
                        println!("Exited after time limit hit")
                    }
                    bombadil_terminal::ExitReason::Interrupted => {
                        println!("Exited after SIGINT")
                    }
                    bombadil_terminal::ExitReason::Terminated(
                        ProcessExitStatus { code, signal: None },
                    ) => println!(
                        "Exited as process terminated with exit code {code}"
                    ),
                    bombadil_terminal::ExitReason::Terminated(
                        ProcessExitStatus {
                            code,
                            signal: Some(signal),
                        },
                    ) => println!(
                        "Exited as process terminated with exit code {code} after signal {signal}"
                    ),
                    bombadil_terminal::ExitReason::Reproduced => {
                        println!("Exited after reproduction finished")
                    }
                    bombadil_terminal::ExitReason::AllDefinite => {
                        println!("Exited as all properties are definite")
                    }
                };

                println!(
                    "Throughput (state samples/sec): {:.1}",
                    strategy.states_seen as f64
                        / SystemTime::now()
                            .duration_since(test_start)?
                            .as_secs_f64()
                );
                println!("Trace written to: {}", output_path.display());

                if strategy.violations_count > 0 {
                    bail!(
                        "{} violation(s) reported",
                        strategy.violations_count
                    );
                }
                Ok(())
            };

            if let Err(error) = run_test() {
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

fn load_reproduce_actions(
    path: &std::path::Path,
) -> Result<VecDeque<TerminalAction>> {
    let trace_file_path = if path.is_dir() {
        path.join("trace.jsonl")
    } else {
        path.to_path_buf()
    };
    let file = File::open(&trace_file_path).map_err(|error| {
        anyhow!(
            "failed to open trace file {}: {}",
            trace_file_path.display(),
            error
        )
    })?;
    let mut actions: VecDeque<TerminalAction> = VecDeque::new();
    for line in BufReader::new(file).lines() {
        let line = line?;
        let entry: TerminalTraceEntry = serde_json::from_str(&line)?;
        if let Some(action) = entry.action {
            actions.push_back(action);
        }
    }
    Ok(actions)
}
