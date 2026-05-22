use ::url::Url;
use anyhow::Result;
use bombadil_browser::{
    browser::LaunchOptions, convert::ToInternal, trace::writer::FileTraceWriter,
};
use clap::Args;
use serde_json as json;
use std::{
    path::{Path, PathBuf},
    str::FromStr,
    time::{Duration, SystemTime},
};
use tempfile::TempDir;
use tokio::io::AsyncBufReadExt;
use tokio::{fs::File, io::BufReader};

use bombadil::{specification::verifier::Specification, styled};
use bombadil_browser::{
    browser::{
        BrowserOptions, DebuggerOptions, Emulation, actions::BrowserAction,
    },
    instrumentation::InstrumentationConfig,
};
use bombadil_schema::schema;

use bombadil_browser::strategy::{
    ExitReason, TestMode, TestResult, TestStrategy,
};

use crate::{duration, inspect_server, output_path};

const DEFAULT_WIDTH: u16 = 1024;
const DEFAULT_HEIGHT: u16 = 768;
const DEFAULT_DEVICE_SCALE_FACTOR: f64 = 1.0;

#[derive(clap::Subcommand)]
pub enum BrowserCommand {
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

#[derive(Args)]
pub struct TestSharedOptions {
    /// Starting URL of the test (also used as a boundary so that Bombadil doesn't navigate to
    /// other websites)
    pub origin: Origin,
    /// A custom specification in TypeScript or JavaScript, using the `@antithesishq/bombadil`
    /// package on NPM
    pub specification_file: Option<PathBuf>,
    /// Where to store output data (trace, screenshots, etc.)
    #[arg(long)]
    pub output_path: Option<PathBuf>,
    /// Whether to exit the test when first failing property is found (useful in development and CI)
    #[arg(long)]
    pub exit_on_violation: bool,
    /// Browser viewport width in pixels
    #[arg(long, default_value_t = DEFAULT_WIDTH)]
    pub width: u16,
    /// Browser viewport height in pixels
    #[arg(long, default_value_t = DEFAULT_HEIGHT)]
    pub height: u16,
    /// Scaling factor of the browser viewport, mostly useful on high-DPI monitors when in headed
    /// mode
    #[arg(long, default_value_t = DEFAULT_DEVICE_SCALE_FACTOR)]
    pub device_scale_factor: f64,
    /// What types of JavaScript to instrument for coverage tracking.
    /// Comma-separated list of: "files", "inline"
    #[arg(long, default_value = "files,inline", value_parser = parse_instrumentation_config)]
    pub instrument_javascript: InstrumentationConfig,
    /// Maximum time to run the test. Accepts a number with a unit suffix:
    /// s (seconds), m (minutes), h (hours), or d (days). Examples: 30s, 5m, 2h, 1d.
    #[arg(long, value_parser = duration::parse_duration)]
    pub time_limit: Option<Duration>,
    /// Comma-separated list of Chrome permissions to grant.
    /// Examples: local-network-access, geolocation, notifications.
    #[arg(
        long,
        default_value = "local-network-access,local-network,loopback-network"
    )]
    pub chrome_grant_permissions: String,
    /// Extra HTTP header to send with all browser requests, in KEY=VALUE format.
    /// Can be specified multiple times.
    #[arg(long = "header", value_name = "KEY=VALUE", value_parser = parse_header)]
    pub headers: Vec<(String, String)>,
    /// Reproduce a previous test run from a trace file, instead of random exploration.
    /// Mutually exclusive with --time-limit and --exit-on-violation.
    #[arg(long, value_name = "TRACE_FILE", conflicts_with_all = ["time_limit", "exit_on_violation"])]
    pub reproduce: Option<PathBuf>,
}

