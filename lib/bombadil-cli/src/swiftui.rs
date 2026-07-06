use std::time::{Duration, SystemTime};
use std::{path::PathBuf, process::exit};

use antithesis_sdk::random::AntithesisRng;
use anyhow::{Result, bail};
use bombadil::runner::Runner;
use bombadil::specification::verifier::Specification;
use bombadil_schema::Time;
use bombadil_schema::swiftui::{ProcessExitStatus, SwiftUIStateSummary};
use bombadil_swiftui::agent::SwiftUITarget;
use bombadil_swiftui::driver::SwiftUIDriver;
use bombadil_swiftui::trace::TraceWriter;
use bombadil_swiftui::{SwiftUIStrategy, SwiftUITestMode};

use crate::{duration, output_path, reproduce};

mod defaults {
    pub const CONNECT_TIMEOUT_SECS: u64 = 30;
    pub const QUIESCENCE_TIMEOUT_MS: u64 = 100;
    pub const STATE_TIMEOUT_SECS: u64 = 10;
}

#[derive(clap::Subcommand)]
pub enum Command {
    /// [EXPERIMENTAL] Test the given SwiftUI app against a TypeScript
    /// specification. The app must link the BombadilAgent Swift package
    /// and call `BombadilAgent.startIfRequested()` on launch.
    Test {
        /// Path to a TypeScript specification file (uses the
        /// `@antithesishq/bombadil/swiftui` API). Unless specified, Bombadil
        /// will use the default specification for SwiftUI apps.
        #[arg(long = "specification")]
        specification_file: Option<PathBuf>,
        /// Whether to exit the test when first failing property is found (useful in development and CI)
        #[arg(long)]
        exit_on_violation: bool,
        /// Maximum time to run the test. Accepts a number with a unit suffix:
        /// s (seconds), m (minutes), h (hours), or d (days). Examples: 30s, 5m, 2h, 1d.
        #[arg(long, value_parser = duration::parse_duration)]
        time_limit: Option<Duration>,
        /// Don't launch the app; print the address to connect to and
        /// wait for an externally launched app (e.g. from Xcode).
        #[arg(long)]
        attach: bool,
        /// How long to wait (in seconds) for the app's agent to connect.
        #[arg(long, default_value_t = defaults::CONNECT_TIMEOUT_SECS)]
        connect_timeout_secs: u64,
        /// How long (in milliseconds) the UI must stay unchanged before the
        /// agent samples the next state. Lower values increase throughput but
        /// risk sampling mid-animation.
        #[arg(long, default_value_t = defaults::QUIESCENCE_TIMEOUT_MS)]
        quiescence_timeout_ms: u64,
        /// How long to wait (in seconds) for the agent to answer a
        /// state request or action.
        #[arg(long, default_value_t = defaults::STATE_TIMEOUT_SECS)]
        state_timeout_secs: u64,
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
        /// The command that launches the app under test. Everything after
        /// `--` is forwarded as program + arguments. Not needed with --attach.
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
            attach,
            connect_timeout_secs,
            quiescence_timeout_ms,
            state_timeout_secs,
            output_path,
            output_path_overwrite,
            reproduce,
            command,
        } => {
            let run_test = || -> Result<()> {
                let target = if attach {
                    if !command.is_empty() {
                        bail!(
                            "--attach and a launch command are mutually exclusive"
                        );
                    }
                    SwiftUITarget::Attach
                } else {
                    match &command[..] {
                        [program, args @ ..] => SwiftUITarget::Spawn {
                            program: program.clone(),
                            arguments: args.to_vec(),
                        },
                        _ => bail!(
                            "expected `<program> [args...]` after `--`, or --attach"
                        ),
                    }
                };

                let specification = match specification_file {
                    Some(path) => Specification::from_file(&path),
                    None => {
                        log::info!("using default specification");
                        Specification {
                            module_specifier:
                                "@antithesishq/bombadil/swiftui/defaults"
                                    .to_string(),
                        }
                    }
                };

                let output_path = output_path::resolve_output_path(
                    &output_path,
                    "bombadil_swiftui_",
                )?;
                let writer = TraceWriter::initialize(
                    output_path.clone(),
                    output_path_overwrite,
                )?;

                let mode = match reproduce {
                    Some(path) => SwiftUITestMode::Reproduce(
                        reproduce::load_reproduce_actions::<
                            bombadil_schema::swiftui::SwiftUIAction,
                            _,
                            SwiftUIStateSummary,
                        >(&path)?,
                    ),
                    None => SwiftUITestMode::RandomWalk,
                };

                let (driver, verifier) = SwiftUIDriver::launch(
                    specification,
                    target,
                    Duration::from_secs(connect_timeout_secs),
                    Duration::from_millis(quiescence_timeout_ms),
                    Duration::from_secs(state_timeout_secs),
                )?;

                let test_start = SystemTime::now();
                let deadline = time_limit.map(|d| test_start + d);

                let runner = Runner::new(driver, verifier);
                let mut strategy = SwiftUIStrategy {
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
                    bombadil_swiftui::ExitReason::ExitOnViolation => {
                        println!("Exited due to violation")
                    }
                    bombadil_swiftui::ExitReason::TimeLimit => {
                        println!("Exited after time limit hit")
                    }
                    bombadil_swiftui::ExitReason::Interrupted => {
                        println!("Exited after SIGINT")
                    }
                    bombadil_swiftui::ExitReason::Terminated(
                        ProcessExitStatus { code, signal: None },
                    ) => println!(
                        "Exited as app terminated with exit code {code}"
                    ),
                    bombadil_swiftui::ExitReason::Terminated(
                        ProcessExitStatus {
                            code,
                            signal: Some(signal),
                        },
                    ) => println!(
                        "Exited as app terminated with exit code {code} after signal {signal}"
                    ),
                    bombadil_swiftui::ExitReason::Reproduced => {
                        println!("Exited after reproduction finished")
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
                eprintln!("\n\nswiftui test failed: {error}");
                exit(1);
            }
        }
    }
}
