mod browser;
mod duration;
mod inspect_server;
mod output_path;
#[cfg(feature = "terminal")]
mod terminal;

use anyhow::Result;
use clap::Parser;

/// Property-based testing for web UIs
#[derive(Parser)]
#[command(name = "bombadil", version, about, long_about=None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Subcommand)]
#[allow(clippy::large_enum_variant)]
enum Command {
    /// Property-based testing for web UIs
    Browser {
        #[command(subcommand)]
        command: browser::BrowserCommand,
    },
    /// [EXPERIMENTAL] Property-based testing for terminal UIs
    #[cfg(feature = "terminal")]
    Terminal {
        #[command(subcommand)]
        command: terminal::Command,
    },
}

#[hotpath::main]
fn main() -> Result<()> {
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
        Command::Browser { command } => {
            tokio::runtime::Runtime::new()?.block_on(browser::run(command))
        }
        #[cfg(feature = "terminal")]
        Command::Terminal { command } => {
            terminal::run(command);
            Ok(())
        }
    }
}
