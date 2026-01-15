use std::str::FromStr;

use ::url::Url;
use anyhow::Result;
use clap::Parser;
use tempfile::TempDir;
use tokio::sync::broadcast;

use antithesis_browser::{
    browser::BrowserOptions,
    proxy::Proxy,
    runner::{run_test, RunnerOptions},
};

#[derive(Parser)]
#[command(version, about)]
struct CLI {
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Subcommand)]
enum Command {
    Test {
        origin: Origin,
        #[arg(long)]
        seed: Option<String>,
        #[arg(long, default_value_t = false)]
        headless: bool,
        #[arg(long, default_value_t = false)]
        no_sandbox: bool,
        #[arg(long, default_value_t = 1024)]
        width: u16,
        #[arg(long, default_value_t = 768)]
        height: u16,
        #[arg(long, default_value_t = false)]
        exit_on_violation: bool,
    },
    Proxy {
        #[arg(long)]
        port: u16,
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
        .init();
    let cli = CLI::parse();
    match cli.command {
        Command::Test {
            origin,
            seed: _,
            headless,
            width,
            height,
            no_sandbox,
            exit_on_violation,
        } => {
            let user_data_directory = TempDir::with_prefix("user_data_")?;
            let states_directory = TempDir::with_prefix("states_")?;
            let runner_options = RunnerOptions {
                exit_on_violation,
                states_directory: states_directory.keep(),
            };
            let browser_options = BrowserOptions {
                headless,
                user_data_directory: user_data_directory.path().to_path_buf(),
                width,
                height,
                no_sandbox,
                proxy: None,
            };
            let mut events =
                run_test(origin.url, &runner_options, &browser_options).await?;
            loop {
                match events.recv().await {
                    Ok(event) => {
                        log::info!("{:?}", event);
                    }
                    Err(broadcast::error::RecvError::Closed) => return Ok(()),
                    Err(err) => {
                        eprintln!("{}", err);
                        std::process::exit(1);
                    }
                }
            }
        }
        Command::Proxy { port } => {
            let mut proxy = Proxy::spawn(port).await?;
            log::info!("proxy started on 127.0.0.1:{}", proxy.port);
            Ok(proxy.done().await)
        }
    }
}
