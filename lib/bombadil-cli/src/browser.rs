use ::url::Url;
use antithesis_sdk::random::AntithesisRng;
use anyhow::{Context, Result, anyhow, bail};
use bombadil_browser::{
    browser_options::{BrowserOptions, Emulation, VirtualTimePolicy},
    chromium::{self, LaunchOptions},
    convert::ToInternal,
    cookie::BrowserCookie,
    driver::{BrowserDriver, BrowserSession, DebuggerOptions},
    match_pattern::MatchPattern,
    trace::writer::{FileOutputWriter, FileTraceWriter},
};
use clap::Args;
use serde_json as json;
use std::{
    fs::File,
    io::{BufRead, BufReader},
    num::NonZeroUsize,
    path::{Path, PathBuf},
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime},
};
use tempfile::TempDir;

use bombadil::{
    antithesis,
    driver::{RunId, TraceWriter},
    fuzzer,
    specification::{bundler::bundle, verifier::Specification},
    styled,
};
use bombadil_browser::{
    browser::actions::BrowserAction, instrumentation::InstrumentationConfig,
};
use bombadil_schema::browser;

use bombadil_browser::strategy::{
    ExitReason, TestMode, TestResult, TestStrategy,
};

use crate::shrink::{self, ShrinkOptions};
use crate::{duration, inspect_server, output_path};

const DEFAULT_WIDTH: u16 = 1024;
const DEFAULT_HEIGHT: u16 = 768;
const DEFAULT_DEVICE_SCALE_FACTOR: f64 = 1.0;

const TEST_EXAMPLES: &str = "\
Examples:
  # Test a website using the default specification
  bombadil browser test https://example.com

  # Use a custom specification and stop at the first violation
  bombadil browser test --exit-on-violation https://example.com spec.ts

  # Run headless for five minutes
  bombadil browser test --headless --time-limit 5m https://example.com

  # Grant specific Chrome permissions
  bombadil browser test --chrome-grant-permissions geolocation,notifications https://example.com

  # Speed up a timer-heavy app by automatically advancing virtual time in Chrome
  bombadil browser test --chrome-virtual-time-policy Advance https://example.com

  # Reproduce a test run (best-effort, fails on divergence)
  bombadil browser test --reproduce my-test https://example.com
";

const TEST_EXTERNAL_EXAMPLES: &str = "\
Examples:
  # Start Chromium (or an Electron app) with remote debugging enabled, and launch a test against the existing open target
  chromium --remote-debugging-port=9222 https://example.com
  bombadil browser test-external --remote-debugger http://localhost:9222 https://example.com
";

const SHRINK_EXAMPLES: &str = "\
Examples:
  # Shrink the trace in `my-test`, which violates a single property
  bombadil browser shrink --headless --trace my-test https://example.com

  # Shrink for a specific property, using four parallel browsers
  bombadil browser shrink --headless --trace my-test --property noErrors --parallelism 4 https://example.com spec.ts
";

const INSPECT_EXAMPLES: &str = "\
Examples:
  # Inspect the trace in an output directory named `my-test`
  bombadil browser inspect my-test

  # Serve on another port, without trying to open a browser
  bombadil browser inspect --port 8080 --no-open my-test/trace.jsonl
";

#[derive(clap::Subcommand)]
pub enum BrowserCommand {
    /// Run a test with a browser managed by Bombadil
    #[command(after_help = TEST_EXAMPLES)]
    Test {
        #[clap(flatten)]
        shared: RunSharedOptions,
        #[clap(flatten)]
        launch_options: ManagedBrowserOptions,
    },
    /// Run a test with an externally managed browser or Electron app (e.g. `chromium
    /// --remote-debugging-port=9992`)
    #[command(after_help = TEST_EXTERNAL_EXAMPLES)]
    TestExternal {
        #[clap(flatten)]
        shared: RunSharedOptions,
        /// Address to the remote debugger's server, e.g. http://localhost:9222
        #[arg(long)]
        remote_debugger: Url,
        /// Whether Bombadil should create a new tab and navigate to the origin URL in it, as part
        /// of starting the test (this should probably be false if you test an Electron app)
        #[arg(long)]
        create_target: bool,
    },
    /// (EXPERIMENTAL) Fuzz (running many tests) with a browser managed by Bombadil
    Fuzz {
        #[clap(flatten)]
        run_shared_options: RunSharedOptions,
        #[clap(flatten)]
        fuzz_options: FuzzOptions,
        #[clap(flatten)]
        launch_options: ManagedBrowserOptions,
    },
    /// (EXPERIMENTAL) Shrink a trace with a property violation into a smaller
    /// one that still violates it (best-effort)
    #[command(after_help = SHRINK_EXAMPLES)]
    Shrink {
        #[clap(flatten)]
        target: TargetOptions,
        #[clap(flatten)]
        shrink_options: ShrinkOptions,
        #[clap(flatten)]
        launch_options: ManagedBrowserOptions,
    },
    /// Launch Bombadil Inspect to inspect a trace file
    #[command(after_help = INSPECT_EXAMPLES)]
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
pub struct RunSharedOptions {
    #[clap(flatten)]
    pub target: TargetOptions,

