use std::path::PathBuf;

use anyhow::Result;
use bombadil::runner::PropertyViolation;
use bombadil::specification::convert::ToSchema;
use bombadil::specification::domain::Snapshot;
use bombadil_schema::{Time, TraceEntry};
use serde::{Deserialize, Serialize};
use serde_json as json;
use tokio::{fs::File, io::AsyncWriteExt};

use crate::driver::{Size, TerminalAction, TerminalState};

/// Subset of [`TerminalState`] that ends up in the JSONL trace.
/// Timestamp lives at the `TraceEntry` level; `last_action` is the
/// entry's `action` field. Everything else is the on-disk snapshot of
/// the rendered grid + bookkeeping.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TerminalStateSummary {
    pub size: Size,
    pub rows: Vec<String>,
    pub scrollback: Vec<String>,
    pub scroll_offset: u32,
    pub finished: bool,
}

impl TerminalStateSummary {
    pub fn from_state(state: &TerminalState) -> Self {
        Self {
            size: state.size,
            rows: state.rows.clone(),
            scrollback: state.scrollback.clone(),
            scroll_offset: state.scroll_offset,
            finished: state.finished,
        }
    }
}

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
            state: TerminalStateSummary::from_state(state),
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
