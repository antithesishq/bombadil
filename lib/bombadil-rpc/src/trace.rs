use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use bombadil::driver::{OutputWriter, RunId, RunState, TraceWriter};
use bombadil::runner::PropertyViolation;
use bombadil::shrink::TraceActions;
use bombadil::specification::convert::ToSchema;
use bombadil::specification::domain::Snapshot;
use bombadil_schema::TraceEntry;
use bombadil_trace::{TraceDriver, TraceFileWriter, open_trace};
use serde::{Deserialize, Serialize};
use serde_json as json;

use crate::action::ExternalAction;
use crate::driver::{ExternalSession, ExternalState};

const TRACE_DRIVER: TraceDriver = TraceDriver::Unknown(0xFFFF);

/// A trace entry of an external driver: its actions and states are JSON,
/// and it records what the driver said about the state besides the state
/// itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalTraceEntry {
    #[serde(flatten)]
    pub entry: TraceEntry<json::Value, json::Value>,
    /// Present, and `false`, only if the driver reported the state before
    /// the app settled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settled: Option<bool>,
    /// Why the driver didn't perform `action`, if it didn't.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_applied: Option<String>,
}

pub struct ExternalOutputWriter {
    pub root_path: PathBuf,
    pub overwrite: bool,
}

impl OutputWriter<ExternalSession> for ExternalOutputWriter {
    type TraceWriter = ExternalTraceWriter;

    fn trace_writer(&mut self, run_id: RunId) -> Result<Self::TraceWriter> {
        let run_path =
            bombadil_trace::numbered_trace_path(&self.root_path, run_id.0);
        ExternalTraceWriter::initialize(run_path, self.overwrite)
    }
}

/// Writes a trace file, one [`ExternalTraceEntry`] per state.
pub struct ExternalTraceWriter {
    file: TraceFileWriter<ExternalTraceEntry>,
}

impl ExternalTraceWriter {
    pub fn initialize(run_path: PathBuf, overwrite: bool) -> Result<Self> {
        let file = TraceFileWriter::create(&run_path, TRACE_DRIVER, overwrite)?;
        Ok(Self { file })
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
            entry: TraceEntry {
                timestamp: state.timestamp(),
                action: last_action.map(|action| action.0.clone()),
                state: state.value.clone(),
                snapshots: snapshots.iter().map(ToSchema::to_schema).collect(),
                violations: violations
                    .iter()
                    .map(ToSchema::to_schema)
                    .collect(),
            },
            settled: (!state.settled).then_some(false),
            not_applied: state.not_applied.clone(),
        };
        self.file.write_state(entry, state.screenshot_png.clone())
    }

    fn finish(&mut self) -> Result<()> {
        self.file.finish()
    }
}

pub fn load_trace_actions(path: &Path) -> Result<TraceActions<ExternalAction>> {
    let mut trace = TraceActions {
        actions: Vec::new(),
        violations_earliest: BTreeMap::new(),
    };
    let mut states = 0;
    for state in open_trace::<ExternalTraceEntry>(path)?
        .expect_driver(TRACE_DRIVER)?
        .without_payloads()
    {
        let entry = state?.metadata.entry;
        states += 1;
        if let Some(action) = entry.action {
            trace.actions.push(ExternalAction(action));
        }
        for violation in entry.violations {
            trace
                .violations_earliest
                .entry(violation.name)
                .or_insert(trace.actions.len());
        }
    }
    if states == 0 {
        bail!("{} holds no states", path.display());
    }
    Ok(trace)
}
