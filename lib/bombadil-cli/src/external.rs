use std::collections::VecDeque;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use antithesis_sdk::random::AntithesisRng;
use anyhow::{Result, anyhow, bail};
use bombadil::driver::{InterfaceDriver, RunId};
use bombadil::specification::bundler::bundle;
use bombadil::specification::verifier::Verifier;
use bombadil::{fuzzer, runner};
use bombadil_rpc::action::ExternalAction;
use bombadil_rpc::driver::ExternalDriver;
use bombadil_rpc::trace::{
    ExternalOutputWriter, ExternalTraceWriter, load_trace_actions,
};
use bombadil_rpc::{ExitReason, ExternalStrategy, ExternalTestMode};

use crate::duration;
use crate::output_path::resolve_output_path;
use crate::shrink::{self, ShrinkOptions};

const EXAMPLES: &str = "\
Examples:
  # Test an app through its driver, which Bombadil starts as a child process
  bombadil external test --specification app.spec.ts -- ./my-driver --device emulator-5554

  # Fuzz with four drivers in parallel for ten minutes
  bombadil external fuzz --specification app.spec.ts --parallelism 4 --time-limit-fuzz 10m -- ./my-driver
";

/// The external driver and the specification to test it against.
#[derive(clap::Args)]
pub struct DriverArgs {
    /// Path to a TypeScript specification file (uses the
    /// `@antithesishq/bombadil/external` API).
    #[arg(long = "specification")]
    specification_file: PathBuf,
    /// The driver program to run. Everything after `--` is forwarded as
    /// program + arguments. The driver speaks the external driver
    /// protocol on its stdin and stdout.
    #[clap(trailing_var_arg = true, required = true)]
    command: Vec<String>,
}

impl DriverArgs {
    fn driver(self) -> Result<(ExternalDriver, Arc<str>)> {
        let Some((program, arguments)) = self.command.split_first() else {
            bail!("expected `<program> [args...]` after `--`");
        };
        // Prepend "./" to relative paths so the bundler treats them as
        // paths rather than bare specifiers.
        let path = &self.specification_file;
        let path = if path.is_relative() && !path.starts_with(".") {
            PathBuf::from(".").join(path)
        } else {
            path.clone()
        };
        let specification_bundle: Arc<str> =
            bundle(".", &path.display().to_string())
                .map_err(|e| anyhow!("bundle failed: {e}"))?
                .into();
        let driver = ExternalDriver {
            specification_bundle: specification_bundle.clone(),
            program: program.clone(),
            arguments: arguments.to_vec(),
        };
        Ok((driver, specification_bundle))
    }
}

#[derive(clap::Subcommand)]
pub enum Command {
    /// (EXPERIMENTAL) Test an app through an external driver
    #[command(after_help = EXAMPLES)]
    Test {
        /// Whether to exit the test when first failing property is found (useful in development and CI)
        #[arg(long)]
        exit_on_violation: bool,
        /// Maximum time to run the test. Accepts a number with a unit suffix:
        /// s (seconds), m (minutes), h (hours), or d (days).
        #[arg(long, value_parser = duration::parse_duration)]
        time_limit: Option<Duration>,
        /// Trace FILE to write (any filename). Defaults to a unique file
        /// in system temp. Parent directories must already exist.
        #[arg(long)]
        output_path: Option<PathBuf>,
        /// Overwrite any existing trace at `--output-path`.
        #[arg(long)]
        output_path_overwrite: bool,
        /// Reproduce a previous test run from a trace FILE (not a directory).
        #[arg(long, value_name = "TRACE_FILE")]
        reproduce: Option<PathBuf>,
        #[clap(flatten)]
        driver: DriverArgs,
    },
    /// (EXPERIMENTAL) Fuzz (running many short test runs) an app through an
    /// external driver
    Fuzz {
        /// Maximum time to run an individual test run. Accepts a number with a unit suffix:
        /// s (seconds), m (minutes), h (hours), or d (days).
        #[arg(long, value_parser = duration::parse_duration, default_value = "10s")]
        time_limit_run: Duration,
        /// Maximum time to run the full fuzzing campaign. Accepts a number with a unit suffix:
        /// s (seconds), m (minutes), h (hours), or d (days).
        #[arg(long, value_parser = duration::parse_duration, default_value = "5m")]
        time_limit_fuzz: Duration,
        /// Whether to apply swarm testing to actions. Otherwise all actions are enabled.
        #[arg(long)]
        swarm: bool,
        /// Number of test runs to execute in parallel, each with its own
        /// driver process.
        #[arg(long, default_value = "1")]
        parallelism: NonZeroUsize,
        /// Trace file base: run.bin writes sibling files run-0.bin,
        /// run-1.bin, etc. Defaults to a unique base in system temp.
        #[arg(long)]
        output_path: Option<PathBuf>,
        /// Overwrite any existing traces at `--output-path`.
        #[arg(long)]
        output_path_overwrite: bool,
        #[clap(flatten)]
        driver: DriverArgs,
    },
    /// (EXPERIMENTAL) Shrink a trace with a property violation into a
    /// smaller one that still violates it
    Shrink {
        #[clap(flatten)]
        shrink_options: ShrinkOptions,
        #[clap(flatten)]
        driver: DriverArgs,
    },
}

