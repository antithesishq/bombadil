use std::path::PathBuf;

use anyhow::Result;
use bombadil::runner::PropertyViolation;
use bombadil::specification::convert::ToSchema;
use bombadil::specification::domain::Snapshot;
use bombadil_schema::{TerminalSize, TerminalStateSummary, Time, TraceEntry};
use serde_json as json;
use tokio::{fs::File, io::AsyncWriteExt};

use crate::{driver::TerminalAction, state::TerminalState};

pub type TerminalTraceEntry = TraceEntry<TerminalAction, TerminalStateSummary>;

pub struct TraceWriter {
    trace_file: File,
}

impl TraceWriter {
    pub async fn initialize(root_path: PathBuf) -> Result<Self> {
        tokio::fs::create_dir_all(&root_path).await?;
        let trace_path = root_path.join("trace.jsonl");
        let trace_file = File::options()
            .append(true)
            .create(true)
            .open(&trace_path)
            .await?;
        log::info!("storing trace in {}", root_path.display());
        Ok(Self { trace_file })
    }

    pub async fn write(
        &mut self,
        state: &TerminalState,
        last_action: Option<&TerminalAction>,
        snapshots: &[Snapshot],
        violations: &[PropertyViolation],
    ) -> Result<()> {
        let entry = TerminalTraceEntry {
            timestamp: Time::from_system_time(state.timestamp),
            action: last_action.cloned(),
            state: state_summary_from_state(state),
            snapshots: snapshots.iter().map(|s| s.to_schema()).collect(),
            violations: violations.iter().map(|v| v.to_schema()).collect(),
        };
        self.trace_file
            .write_all(json::to_string(&entry)?.as_bytes())
            .await?;
        self.trace_file.write_u8(b'\n').await?;
        Ok(())
    }
}

pub fn state_summary_from_state(state: &TerminalState) -> TerminalStateSummary {
    TerminalStateSummary {
        size: TerminalSize {
            columns: state.size.columns,
            rows: state.size.rows,
        },
        rows: state.rows.clone(),
        scrollback: state.scrollback.clone(),
        scroll_offset: state.scroll_offset,
        terminated: state.terminated,
    }
}
