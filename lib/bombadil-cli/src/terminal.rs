use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};
use std::{collections::VecDeque, path::PathBuf, process::exit};

use antithesis_sdk::random::AntithesisRng;
use anyhow::{Result, anyhow, bail};
use bombadil::driver::{InterfaceDriver, NoopTraceWriter, RunId};
use bombadil::fuzzer::FuzzOptions;
use bombadil::specification::convert::ToInternal;
use bombadil::specification::verifier::Specification;
use bombadil::{antithesis, fuzzer, runner};
use bombadil_schema::Time;
use bombadil_schema::terminal::{
    ProcessExitStatus, TerminalSize, TerminalTraceEntry,
};
use bombadil_terminal::driver::{
    TerminalAction, TerminalDriver, TerminalProgramOptions,
};
use bombadil_terminal::trace::{TerminalOutputWriter, TerminalTraceWriter};
use bombadil_terminal::{TerminalStrategy, TerminalTestMode};
use std::fs::File;
use std::io::{BufRead, BufReader};
use tempfile::TempDir;

use crate::duration;

mod defaults {
    pub const COLUMNS: u16 = 100;
    pub const ROWS: u16 = 40;
    pub const SCROLLBACK_LINES_MAX: u16 = 100;
    pub const QUIESCENCE_TIMEOUT_MS: u64 = 5;
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
        /// How long to wait (in milliseconds) for the program to stop emitting
        /// output before extracting the next state. Lower values increase
        /// throughput but risk sampling mid-render; higher values give the
        /// program more time to finish drawing.
        #[arg(long, default_value_t = defaults::QUIESCENCE_TIMEOUT_MS)]
        quiescence_timeout_ms: u64,
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

