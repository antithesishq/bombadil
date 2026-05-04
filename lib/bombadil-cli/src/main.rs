mod duration;
mod inspect_server;
mod render;

use ::url::Url;
use anyhow::Result;
use clap::{Args, Parser};
use std::{
    path::PathBuf,
    str::FromStr,
    time::{Duration, SystemTime},
};
use tempfile::TempDir;

use bombadil::{
    browser::{
        BrowserOptions, DebuggerOptions, Emulation, LaunchOptions,
        actions::BrowserAction, state::BrowserState,
    },
    instrumentation::InstrumentationConfig,
    runner::{ControlFlow, RunObserver, Runner},
    specification::verifier::{Snapshot, Specification},
    styled,
    trace::{PropertyViolation, writer::TraceWriter},
};
use bombadil_schema::markup;

/// Property-based testing for web UIs
#[derive(Parser)]
#[command(name = "bombadil", version, about, long_about=None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Args)]
struct TestSharedOptions {
    /// Starting URL of the test (also used as a boundary so that Bombadil doesn't navigate to
    /// other websites)
    origin: Origin,
    /// A custom specification in TypeScript or JavaScript, using the `@antithesishq/bombadil`
    /// package on NPM
    specification_file: Option<PathBuf>,
    /// Where to store output data (trace, screenshots, etc.)
    #[arg(long)]
    output_path: Option<PathBuf>,
    /// Whether to exit the test when first failing property is found (useful in development and CI)
    #[arg(long)]
    exit_on_violation: bool,
    /// Browser viewport width in pixels
    #[arg(long, default_value_t = 1024)]
    width: u16,
    /// Browser viewport height in pixels
    #[arg(long, default_value_t = 768)]
    height: u16,
    /// Scaling factor of the browser viewport, mostly useful on high-DPI monitors when in headed
    /// mode
    #[arg(long, default_value_t = 2.0)]
    device_scale_factor: f64,
    /// What types of JavaScript to instrument for coverage tracking.
    /// Comma-separated list of: "files", "inline"
    #[arg(long, default_value = "files,inline", value_parser = parse_instrumentation_config)]
    instrument_javascript: InstrumentationConfig,
    /// Maximum time to run the test. Accepts a number with a unit suffix:
    /// s (seconds), m (minutes), h (hours), or d (days). Examples: 30s, 5m, 2h, 1d.
    #[arg(long, value_parser = duration::parse_duration)]
    time_limit: Option<Duration>,
    /// Comma-separated list of Chrome permissions to grant.
    /// Examples: local-network-access, geolocation, notifications.
    #[arg(
        long,
        default_value = "local-network-access,local-network,loopback-network"
    )]
    chrome_grant_permissions: String,
    /// Extra HTTP header to send with all browser requests, in KEY=VALUE format.
    /// Can be specified multiple times.
    #[arg(long = "header", value_name = "KEY=VALUE", value_parser = parse_header)]
    headers: Vec<(String, String)>,
}

#[derive(clap::Subcommand)]
enum Command {
    /// Run a test with a browser managed by Bombadil
    Test {
        #[clap(flatten)]
        shared: TestSharedOptions,
        /// Whether the browser should run in a visible window or not
        #[arg(long, default_value_t = false)]
        headless: bool,
        /// Disable Chromium sandboxing
        #[arg(long, default_value_t = false)]
        no_sandbox: bool,
    },
    /// Run a test with an externally managed browser or Electron app (e.g. `chromium
    /// --remote-debugging-port=9992`)
    TestExternal {
        #[clap(flatten)]
        shared: TestSharedOptions,
        /// Address to the remote debugger's server, e.g. http://localhost:9222
        #[arg(long)]
        remote_debugger: Url,
        /// Whether Bombadil should create a new tab and navigate to the origin URL in it, as part
        /// of starting the test (this should probably be false if you test an Electron app)
        #[arg(long)]
        create_target: bool,
    },
    /// Launch Bombadil Inspect to inspect a trace file
    Inspect {
        /// Path to trace.jsonl file or directory containing it
        trace_path: PathBuf,
        /// Port to bind the inspect server to
        #[arg(long, default_value_t = 1073)]
        port: u16,
        /// Skip auto-opening browser
        #[arg(long, default_value_t = false)]
        no_open: bool,
    },
}