#[derive(Clone)]
pub struct Origin {
    pub url: Url,
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

pub async fn run(command: BrowserCommand) -> Result<()> {
    match command {
        BrowserCommand::Test {
            shared,
            headless,
            no_sandbox,
        } => {
            let mode = resolve_test_mode(&shared).await?;
            let user_data_directory = TempDir::with_prefix("user_data_")?;
            let output_path =
                output_path::resolve_output_path(&shared.output_path)?;

            let mut reproduce_args =
                reproduce_command_args("browser test", &shared);
            if headless {
                reproduce_args.push("--headless".into());
            }
            if no_sandbox {
                reproduce_args.push("--no-sandbox".into());
            }

            let browser_options =
                browser_options_from_shared(&shared, &output_path);
            let debugger_options = DebuggerOptions::Managed {
                launch_options: LaunchOptions {
                    headless,
                    user_data_directory: user_data_directory
                        .path()
                        .to_path_buf(),
                    no_sandbox,
                },
            };
            browser_test(
                mode,
                reproduce_args,
                output_path,
                shared,
                browser_options,
                debugger_options,
            )
            .await
        }
        BrowserCommand::TestExternal {
            shared,
            remote_debugger,
            create_target,
        } => {
            let mode = resolve_test_mode(&shared).await?;
            let output_path =
                output_path::resolve_output_path(&shared.output_path)?;

            let mut reproduce_args =
                reproduce_command_args("browser test-external", &shared);
            reproduce_args.push(format!("--remote-debugger {remote_debugger}"));
            if create_target {
                reproduce_args.push("--create-target".into());
            }

            let browser_options = BrowserOptions {
                create_target,
                ..browser_options_from_shared(&shared, &output_path)
            };
            let debugger_options =
                DebuggerOptions::External { remote_debugger };
            browser_test(
                mode,
                reproduce_args,
                output_path,
                shared,
                browser_options,
                debugger_options,
            )
            .await
        }
        BrowserCommand::Inspect {
            trace_path,
            port,
            no_open,
        } => inspect_server::serve(trace_path, port, !no_open).await,
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

fn browser_options_from_shared(
    shared: &TestSharedOptions,
    output_path: &Path,
) -> BrowserOptions {
    BrowserOptions {
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
    }
}

fn reproduce_command_args(
    subcommand: &str,
    shared: &TestSharedOptions,
) -> Vec<String> {
    let mut args = vec![subcommand.to_string(), shared.origin.url.to_string()];
    if let Some(path) = &shared.specification_file {
        args.push(path.display().to_string());
    }
    if shared.width != DEFAULT_WIDTH {
        args.push(format!("--width {}", shared.width));
    }
    if shared.height != DEFAULT_HEIGHT {
        args.push(format!("--height {}", shared.height));
    }
    if (shared.device_scale_factor - DEFAULT_DEVICE_SCALE_FACTOR).abs()
        > f64::EPSILON
    {
        args.push(format!(
            "--device-scale-factor {}",
            shared.device_scale_factor
        ));
    }
    for (key, value) in &shared.headers {
        args.push(format!("--header {key}={value}"));
    }
    args
}

async fn resolve_test_mode(
    shared_options: &TestSharedOptions,
) -> Result<TestMode> {
    match &shared_options.reproduce {
        None => Ok(TestMode::RandomWalk),
        Some(path) => {
            let trace_file_path =
                output_path::resolve_trace_directory(path).join("trace.jsonl");
            let trace_file = File::open(&trace_file_path).await?;
            let mut lines = BufReader::new(trace_file).lines();
            let mut actions: Vec<BrowserAction> = vec![];
            while let Some(line) = lines.next_line().await? {
                let entry: schema::BrowserTraceEntry = json::from_str(&line)?;
                if let Some(action) = entry.action {
                    actions.push(action.to_internal());
                }
            }
            Ok(TestMode::Reproduce(actions.into()))
        }
    }
}

async fn browser_test(
    mode: TestMode,
    reproduce_args: Vec<String>,
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
            module_specifier: "@antithesishq/bombadil/browser/defaults"
                .to_string(),
        }
    };

    let is_reproduce = shared_options.reproduce.is_some();

    let runner = bombadil_browser::runner::launch(
        shared_options.origin.url.clone(),
        specification,
        browser_options,
        debugger_options,
    )
    .await?;

    if let Some(duration) = shared_options.time_limit {
        log::info!(
            "test time limit set to {}",
            duration::format_duration(duration)
        );
    }

    let deadline = shared_options.time_limit.map(|d| SystemTime::now() + d);

    let mut strategy = TestStrategy {
        mode,
        writer: FileTraceWriter::initialize(output_path.clone()).await?,
        exit_on_violation: shared_options.exit_on_violation,
        test_start: None,
        deadline,
        output_path: output_path.clone(),
        violations_count: 0,
        origin: shared_options.origin.url,
    };

    let test_result = runner.run(&mut strategy).await?;

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
            ExitReason::Reproduced => {
                format!("Reproduction finished{findings}!",)
            }
            ExitReason::AllDefinite => {
                format!("Test finished with all properties definite{findings}!")
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

    let output_display = strategy.output_path.display();
    let inspect_command = styled::maybe_italic(format!(
        "bombadil browser inspect {output_display}"
    ));
    println!(
        "\n{heading}\n\nInspect the test results using:\
         \n\n  {inspect_command}\n",
    );
    if !is_reproduce {
        let reproduce_command = styled::maybe_italic(format!(
            "bombadil {} --reproduce {output_display}",
            reproduce_args.join(" "),
        ));
        println!(
            "Reproduce this test using:\
             \n\n  {reproduce_command}\n",
        );
    }

    if let Some(result) = test_result
        && result.violations_count > 0
    {
        std::process::exit(2);
    }

    Ok(())
}