    /// [EXPERIMENTAL] Fuzz (running many short test runs) the given program against a
    /// TypeScript specification
    #[command(hide = true)]
    Fuzz {
        /// Path to a TypeScript specification file (uses the
        /// `@antithesishq/bombadil/terminal` API). Unless specified, Bombadil will
        /// use the default specification for terminal UIs.
        #[arg(long = "specification")]
        specification_file: Option<PathBuf>,
        /// Whether to exit the test when first failing property is found (useful in development and CI)
        #[arg(long)]
        exit_on_violation: bool,
        /// Maximum time to run an individual test run. Accepts a number with a unit suffix:
        /// s (seconds), m (minutes), h (hours), or d (days). Examples: 30s, 5m, 2h, 1d.
        #[arg(long, value_parser = duration::parse_duration, default_value = "10s")]
        time_limit_run: Duration,
        /// Maximum time to run the full fuzzing campaign. Accepts a number with a unit suffix:
        /// s (seconds), m (minutes), h (hours), or d (days). Examples: 30s, 5m, 2h, 1d.
        #[arg(long, value_parser = duration::parse_duration, default_value = "5m")]
        time_limit_fuzz: Duration,

        /// Whether to apply swarm testing to actions. Otherwise all actions are enabled.
        #[arg(long)]
        swarm: bool,

        /// Terminal columns at startup
        #[arg(long, default_value_t = defaults::COLUMNS)]
        columns: u16,
        /// Terminal rows at startup
        #[arg(long, default_value_t = defaults::ROWS)]
        rows: u16,
        /// Maximum line count to keep in scrollback buffer
        #[arg(long, default_value_t = defaults::SCROLLBACK_LINES_MAX)]
        scrollback_lines_max: u16,
        /// How long to wait (in milliseconds) for the program to stop emitting
        /// output before extracting the next state. Lower values increase
        /// throughput but risk sampling mid-render; higher values give the
        /// program more time to finish drawing.
        #[arg(long, default_value_t = defaults::QUIESCENCE_TIMEOUT_MS)]
        quiescence_timeout_ms: u64,
        /// The command to run as the system under test. Everything after
        /// `--` is forwarded as program + arguments.
        #[clap(trailing_var_arg = true)]
        command: Vec<String>,
        /// Where to store output data (trace.jsonl). Defaults to a
        /// fresh temporary directory.
        #[arg(long)]
        output_path: Option<PathBuf>,
        /// Overwrite any existing trace at --output-path. Without this
        /// flag, Bombadil refuses to write when trace.jsonl already exists.
        #[arg(long)]
        output_path_overwrite: bool,
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
            quiescence_timeout_ms,
            output_path,
            output_path_overwrite,
            reproduce,
            command,
        } => {
            let run_test = || -> Result<()> {
                let (program, arguments) = match &command[..] {
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

                let mode = match reproduce {
                    Some(path) => TerminalTestMode::Reproduce(
                        load_reproduce_actions(&path)?,
                    ),
                    None => TerminalTestMode::RandomWalk,
                };

                let program_options = TerminalProgramOptions {
                    size: TerminalSize { columns, rows },
                    scrollback_lines_max: scrollback_lines_max as usize,
                    quiescence_timeout: Duration::from_millis(
                        quiescence_timeout_ms,
                    ),
                    program: program.to_string(),
                    arguments: arguments.to_vec(),
                };
                let driver =
                    TerminalDriver::new(specification, program_options)?;

                let test_start = SystemTime::now();
                let deadline = time_limit.map(|d| test_start + d);

                let interrupted = Arc::new(AtomicBool::new(false));
                {
                    let interrupted = interrupted.clone();
                    ctrlc::set_handler(move || {
                        interrupted.store(true, Ordering::SeqCst);
                    })?;
                }

                let mut strategy = TerminalStrategy {
                    rng: AntithesisRng,
                    mode,
                    test_start: Some(Time::from_system_time(test_start)),
                    violations_count: 0,
                    exit_on_violation,
                    deadline,
                    states_seen: 0,
                };
                let (mut session, verifier) =
                    driver.new_session(RunId::default())?;

                let exit_reason = if antithesis::is_in_guest() {
                    runner::run(
                        &mut session,
                        &mut strategy,
                        verifier,
                        &mut NoopTraceWriter,
                        interrupted,
                    )?
                } else {
                    let mut trace_writer = TerminalTraceWriter::initialize(
                        output_path.clone(),
                        output_path_overwrite,
                    )?;
                    runner::run(
                        &mut session,
                        &mut strategy,
                        verifier,
                        &mut trace_writer,
                        interrupted,
                    )?
                };

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
                println!("Output written to: {}", output_path.display());

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

                if let Some(source) = error.source() {
                    eprintln!("\nCauses:");

                    for cause in anyhow::Chain::new(source) {
                        eprintln!("  - {cause}");
                    }
                }

                exit(1);
            }
        }
        Command::Fuzz {
            specification_file,
            exit_on_violation: _,
            time_limit_run,
            time_limit_fuzz,
            swarm,
            columns,
            rows,
            scrollback_lines_max,
            quiescence_timeout_ms,
            command,
            output_path,
            output_path_overwrite,
        } => {
            let run_fuzz = || {
                if antithesis::is_in_guest() {
                    bail!(
                        "bombadil fuzzing mode is not available in antithesis; use `test` or `test-external`"
                    );
                };

                let (program, arguments) = match &command[..] {
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

                let output_writer = TerminalOutputWriter {
                    root_path: output_path.clone(),
                    overwrite: output_path_overwrite,
                };

                let program_options = TerminalProgramOptions {
                    size: TerminalSize { columns, rows },
                    scrollback_lines_max: scrollback_lines_max as usize,
                    quiescence_timeout: Duration::from_millis(
                        quiescence_timeout_ms,
                    ),
                    program: program.to_string(),
                    arguments: arguments.to_vec(),
                };
                let driver = Arc::new(TerminalDriver::new(
                    specification,
                    program_options,
                )?);

                let interrupted = Arc::new(AtomicBool::new(false));
                {
                    let interrupted = interrupted.clone();
                    ctrlc::set_handler(move || {
                        interrupted.store(true, Ordering::SeqCst);
                    })?;
                }

                fuzzer::fuzz(FuzzOptions {
                    rng: AntithesisRng,
                    driver,
                    interrupted,
                    time_limit_fuzz,
                    time_limit_run,
                    swarm,
                    output_writer,
                })?;

                Ok(())
            };

            if let Err(error) = run_fuzz() {
                eprintln!("\n\nterminal fuzz failed: {error}");

                if let Some(source) = error.source() {
                    eprintln!("\nCauses:");

                    for cause in anyhow::Chain::new(source) {
                        eprintln!("  - {cause}");
                    }
                }

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
            actions.push_back(action.to_internal());
        }
    }
    Ok(actions)
}