    /// Where to store output data (trace, screenshots, etc.)
    #[arg(long)]
    pub output_path: Option<PathBuf>,

    /// Overwrite any existing trace at `--output-path`. Without this flag,
    /// Bombadil refuses to write when trace.jsonl or trace.bin already
    /// exists.
    #[arg(long)]
    pub output_path_overwrite: bool,

    /// Whether to exit the test when first failing property is found (useful in development and CI)
    #[arg(long)]
    pub exit_on_violation: bool,

    /// Maximum time to run the test. Accepts a number with a unit suffix:
    /// s (seconds), m (minutes), h (hours), or d (days).
    #[arg(long, value_parser = duration::parse_duration)]
    pub time_limit: Option<Duration>,

    /// Reproduce a previous test run from a trace file, instead of random exploration.
    /// Mutually exclusive with `--time-limit` and `--exit-on-violation`.
    #[arg(long, value_name = "TRACE_FILE", conflicts_with_all = ["time_limit", "exit_on_violation"])]
    pub reproduce: Option<PathBuf>,
}

/// What to test and how to set up the browser.
#[derive(Args)]
pub struct TargetOptions {
    /// Starting URL of the test. Without `--allow-url`, exploration stays on
    /// the origin's scheme, host and port. For a `file://` origin it stays on
    /// that exact file. With one or more `--allow-url` entries, those define the
    /// exploration boundary instead (include the origin there if it should
    /// remain allowed).
    pub origin: Origin,

    /// A custom specification in TypeScript or JavaScript, using the `@antithesishq/bombadil`
    /// package on NPM
    pub specification_file: Option<PathBuf>,

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

    /// Comma-separated list of Chrome permissions to grant.
    #[arg(
        long,
        default_value = "local-network-access,local-network,loopback-network"
    )]
    pub chrome_grant_permissions: String,

    /// Optional virtual time policy for Chrome, used to speed up execution
    /// of timer-heavy applications. One of: Advance, Pause,
    /// PauseIfNetworkFetchesPending.
    #[arg(long)]
    pub chrome_virtual_time_policy: Option<VirtualTimePolicy>,

    /// Extra HTTP header to send with all browser requests, in KEY=VALUE format.
    /// Can be specified multiple times.
    #[arg(long = "header", value_name = "KEY=VALUE", value_parser = parse_header)]
    pub headers: Vec<(String, String)>,

    /// Cookie to set in the browser before testing. Accepts plain NAME=VALUE
    /// (scoped to the origin) or Set-Cookie syntax with attributes such as
    /// Domain, Path, Secure, and HttpOnly. Unlike `--header`, these become real
    /// browser cookies. Can be specified multiple times.
    #[arg(long = "cookie", value_name = "SET-COOKIE", value_parser = parse_cookie)]
    pub cookies: Vec<BrowserCookie>,

    /// Exploration boundary as Chrome extension match patterns, e.g.
    /// `*://*.example.com/*`. Replaces the implicit origin match pattern.
    /// For the pattern syntax, see
    /// [Match patterns](https://developer.chrome.com/docs/extensions/develop/concepts/match-patterns).
    #[arg(long = "allow-url", value_name = "MATCH_PATTERN", value_parser = MatchPattern::parse)]
    pub allow_urls: Vec<MatchPattern>,
}

/// How to launch a browser managed by Bombadil.
#[derive(Args)]
pub struct ManagedBrowserOptions {
    /// Whether the browser should run in a visible window or not
    #[arg(long, default_value_t = false)]
    headless: bool,
    /// Disable Chromium sandboxing
    #[arg(long, default_value_t = false)]
    no_sandbox: bool,
}

