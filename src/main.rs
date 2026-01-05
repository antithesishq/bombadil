use ::url::Url;
use anyhow::Result;
use clap::Parser;
use tempfile::TempDir;

use antithesis_browser::{browser::BrowserOptions, runner::run_test};

#[derive(Parser)]
#[command(version, about)]
struct CLI {
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Subcommand)]
enum Command {
    Test {
        origin: Url,
        #[arg(long)]
        seed: Option<String>,
        #[arg(long, default_value_t = false)]
        headless: bool,
        #[arg(long, default_value_t = 1024)]
        width: u16,
        #[arg(long, default_value_t = 768)]
        height: u16,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::builder()
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
        } => {
            let user_data_directory = TempDir::new()?;

            match run_test(
                origin,
                BrowserOptions {
                    headless,
                    user_data_directory: user_data_directory
                        .path()
                        .to_path_buf(),
                    width,
                    height,
                },
            )
            .await
            {
                Ok(()) => Ok(()),
                Err(error) => {
                    eprintln!("Test failed: {}", error);
                    std::process::exit(2);
                }
            }
        }
    }
}
