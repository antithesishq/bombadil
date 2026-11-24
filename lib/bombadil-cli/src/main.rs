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
    // clap exits with 2 on usage errors by default, which collides with the
    // documented exit code for property violations. Use 1 instead, keeping
    // 0 for --help and --version.
    let cli = Cli::try_parse().unwrap_or_else(|error| {
        let _ = error.print();
        std::process::exit(if error.use_stderr() { 1 } else { 0 })
    });
    bombadil_cli::run(cli)
}
