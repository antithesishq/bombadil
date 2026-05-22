use std::{collections::VecDeque, path::PathBuf, process::exit};

use anyhow::{Result, anyhow, bail};
use bombadil::runner::Runner;
use bombadil::specification::verifier::Specification;
use bombadil_terminal::driver::{Size, TerminalAction, TerminalDriver};
use bombadil_terminal::trace::{TerminalTraceEntry, TraceWriter};
use bombadil_terminal::{TerminalStrategy, TerminalTestMode};
use tempfile::TempDir;
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, BufReader};

const DEFAULT_COLUMNS: u16 = 100;
const DEFAULT_ROWS: u16 = 40;
const MAX_SCROLLBACK: usize = 1_000;

#[derive(clap::Subcommand)]
pub enum Command {
    /// [EXPERIMENTAL] Test the given program against a TypeScript specification
    Test {
        /// Path to a TypeScript specification file (uses the
        /// `@antithesishq/bombadil/terminal` API). Required: there is no
        /// default terminal specification yet.
        #[arg(long = "specification")]
        specification_file: PathBuf,
        /// Terminal columns at startup
        #[arg(long, default_value_t = DEFAULT_COLUMNS)]
        columns: u16,
        /// Terminal rows at startup
        #[arg(long, default_value_t = DEFAULT_ROWS)]
        rows: u16,
        /// Where to store output data (trace.jsonl). Defaults to a
        /// fresh temporary directory.
        #[arg(long)]
        output_path: Option<PathBuf>,
        /// Reproduce a previous test run from a trace file (file path
        /// or directory containing `trace.jsonl`). Replays the recorded
        /// actions in order instead of generating new ones.
        #[arg(long, value_name = "TRACE_FILE")]
        reproduce: Option<PathBuf>,
        /// The command to run as the system under test. Everything after
        /// `--` is forwarded as program + arguments.
        #[clap(trailing_var_arg = true)]
        command: Vec<String>,
    },
}

pub async fn run(command: Command) {
    match command {
        Command::Test {
            specification_file,
            columns,
            rows,
            output_path,
            reproduce,
            command,
        } => {
            if let Err(error) = run_test(
                specification_file,
                Size { columns, rows },
                output_path,
                reproduce,
                &command,
            )
            .await
            {
                eprintln!("\n\nterminal test failed: {error}");
                exit(1);
            }
        }
    }
}

async fn run_test(
    specification_file: PathBuf,
    size: Size,
    output_path: Option<PathBuf>,
    reproduce: Option<PathBuf>,
    command: &[String],
) -> Result<()> {
    let (program, args) = match command {
        [program, args @ ..] => (program.as_str(), args),
        _ => bail!("expected `<program> [args...]` after `--`"),
    };

    // Prepend "./" for relative paths that don't already start with "."
    // so the bundler treats them as paths rather than bare specifiers.
    let specification_file = if specification_file.is_relative()
        && !specification_file.starts_with(".")
    {
        PathBuf::from(".").join(specification_file)
    } else {
        specification_file
    };

    let specification = Specification {
        module_specifier: specification_file.display().to_string(),
    };

    let output_path = resolve_output_path(output_path)?;
    let writer = TraceWriter::initialize(output_path.clone()).await?;

    let mode = match reproduce {
        Some(path) => {
            TerminalTestMode::Reproduce(load_reproduce_actions(&path).await?)
        }
        None => TerminalTestMode::RandomWalk,
    };

    let (driver, verifier) = TerminalDriver::launch(
        specification,
        size,
        MAX_SCROLLBACK,
        program,
        args,
    )
    .await?;

    let runner = Runner::new(driver, verifier);
    let mut strategy = TerminalStrategy {
        mode,
        writer: Some(writer),
        test_start: None,
        violations_count: 0,
    };
    let _ = runner.run(&mut strategy).await?;

    println!("\nTrace written to: {}", output_path.display());

    if strategy.violations_count > 0 {
        bail!("{} violation(s) reported", strategy.violations_count);
    }
    Ok(())
}

fn resolve_output_path(output_path: Option<PathBuf>) -> Result<PathBuf> {
    match output_path {
        Some(path) => Ok(path),
        None => Ok(TempDir::with_prefix("bombadil_terminal_")?
            .keep()
            .to_path_buf()),
    }
}

async fn load_reproduce_actions(
    path: &std::path::Path,
) -> Result<VecDeque<TerminalAction>> {
    let trace_file_path = if path.is_dir() {
        path.join("trace.jsonl")
    } else {
        path.to_path_buf()
    };
    let file = File::open(&trace_file_path).await.map_err(|error| {
        anyhow!(
            "failed to open trace file {}: {}",
            trace_file_path.display(),
            error
        )
    })?;
    let mut lines = BufReader::new(file).lines();
    let mut actions: VecDeque<TerminalAction> = VecDeque::new();
    while let Some(line) = lines.next_line().await? {
        let entry: TerminalTraceEntry = serde_json::from_str(&line)?;
        if let Some(action) = entry.action {
            actions.push_back(action);
        }
    }
    Ok(actions)
}
