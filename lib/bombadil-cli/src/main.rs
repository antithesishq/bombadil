use antithesis_sdk::antithesis_init;
use anyhow::Result;
use bombadil_cli::Cli;
use clap::Parser;

#[hotpath::main]
fn main() -> Result<()> {
    let env = env_logger::Env::default().default_filter_or("warn");
    env_logger::Builder::from_env(env)
        .format_timestamp_millis()
        .format_target(true)
        .filter_module("html5ever", log::LevelFilter::Info)
        .init();
    antithesis_init();
    bombadil_cli::run(Cli::parse())
}