impl ManagedBrowserOptions {
    fn debugger_options(&self) -> Result<DebuggerOptions> {
        Ok(DebuggerOptions::Managed {
            launch_options: LaunchOptions {
                executable: chromium::locate::executable()?,
                headless: self.headless,
                no_sandbox: self.no_sandbox,
            },
        })
    }

    fn reproduce_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        if self.headless {
            args.push("--headless".into());
        }
        if self.no_sandbox {
            args.push("--no-sandbox".into());
        }
        args
    }
}

#[derive(Args)]
pub struct FuzzOptions {
    /// Whether to apply swarm testing to actions. Otherwise all actions are enabled.
    #[arg(long)]
    pub swarm: bool,
    /// Number of test runs to execute in parallel.
    #[arg(long, default_value = "1")]
    pub parallelism: NonZeroUsize,

    #[arg(long, value_parser = duration::parse_duration, default_value = "5m")]
    pub time_limit_fuzz: Duration,
    /// Maximum time to run an individual linear run. Accepts a number with a unit suffix:
    /// s (seconds), m (minutes), h (hours), or d (days).
    #[arg(long, value_parser = duration::parse_duration, default_value = "30s")]
    pub time_limit_run: Duration,
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

pub fn run(command: BrowserCommand) -> Result<()> {
    match command {
        BrowserCommand::Test {
            shared,
            launch_options,
        } => {
            let mode = resolve_test_mode(&shared)?;
            let user_data_directory = TempDir::with_prefix("user_data_")?;
            log::info!(
                "storing chromium/chrome user data in {}",
                user_data_directory.path().display()
            );

            let output_path =
                output_path::resolve_output_path(&shared.output_path)?;

            let mut reproduce_args =
                reproduce_command_args("browser test", &shared.target);
            reproduce_args.extend(launch_options.reproduce_args());

            let browser_options =
                browser_options_from_target(&shared.target, &output_path);
            let debugger_options = launch_options.debugger_options()?;
            browser_test(
                mode,
                reproduce_args,
                output_path,
                shared,
                browser_options,
                debugger_options,
            )
        }
        BrowserCommand::TestExternal {
            shared,
            remote_debugger,
            create_target,
        } => {
            let mode = resolve_test_mode(&shared)?;
            let output_path =
                output_path::resolve_output_path(&shared.output_path)?;

            let mut reproduce_args =
                reproduce_command_args("browser test-external", &shared.target);
            reproduce_args.push(format!("--remote-debugger {remote_debugger}"));
            if create_target {
                reproduce_args.push("--create-target".into());
            }

            let browser_options = BrowserOptions {
                create_target,
                ..browser_options_from_target(&shared.target, &output_path)
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
        }
        BrowserCommand::Fuzz {
            launch_options,
            run_shared_options,
            fuzz_options,
        } => {
            let output_path = output_path::resolve_output_path(
                &run_shared_options.output_path,
            )?;

            let browser_options = browser_options_from_target(
                &run_shared_options.target,
                &output_path,
            );
            let debugger_options = launch_options.debugger_options()?;
            browser_fuzz(
                output_path,
                run_shared_options,
                browser_options,
                debugger_options,
                fuzz_options,
            )
        }
        BrowserCommand::Shrink {
            target,
            shrink_options,
            launch_options,
        } => browser_shrink(target, shrink_options, launch_options),
        BrowserCommand::Inspect {
            trace_path,
            port,
            no_open,
        } => {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;

            runtime.block_on(inspect_server::serve(trace_path, port, !no_open))
        }
    }
}

fn parse_header(s: &str) -> std::result::Result<(String, String), String> {
    s.split_once('=')
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .ok_or_else(|| format!("invalid header {:?}, expected KEY=VALUE", s))
}

fn parse_cookie(s: &str) -> std::result::Result<BrowserCookie, String> {
    BrowserCookie::parse(s)
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

fn browser_options_from_target(
    target: &TargetOptions,
    output_path: &Path,
) -> BrowserOptions {
    BrowserOptions {
        create_target: true,
        emulation: Emulation {
            width: target.width,
            height: target.height,
            device_scale_factor: target.device_scale_factor,
        },
        instrumentation: target.instrument_javascript.clone(),
        downloads_directory: output_path.join("downloads"),
        grant_permissions: target
            .chrome_grant_permissions
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        virtual_time_policy: target.chrome_virtual_time_policy.clone(),
        extra_headers: target.headers.iter().cloned().collect(),
        cookies: target.cookies.clone(),
    }
}

/// Load a user-provided specification, or use the defaults provided by Bombadil.
fn load_specification(target: &TargetOptions) -> Specification {
    if let Some(path) = &target.specification_file {
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
    }
}

fn resolve_allow_urls(target: &TargetOptions) -> Result<Vec<MatchPattern>> {
    if target.allow_urls.is_empty() {
        Ok(vec![
            MatchPattern::from_origin(&target.origin.url)
                .map_err(|reason| anyhow!("invalid allow url: {reason}"))?,
        ])
    } else {
        Ok(target.allow_urls.clone())
    }
}

fn reproduce_command_args(
    subcommand: &str,
    target: &TargetOptions,
) -> Vec<String> {
    let mut args = vec![
        subcommand.to_string(),
        shell_quote(target.origin.url.as_str()),
    ];
    if let Some(path) = &target.specification_file {
        args.push(path.display().to_string());
    }
    if target.width != DEFAULT_WIDTH {
        args.push(format!("--width {}", target.width));
    }
    if target.height != DEFAULT_HEIGHT {
        args.push(format!("--height {}", target.height));
    }
    if (target.device_scale_factor - DEFAULT_DEVICE_SCALE_FACTOR).abs()
        > f64::EPSILON
    {
        args.push(format!(
            "--device-scale-factor {}",
            target.device_scale_factor
        ));
    }
    for (key, value) in &target.headers {
        args.push(format!("--header {key}={value}"));
    }
    for cookie in &target.cookies {
        args.push(format!("--cookie {cookie}"));
    }
    for allow_url in &target.allow_urls {
        args.push(format!("--allow-url '{allow_url}'"));
    }
    args
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn resolve_test_mode(shared_options: &RunSharedOptions) -> Result<TestMode> {
    match &shared_options.reproduce {
        None => Ok(TestMode::RandomWalk),
        Some(path) => {
            let trace_file_path =
                output_path::resolve_browser_trace_file(path)?;
            let trace_file = File::open(&trace_file_path)?;
            let mut actions: Vec<BrowserAction> = vec![];
            for line in BufReader::new(trace_file).lines() {
                let line = line.context("failed to read line from trace")?;
                let entry: browser::BrowserTraceEntry = json::from_str(&line)?;
                if let Some(action) = entry.action {
                    actions.push(action.to_internal());
                }
            }
            Ok(TestMode::Reproduce(actions.into()))
        }
    }
}

fn browser_test(
    mode: TestMode,
    reproduce_args: Vec<String>,
    output_path: PathBuf,
    shared_options: RunSharedOptions,
    browser_options: BrowserOptions,
    debugger_options: DebuggerOptions,
) -> Result<()> {
    let specification = load_specification(&shared_options.target);

    let is_reproduce = shared_options.reproduce.is_some();

    if let Some(duration) = shared_options.time_limit {
        log::info!(
            "test time limit set to {}",
            duration::format_duration(duration)
        );
    }

    let allow_urls = resolve_allow_urls(&shared_options.target)?;
    let origin = shared_options.target.origin.url;

    let run_options = RunOptions {
        run_id: RunId::default(),
        specification,
        browser_options,
        debugger_options,
        mode,
        deadline: shared_options.time_limit.map(|d| SystemTime::now() + d),
        allow_urls,
        origin,
        exit_on_violation: shared_options.exit_on_violation,
    };

    let test_result = run_with_writer(
        run_options,
        FileTraceWriter::initialize(
            output_path.clone(),
            shared_options.output_path_overwrite,
        )?,
    )?;

    let heading = {
        let TestResult {
            exit_reason,
            violations_count,
        } = test_result;

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
            ExitReason::Stalled => {
                format!("Test stopped after the browser stalled{findings}!")
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
    };

    let output_display = output_path.display();
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

    if test_result.violations_count > 0 {
        std::process::exit(2);
    }
    if let ExitReason::Stalled = test_result.exit_reason {
        std::process::exit(1);
    }

    Ok(())
}

fn browser_fuzz(
    output_path: PathBuf,
    shared_options: RunSharedOptions,
    browser_options: BrowserOptions,
    debugger_options: DebuggerOptions,
    fuzz_options: FuzzOptions,
) -> Result<()> {
    if antithesis::is_in_guest() {
        bail!(
            "bombadil fuzzing mode is not available in antithesis; use `test` or `test-external`"
        );
    };

    let specification = load_specification(&shared_options.target);

    let interrupted = Arc::new(AtomicBool::new(false));
    {
        let interrupted = interrupted.clone();
        ctrlc::set_handler(move || {
            interrupted.store(true, Ordering::SeqCst);
        })?;
    }

    let specification_bundle =
        Arc::from(bundle(".", &specification.module_specifier)?);

    let output_writer = FileOutputWriter {
        root_path: output_path.clone(),
        overwrite: shared_options.output_path_overwrite,
    };

    let allow_urls = resolve_allow_urls(&shared_options.target)?;
    let driver = Arc::new(BrowserDriver {
        origin: shared_options.target.origin.url,
        browser_options,
        debugger_options,
        specification_bundle,
    });

    fuzzer::fuzz(fuzzer::FuzzOptions {
        rng: AntithesisRng,
        driver,
        strategy: move || TestStrategy {
            rng: AntithesisRng,
            mode: TestMode::RandomWalk,
            exit_on_violation: false,
            test_start: None,
            deadline: None,
            violations_count: 0,
            allow_urls: allow_urls.clone(),
            quiet: true,
        },
        interrupted,
        time_limit_fuzz: fuzz_options.time_limit_fuzz,
        time_limit_run: fuzz_options.time_limit_run,
        swarm: fuzz_options.swarm,
        parallelism: fuzz_options.parallelism,
        output_writer,
    })?;

    // TODO: return result from `fuzz` that can be used to print something like the following:

    /*
    let heading = {
        let TestResult {
            exit_reason,
            violations_count,
        } = test_result;

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
    };

    let output_display = output_path.display();
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

    if test_result.violations_count > 0 {
        std::process::exit(2);
    }
        */

    Ok(())
}

struct RunOptions {
    run_id: RunId,
    origin: Url,
    specification: Specification,
    browser_options: BrowserOptions,
    debugger_options: DebuggerOptions,
    mode: TestMode,
    exit_on_violation: bool,
    deadline: Option<SystemTime>,
    allow_urls: Vec<MatchPattern>,
}

fn run_with_writer<Writer: TraceWriter<BrowserSession>>(
    RunOptions {
        run_id,
        origin,
        specification,
        browser_options,
        debugger_options,
        mode,
        exit_on_violation,
        deadline,
        allow_urls,
    }: RunOptions,
    trace_writer: Writer,
) -> Result<TestResult> {
    let interrupted = Arc::new(AtomicBool::new(false));
    {
        let interrupted = interrupted.clone();
        ctrlc::set_handler(move || {
            interrupted.store(true, Ordering::SeqCst);
        })?;
    }

    let mut strategy = TestStrategy {
        rng: AntithesisRng,
        mode,
        exit_on_violation,
        test_start: None,
        deadline,
        violations_count: 0,
        allow_urls,
        quiet: false,
    };

    bombadil_browser::runner::launch(
        run_id,
        origin,
        specification,
        browser_options,
        debugger_options,
        trace_writer,
        interrupted,
        &mut strategy,
    )
}

fn browser_shrink(
    target: TargetOptions,
    shrink_options: ShrinkOptions,
    launch_options: ManagedBrowserOptions,
) -> Result<()> {
    // Check the trace before looking for a browser to run it in.
    let options = shrink_options.resolve(|trace_path| {
        bombadil::shrink::load_trace_actions::<browser::BrowserAction>(
            &output_path::resolve_browser_trace_file(trace_path)?,
        )
    })?;
    let debugger_options = launch_options.debugger_options()?;
    let specification = load_specification(&target);
    let allow_urls = resolve_allow_urls(&target)?;
    let browser_options =
        browser_options_from_target(&target, &options.output_path);
    let driver = Arc::new(BrowserDriver {
        origin: target.origin.url.clone(),
        browser_options,
        debugger_options,
        specification_bundle: Arc::from(bundle(
            ".",
            &specification.module_specifier,
        )?),
    });

    let result = bombadil::shrink::shrink(bombadil::shrink::ShrinkOptions {
        driver,
        strategy: move || TestStrategy {
            rng: AntithesisRng,
            mode: TestMode::RandomWalk,
            exit_on_violation: false,
            test_start: None,
            deadline: None,
            violations_count: 0,
            allow_urls: allow_urls.clone(),
            quiet: true,
        },
        trace_writer: |path| FileTraceWriter::initialize(path, false),
        trace: options.trace.map_actions(|action| action.to_internal()),
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
        "Inspect the shrunk trace using:\n\n  {}\n",
        styled::maybe_italic(format!(
            "bombadil browser inspect {}",
            options.output_path.display()
        ))
    );
    Ok(())
}
