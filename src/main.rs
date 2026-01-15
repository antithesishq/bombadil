use ::url::Url;
use anyhow::Result;
use clap::Parser;
use serde_json as json;
use std::str::FromStr;
use tempfile::TempDir;
use tokio::io::AsyncWriteExt;
use tokio::{fs::File, sync::broadcast};

use antithesis_browser::{
    browser::BrowserOptions, proxy::Proxy, runner::run_test,
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
            // TODO: make this configurable with CLI option
            let states_directory = TempDir::with_prefix("states_")?.keep();
            let browser_options = BrowserOptions {
                headless,
                user_data_directory: user_data_directory.path().to_path_buf(),
                width,
                height,
                no_sandbox,
                proxy: None,
            };
            let mut events = run_test(origin.url, &browser_options).await?;

            let mut trace_file = File::options()
                .append(true)
                .create(true)
                .open(states_directory.join("trace.jsonl"))
                .await?;
            let screenshots_dir_path = states_directory.join("screenshots");
            tokio::fs::create_dir_all(&screenshots_dir_path).await?;

            loop {
                match events.recv().await {
                    Ok(
                        antithesis_browser::runner::RunEvent::NewTraceEntry {
                            entry,
                            violation,
                        },
                    ) => {
                        log::debug!("new trace entry: {:?}", entry);

                        let screenshot_path = screenshots_dir_path.join(
                            entry
                                .screenshot_path
                                .file_name()
                                .expect("screenshot must have a file name"),
                        );
                        // TODO: keep screenshot in memory until this point, no need to copy.
                        tokio::fs::copy(
                            &entry.screenshot_path,
                            &screenshot_path,
                        )
                        .await?;

                        trace_file
                            .write(json::to_string(&entry)?.as_bytes())
                            .await?;
                        trace_file.write_u8(b'\n').await?;

                        if let Some(hash) = entry.hash_current {
                            log::info!("got new transition hash: {:?}", hash);
                        };

                        if let Some(ref err) = *violation {
                            if exit_on_violation {
                                eprintln!("violation: {}", err);
                                std::process::exit(2);
                            } else {
                                log::error!("violation: {}", err);
                            }
                        }
                    }
                    Ok(antithesis_browser::runner::RunEvent::Error(err)) => {
                        eprintln!("{}", err);
                        std::process::exit(1);
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