#[derive(Clone)]
struct Origin {
    url: Url,
}

impl FromStr for Origin {
    type Err = url::ParseError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Url::parse(s)
            .or(Url::parse(&format!(
                "file://{}",
                std::path::absolute(s)
                    .expect("invalid path")
                    .to_str()
                    .expect("invalid path")
            )))
            .map(|url| Origin { url })
    }
}

fn parse_header(s: &str) -> std::result::Result<(String, String), String> {
    s.split_once('=')
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .ok_or_else(|| format!("invalid header {:?}, expected KEY=VALUE", s))
}

fn parse_instrumentation_config(
    s: &str,
) -> std::result::Result<InstrumentationConfig, String> {
    if s.is_empty() {
        return Ok(InstrumentationConfig::none());
    }

    let mut instrument_files = false;
    let mut instrument_inline = false;

    for part in s.split(',') {
        let part = part.trim();
        match part {
            "files" => instrument_files = true,
            "inline" => instrument_inline = true,
            "" => {}
            unknown => {
                return Err(format!(
                    "unknown instrumentation target '{}', valid options are: files, inline",
                    unknown
                ));
            }
        }
    }

    Ok(InstrumentationConfig {
        instrument_files,
        instrument_inline,
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    let env = env_logger::Env::default().default_filter_or("warn");
    env_logger::Builder::from_env(env)
        .format_timestamp_millis()
        .format_target(true)
        // Until we hav a fix for https://github.com/mattsse/chromiumoxide/issues/287
        .filter_module("chromiumoxide::browser", log::LevelFilter::Error)
        .filter_module("html5ever", log::LevelFilter::Info)
        .init();
    let cli = Cli::parse();
    match cli.command {
        Command::Test {
            shared,
            headless,
            no_sandbox,
        } => {
            let user_data_directory = TempDir::with_prefix("user_data_")?;
            let output_path = resolve_output_path(&shared)?;

            let browser_options = BrowserOptions {
                create_target: true,
                emulation: Emulation {
                    width: shared.width,
                    height: shared.height,
                    device_scale_factor: shared.device_scale_factor,
                },
                instrumentation: shared.instrument_javascript.clone(),
                downloads_directory: output_path.join("downloads"),
                grant_permissions: shared
                    .chrome_grant_permissions
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect(),
                extra_headers: shared.headers.iter().cloned().collect(),
            };
            let debugger_options = DebuggerOptions::Managed {
                launch_options: LaunchOptions {
                    headless,
                    user_data_directory: user_data_directory
                        .path()
                        .to_path_buf(),
                    no_sandbox,
                },
            };
            test(output_path, shared, browser_options, debugger_options).await
        }
        Command::TestExternal {
            shared,
            remote_debugger,
            create_target,
        } => {
            let output_path = resolve_output_path(&shared)?;
            let browser_options = BrowserOptions {
                create_target,
                emulation: Emulation {
                    width: shared.width,
                    height: shared.height,
                    device_scale_factor: shared.device_scale_factor,
                },
                instrumentation: shared.instrument_javascript.clone(),
                downloads_directory: output_path.join("downloads"),
                grant_permissions: shared
                    .chrome_grant_permissions
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect(),
                extra_headers: shared.headers.iter().cloned().collect(),
            };
            let debugger_options =
                DebuggerOptions::External { remote_debugger };
            test(output_path, shared, browser_options, debugger_options).await
        }
        Command::Inspect {
            trace_path,
            port,
            no_open,
        } => inspect_server::serve(trace_path, port, !no_open).await,
    }
}

fn resolve_output_path(shared_options: &TestSharedOptions) -> Result<PathBuf> {
    match &shared_options.output_path {
        Some(path) => Ok(path.clone()),
        None => Ok(TempDir::with_prefix("bombadil_")?.keep().to_path_buf()),
    }
}

async fn test(
    output_path: PathBuf,
    shared_options: TestSharedOptions,
    browser_options: BrowserOptions,
    debugger_options: DebuggerOptions,
) -> Result<()> {
    // Load a user-provided specification, or use the defaults provided by Bombadil.
    let specification = if let Some(path) = &shared_options.specification_file {
        let path = if path.is_relative() && !path.starts_with(".") {
            PathBuf::from(".").join(path)
        } else {
            path.clone()
        };
        log::info!("loading specification from file: {}", path.display());
        Specification {
            module_specifier: path.display().to_string(),
        }
    } else {
        log::info!("using default specification");
        Specification {
            module_specifier: "@antithesishq/bombadil/defaults".to_string(),
        }
    };

    let runner = Runner::new(
        shared_options.origin.url,
        specification,
        browser_options,
        debugger_options,
    )
    .await?;

    struct MainObserver {
        writer: TraceWriter,
        exit_on_violation: bool,
        test_start: Option<bombadil_schema::Time>,
        deadline: Option<SystemTime>,
        output_path: PathBuf,
        violations_count: u64,
    }

    #[derive(Clone, Copy, Debug)]
    enum ExitReason {
        ExitOnViolation,
        TimeLimit,
        Interrupted,
    }

    #[derive(Clone, Copy, Debug)]
    struct TestResult {
        exit_reason: ExitReason,
        violations_count: u64,
    }

    impl RunObserver for MainObserver {
        type StopValue = TestResult;

        async fn on_new_state(
            &mut self,
            state: &BrowserState,
            last_action: Option<&BrowserAction>,
            snapshots: &[Snapshot],
            violations: &[PropertyViolation],
        ) -> anyhow::Result<ControlFlow<Self::StopValue>> {
            let test_start = *self.test_start.get_or_insert(
                bombadil_schema::Time::from_system_time(state.timestamp),
            );

            if let Some(action) = last_action {
                println!(
                    "{} {}",
                    render::format_timestamp(state.timestamp, test_start),
                    render::format_action(action)
                );
            }

            self.violations_count += violations.len() as u64;
            for violation in violations {
                log::info!("violation of property `{}`", violation.name);
                let api_violation = violation.to_api();
                let markup = markup::render_violation(&api_violation);
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

            self.writer
                .write(state, last_action, snapshots, violations)
                .await?;

            if self.violations_count > 0 && self.exit_on_violation {
                return Ok(ControlFlow::Stop(TestResult {
                    exit_reason: ExitReason::ExitOnViolation,
                    violations_count: self.violations_count,
                }));
            }

            if let Some(deadline) = self.deadline
                && state.timestamp >= deadline
            {
                log::info!("time limit reached, stopping");
                return Ok(ControlFlow::Stop(TestResult {
                    exit_reason: ExitReason::TimeLimit,
                    violations_count: self.violations_count,
                }));
            }

            Ok(ControlFlow::Continue)
        }

        async fn on_interrupted(&mut self) -> anyhow::Result<Self::StopValue> {
            Ok(TestResult {
                exit_reason: ExitReason::Interrupted,
                violations_count: self.violations_count,
            })
        }
    }

    if let Some(duration) = shared_options.time_limit {
        log::info!(
            "test time limit set to {}",
            duration::format_duration(duration)
        );
    }

    let deadline = shared_options.time_limit.map(|d| SystemTime::now() + d);

    let mut observer = MainObserver {
        writer: TraceWriter::initialize(output_path.clone()).await?,
        exit_on_violation: shared_options.exit_on_violation,
        test_start: None,
        deadline,
        output_path: output_path.clone(),
        violations_count: 0,
    };

    let test_result = runner.run(&mut observer).await?;

    let heading = if let Some(TestResult {
        exit_reason,
        violations_count,
    }) = test_result
    {
        let findings = match violations_count {
            0 => "".into(),
            1 => ", finding 1 violation".into(),
            n => format!(", finding {n} violations"),
        };

        let heading = styled::maybe_bold(match exit_reason {
            ExitReason::ExitOnViolation => {
                format!("Test finished{findings}!",)
            }
            ExitReason::TimeLimit => {
                format!("Test finished after time limit{findings}!")
            }
            ExitReason::Interrupted => {
                format!("Test was interrupted by SIGINT{findings}!",)
            }
        });

        if violations_count > 0 {
            styled::maybe_red(heading)
        } else {
            heading
        }
    } else {
        styled::maybe_bold("Test finished!".to_string())
    };

    println!(
        "\n{heading}\n\nInspect the test results using:\n\n  {}",
        styled::maybe_italic(format!(
            "bombadil inspect {}",
            observer.output_path.display()
        ))
    );

    if let Some(result) = test_result
        && result.violations_count > 0
    {
        std::process::exit(2);
    }

    Ok(())
}
