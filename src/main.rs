use ::url::Url;
use anyhow::Result;
use clap::{Args, Parser};
use std::{path::PathBuf, str::FromStr};
use tempfile::TempDir;

use bombadil::{
    browser::{BrowserOptions, DebuggerOptions, Emulation, LaunchOptions},
    runner::{Runner, RunnerOptions},
    specification::{render::render_violation, verifier::Specification},
    trace::writer::TraceWriter,
};

#[derive(Parser)]
#[command(version, about)]
struct CLI {
    #[command(subcommand)]
    command: Command,
}

#[derive(Args)]
struct TestSharedOptions {
    origin: Origin,
    specification_file: Option<PathBuf>,
    #[arg(long)]
    output_path: Option<PathBuf>,
    #[arg(long)]
    exit_on_violation: bool,
    #[arg(long, default_value_t = 1024)]
    width: u16,
    #[arg(long, default_value_t = 768)]
    height: u16,
    #[arg(long, default_value_t = 2.0)]
    device_scale_factor: f64,
}

#[derive(clap::Subcommand)]
enum Command {
    Test {
        #[clap(flatten)]
        shared: TestSharedOptions,
        #[arg(long, default_value_t = false)]
        headless: bool,
        #[arg(long, default_value_t = false)]
        no_sandbox: bool,
    },
    TestExternal {
        #[clap(flatten)]
        shared: TestSharedOptions,
        #[arg(long)]
        remote_debugger: Url,
        #[arg(long)]
        create_target: bool,
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

#[tokio::main]
async fn main() -> Result<()> {
    let env = env_logger::Env::default().default_filter_or("info");
    env_logger::Builder::from_env(env)
        .format_timestamp_millis()
        .format_target(true)
        // Until we hav a fix for https://github.com/mattsse/chromiumoxide/issues/287
        .filter_module("chromiumoxide::browser", log::LevelFilter::Error)
        .filter_module("html5ever", log::LevelFilter::Info)
        .init();
    let cli = CLI::parse();
    match cli.command {
        Command::Test {
            shared,
            headless,
            no_sandbox,
        } => {
            let user_data_directory = TempDir::with_prefix("user_data_")?;

            let browser_options = BrowserOptions {
                create_target: true,
                emulation: Emulation {
                    width: shared.width,
                    height: shared.height,
                    device_scale_factor: shared.device_scale_factor,
                },
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
            test(shared, browser_options, debugger_options).await
        }
        Command::TestExternal {
            shared,
            remote_debugger,
            create_target,
        } => {
            let browser_options = BrowserOptions {
                create_target,
                emulation: Emulation {
                    width: shared.width,
                    height: shared.height,
                    device_scale_factor: shared.device_scale_factor,
                },
            };
            let debugger_options = DebuggerOptions::External {
                remote_debugger,
            };
            test(shared, browser_options, debugger_options).await
        }
    }
}

async fn test(
    shared_options: TestSharedOptions,
    browser_options: BrowserOptions,
    debugger_options: DebuggerOptions,
) -> Result<()> {
    // Load a user-provided specification, or use the defaults provided by Bombadil.
    let specification = if let Some(path) = &shared_options.specification_file {
        log::info!("loading specification from file: {}", path.display());
        Specification::from_path(path.as_path()).await?
    } else {
        log::info!("using default specification");
        Specification::from_string(
            r#"
                export * from "bombadil/defaults";
            "#,
            PathBuf::from("default_spec.js").as_path(),
        )?
    };

    let output_path = match shared_options.output_path {
        Some(path) => path,
        None => TempDir::with_prefix("states_")?.keep().to_path_buf(),
    };

    let runner = Runner::new(
        shared_options.origin.url,
        specification,
        RunnerOptions {
            stop_on_violation: shared_options.exit_on_violation,
        },
        browser_options,
        debugger_options,
    )
    .await?;
    let mut events = runner.start();
    let mut writer = TraceWriter::initialize(output_path).await?;

    let exit_code: anyhow::Result<Option<i32>> = async {
        loop {
            match events.next().await {
                Ok(Some(bombadil::runner::RunEvent::NewState {
                    state,
                    last_action,
                    violations,
                })) => {
                    let has_violations = !violations.is_empty();

                    for violation in &violations {
                        log::error!(
                            "violation of property `{}`:\n{}",
                            violation.name,
                            render_violation(&violation.violation)
                        );
                    }

                    writer.write(last_action, state, violations).await?;

                    if has_violations && shared_options.exit_on_violation {
                        break Ok(Some(2));
                    }
                }
                Ok(None) => break Ok(None),
                Err(err) => {
                    eprintln!("next run event failure: {}", err);
                    break Ok(Some(1));
                }
            }
        }
    }
    .await;

    events.shutdown().await?;

    if let Some(exit_code) = exit_code? {
        std::process::exit(exit_code);
    }

    Ok(())
}
