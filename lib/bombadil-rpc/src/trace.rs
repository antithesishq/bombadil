use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;

use anyhow::Result;
use bombadil::driver::{OutputWriter, RunId, RunState, TraceWriter};
use bombadil::runner::PropertyViolation;
use bombadil::specification::convert::ToSchema;
use bombadil::specification::domain::Snapshot;
use bombadil_schema::TraceEntry;
use serde_json as json;

use crate::action::ExternalAction;
use crate::driver::{ExternalSession, ExternalState};

/// A trace entry of an external driver: its actions and states are JSON.
pub type ExternalTraceEntry = TraceEntry<json::Value, json::Value>;

pub struct ExternalOutputWriter {
    pub root_path: PathBuf,
    pub overwrite: bool,
}

impl OutputWriter<ExternalSession> for ExternalOutputWriter {
    type TraceWriter = ExternalTraceWriter;

    fn trace_writer(&mut self, run_id: RunId) -> Result<Self::TraceWriter> {
        let run_path = self.root_path.join("runs").join(run_id.to_string());
        ExternalTraceWriter::initialize(run_path, self.overwrite)
    }
}

/// Writes `trace.jsonl`, one [`ExternalTraceEntry`] per state.
pub struct ExternalTraceWriter {
    file: BufWriter<File>,
}

impl ExternalTraceWriter {
    pub fn initialize(run_path: PathBuf, overwrite: bool) -> Result<Self> {
        std::fs::create_dir_all(&run_path)?;
        let trace_path = run_path.join("trace.jsonl");
        if trace_path.try_exists()? && !overwrite {
            anyhow::bail!(
                "trace.jsonl already exists at {}. \
                 Use --output-path-overwrite to overwrite, or choose a different --output-path.",
                trace_path.display(),
            );
        }
        log::info!("storing run trace in {}", run_path.display());
        Ok(Self {
            file: BufWriter::new(File::create(&trace_path)?),
        })
    }
}

impl TraceWriter<ExternalSession> for ExternalTraceWriter {
    fn write(
        &mut self,
        state: &ExternalState,
        last_action: Option<&ExternalAction>,
        snapshots: &[Snapshot],
        violations: &[PropertyViolation],
    ) -> Result<()> {
        let entry = ExternalTraceEntry {
            timestamp: state.timestamp(),
            action: last_action.map(|action| action.0.clone()),
            state: state.value.clone(),
            snapshots: snapshots.iter().map(ToSchema::to_schema).collect(),
            violations: violations.iter().map(ToSchema::to_schema).collect(),
        };
        json::to_writer(&mut self.file, &entry)?;
        self.file.write_all(b"\n")?;
        // Flushed per entry so a trace survives the driver crashing us.
        self.file.flush()?;
        Ok(())
    }
}