pub fn run(command: Command) -> Result<()> {
    match command {
        Command::Test {
            exit_on_violation,
            time_limit,
            output_path,
            output_path_overwrite,
            reproduce,
            driver,
        } => {
            let (driver, specification_bundle) = driver.driver()?;
            let output_path = resolve_output_path(&output_path)?;
            let mode = match reproduce {
                Some(path) => {
                    ExternalTestMode::Reproduce(load_reproduce_actions(&path)?)
                }
                None => ExternalTestMode::RandomWalk,
            };
            let mut strategy = ExternalStrategy {
                rng: AntithesisRng,
                mode,
                test_start: None,
                violations_count: 0,
                exit_on_violation,
                deadline: time_limit.map(|limit| SystemTime::now() + limit),
                quiet: false,
            };
            let verifier = Verifier::new(&specification_bundle)?;
            let mut session = driver.new_session(RunId::default())?;
            let mut trace_writer = ExternalTraceWriter::initialize(
                output_path.clone(),
                output_path_overwrite,
            )?;
            let exit_reason = runner::run(
                &mut session,
                &mut strategy,
                verifier,
                &mut trace_writer,
                interrupt_handler()?,
            )?;
            println!(
                "\n{}",
                match exit_reason {
                    ExitReason::ExitOnViolation => "Exited due to violation",
                    ExitReason::TimeLimit => "Exited after time limit hit",
                    ExitReason::Stalled => "Exited after the driver stalled",
                    ExitReason::Interrupted => "Exited after SIGINT",
                    ExitReason::Reproduced => {
                        "Exited after reproduction finished"
                    }
                }
            );
            println!("Trace written to: {}", output_path.display());
            if strategy.violations_count > 0 {
                std::process::exit(2);
            }
            Ok(())
        }
        Command::Fuzz {
            time_limit_run,
            time_limit_fuzz,
            swarm,
            parallelism,
            output_path,
            output_path_overwrite,
            driver,
        } => {
            let (driver, specification_bundle) = driver.driver()?;
            let output_path = resolve_output_path(&output_path)?;
            println!("Using trace file base {}", output_path.display());
            fuzzer::fuzz(fuzzer::FuzzOptions {
                rng: AntithesisRng,
                driver: Arc::new(driver),
                specification_bundle,
                strategy: quiet_strategy,
                output_writer: ExternalOutputWriter {
                    root_path: output_path,
                    overwrite: output_path_overwrite,
                },
                interrupted: interrupt_handler()?,
                time_limit_fuzz,
                time_limit_run,
                swarm,
                parallelism,
            })
        }
        Command::Shrink {
            shrink_options,
            driver,
        } => {
            let (driver, specification_bundle) = driver.driver()?;
            let options = shrink_options.resolve(load_trace_actions)?;
            let result =
                bombadil::shrink::shrink(bombadil::shrink::ShrinkOptions {
                    driver: Arc::new(driver),
                    specification_bundle,
                    strategy: quiet_strategy,
                    trace_writer: |path| {
                        ExternalTraceWriter::initialize(path, false)
                    },
                    trace: options.trace,
                    property_name: options.property_name,
                    output_path: options.output_path.clone(),
                    output_path_overwrite: options.output_path_overwrite,
                    parallelism: options.parallelism,
                    interrupted: options.interrupted,
                    time_limit: options.time_limit,
                    settings: options.settings,
                })?;
            shrink::print_result(&result);
            println!(
                "Shrunk trace written to: {}\n",
                options.output_path.display()
            );
            Ok(())
        }
    }
}

/// The strategy of fuzz and shrink runs, whose actions are chosen for them.
fn quiet_strategy() -> ExternalStrategy<AntithesisRng> {
    ExternalStrategy {
        rng: AntithesisRng,
        mode: ExternalTestMode::RandomWalk,
        test_start: None,
        violations_count: 0,
        exit_on_violation: false,
        deadline: None,
        quiet: true,
    }
}

fn interrupt_handler() -> Result<Arc<AtomicBool>> {
    let interrupted = Arc::new(AtomicBool::new(false));
    let handler_interrupted = interrupted.clone();
    ctrlc::set_handler(move || {
        handler_interrupted.store(true, Ordering::SeqCst);
    })?;
    Ok(interrupted)
}

fn load_reproduce_actions(path: &Path) -> Result<VecDeque<ExternalAction>> {
    Ok(load_trace_actions(path)?.actions.into())
}
