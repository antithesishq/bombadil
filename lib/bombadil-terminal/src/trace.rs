use std::{
    io::{BufWriter, Write},
    path::PathBuf,
};

use anyhow::Result;
use bombadil::runner::PropertyViolation;
use bombadil::specification::convert::ToSchema;
use bombadil::specification::domain::Snapshot;
use bombadil_schema::{TerminalGrid, Time, TraceEntry};
use serde::Serialize;
use serde_json as json;
use std::fs::File;

use crate::{driver::TerminalAction, state::TerminalState};

pub type TerminalTraceEntry =
    TraceEntry<TerminalAction, bombadil_schema::TerminalStateSummary>;

pub struct TraceWriter {
    trace_file: BufWriter<File>,
    buffer: Vec<u8>,
}

#[derive(Serialize)]
struct BorrowedTerminalTraceEntry<'a> {
    timestamp: Time,
    action: Option<&'a TerminalAction>,
    state: TerminalStateSummary<'a>,
    snapshots: Vec<bombadil_schema::Snapshot>,
    violations: Vec<bombadil_schema::PropertyViolation>,
}

#[derive(Serialize)]
struct TerminalStateSummary<'a> {
    grid: &'a TerminalGrid,
    scrollback: &'a TerminalGrid,
    scroll_offset: u32,
    terminated: bool,
}

impl TraceWriter {
    pub fn initialize(root_path: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&root_path)?;
        let trace_path = root_path.join("trace.jsonl");
        let trace_file = File::options()
            .append(true)
            .create(true)
            .open(&trace_path)?;
        log::info!("storing trace in {}", root_path.display());
        Ok(Self {
            trace_file: BufWriter::new(trace_file),
            buffer: Vec::new(),
        })
    }

    #[hotpath::measure]
    pub fn write(
        &mut self,
        state: &TerminalState,
        last_action: Option<&TerminalAction>,
        snapshots: &[Snapshot],
        violations: &[PropertyViolation],
    ) -> Result<()> {
        let entry = BorrowedTerminalTraceEntry {
            timestamp: Time::from_system_time(state.timestamp),
            action: last_action,
            state: TerminalStateSummary {
                grid: &state.grid,
                scrollback: &state.scrollback,
                scroll_offset: state.scroll_offset,
                terminated: state.terminated,
            },
            snapshots: snapshots.iter().map(|s| s.to_schema()).collect(),
            violations: violations.iter().map(|v| v.to_schema()).collect(),
        };
        self.buffer.clear();
        json::to_writer(&mut self.buffer, &entry)?;
        self.buffer.push(b'\n');
        self.trace_file.write_all(&self.buffer)?;
        Ok(())
    }

    pub fn flush(&mut self) -> Result<()> {
        self.trace_file.flush()?;
        Ok(())
    }
}
