mod browser;
mod duration;
mod inspect_server;
mod output_path;
#[cfg(feature = "terminal")]
mod terminal;

use anyhow::Result;

/// Property-based testing for web UIs
#[derive(clap::Parser)]
#[command(name = "bombadil", version, about, long_about=None)]
pub struct Cli {
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
    /// (EXPERIMENTAL) Property-based testing for terminal UIs
    #[cfg(feature = "terminal")]
    Terminal {
        #[command(subcommand)]
        command: terminal::Command,
    },
}

pub fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Browser { command } => browser::run(command),
        #[cfg(feature = "terminal")]
        Command::Terminal { command } => {
            terminal::run(command);
            Ok(())
        }
    }
}
