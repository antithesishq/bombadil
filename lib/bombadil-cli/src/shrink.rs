use std::{
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use anyhow::{Result, bail};
use bombadil::{
    shrink::{ShrinkExitReason, ShrinkResult, ShrinkSettings, TraceActions},
    styled,
};

use crate::{duration, output_path};

/// Options shared by the shrink commands of all drivers.
#[derive(clap::Args)]
pub struct ShrinkOptions {
    /// Trace to shrink (trace file, or the run directory containing it)
    #[arg(long = "trace", value_name = "TRACE_PATH")]
    pub trace_path: PathBuf,
    /// Name of the property to shrink for. Required if the trace has
    /// violations of more than one property.
    #[arg(long)]
    pub property: Option<String>,
    /// Where to store the shrunk trace. Defaults to a fresh temporary
    /// directory.
    #[arg(long)]
    pub output_path: Option<PathBuf>,
    /// Overwrite any existing trace at `--output-path`.
    #[arg(long)]
    pub output_path_overwrite: bool,
    /// Maximum time to shrink for, after which the smallest trace found
    /// so far is kept. Accepts a number with a unit suffix: s (seconds), m
    /// (minutes), h (hours), or d (days).
    #[arg(long, value_parser = duration::parse_duration)]
    pub time_limit: Option<Duration>,
    /// Number of test runs to execute in parallel. Only use this if runs
    /// are isolated from each other (e.g. not sharing files or a backend).
    #[arg(long, default_value = "1")]
    pub parallelism: NonZeroUsize,
    /// Number of times to replay the original trace to measure how
    /// reliably it reproduces the violation.
    #[arg(long, default_value = "3")]
    pub calibration_runs: NonZeroUsize,
    /// Maximum number of runs used to confirm that a smaller trace
    /// reproduces reliably enough.
    #[arg(long, default_value = "5")]
    pub confirmation_runs_max: NonZeroUsize,
}

pub struct ResolvedShrinkOptions<A> {
    pub trace: TraceActions<A>,
    pub property_name: Option<String>,
    pub output_path: PathBuf,
    pub output_path_overwrite: bool,
    pub time_limit: Option<Duration>,
    pub parallelism: NonZeroUsize,
    pub settings: ShrinkSettings,
    pub interrupted: Arc<AtomicBool>,
}

impl ShrinkOptions {
    /// `load_trace` reads the driver's trace from the trace's run
    /// directory.
    pub fn resolve<A>(
        self,
        load_trace: impl FnOnce(&Path) -> Result<TraceActions<A>>,
    ) -> Result<ResolvedShrinkOptions<A>> {
        let trace_directory =
            output_path::resolve_trace_directory(&self.trace_path);
        let trace = load_trace(&trace_directory)?;
        let output_path = output_path::resolve_output_path(&self.output_path)?;
        if same_directory(&trace_directory, &output_path) {
            bail!(
                "the output path must differ from the directory of the trace being shrunk"
            );
        }

        let interrupted = Arc::new(AtomicBool::new(false));
        {
            let interrupted = interrupted.clone();
            ctrlc::set_handler(move || {
                interrupted.store(true, Ordering::SeqCst);
            })?;
        }

        Ok(ResolvedShrinkOptions {
            trace,
            property_name: self.property,
            output_path,
            output_path_overwrite: self.output_path_overwrite,
            time_limit: self.time_limit,
            parallelism: self.parallelism,
            settings: ShrinkSettings {
                calibration_runs: self.calibration_runs,
                confirmation_runs_max: self.confirmation_runs_max,
                ..ShrinkSettings::default()
            },
            interrupted,
        })
    }
}

fn same_directory(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

pub fn print_result(result: &ShrinkResult) {
    let heading = match result.exit_reason {
        ShrinkExitReason::Minimal => "Shrinking finished!",
        ShrinkExitReason::TimeLimit => "Shrinking stopped after time limit!",
        ShrinkExitReason::Interrupted => "Shrinking was interrupted by SIGINT!",
    };
    let ShrinkResult {
        property_name,
        trace_length_original,
        trace_length_truncated,
        trace_length_shrunk,
        calibration,
        confirmation,
        run_count,
        elapsed,
        ..
    } = result;
    let elapsed = duration::format_duration(*elapsed);
    println!("\n{}\n", styled::maybe_bold(heading.to_string()));
    println!("  Property:   {property_name}");
    println!(
        "  Actions:    {trace_length_original} → {trace_length_shrunk} \
         ({trace_length_truncated} up to earliest violation)"
    );
    println!(
        "  Reproduced: {}/{} (original: {}/{})",
        confirmation.hit_count,
        confirmation.run_count,
        calibration.hit_count,
        calibration.run_count,
    );
    println!("  Runs:       {run_count} in {elapsed}\n");
}
