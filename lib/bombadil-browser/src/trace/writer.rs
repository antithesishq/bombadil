use std::{borrow::Cow, path::PathBuf};

use anyhow::Result;
use bombadil::{
    driver::{OutputWriter, RunId, TraceWriter},
    specification::domain::Snapshot,
};
use bombadil_schema::browser::BrowserTraceEntry;
use bombadil_trace::{TraceDriver, TraceFileWriter};

use crate::{
    browser::{actions::BrowserAction, state::BrowserState},
    convert::ToSchema,
    driver::BrowserSession,
    trace::{PropertyViolation, TraceEntry},
};

pub struct FileOutputWriter {
    pub root_path: PathBuf,
    pub overwrite: bool,
}

impl FileOutputWriter {
    pub fn run_directory(&self, run_id: RunId) -> PathBuf {
        self.root_path.join("runs").join(format!("{}", run_id.0))
    }
}

impl OutputWriter<BrowserSession> for FileOutputWriter {
    type TraceWriter = FileTraceWriter;

    fn trace_writer(&mut self, run_id: RunId) -> Result<Self::TraceWriter> {
        let run_path =
            self.root_path.join("runs").join(format!("{}", run_id.0));
        FileTraceWriter::initialize(run_path, self.overwrite)
    }
}

/// Writes a browser trace as `trace.bin`: each state's metadata is a
/// [`BrowserTraceEntry`], with the screenshot's format in its
/// `screenshot` field and the screenshot itself as the state's payload.
pub struct FileTraceWriter {
    trace: TraceFileWriter<BrowserTraceEntry>,
    last_transition_hash: Option<u64>,
}

impl FileTraceWriter {
    pub fn initialize(run_path: PathBuf, overwrite: bool) -> Result<Self> {
        Ok(FileTraceWriter {
            trace: TraceFileWriter::create(
                &run_path,
                TraceDriver::Browser,
                overwrite,
            )?,
            last_transition_hash: None,
        })
    }
}

impl TraceWriter<BrowserSession> for FileTraceWriter {
    fn write(
        &mut self,
        state: &BrowserState,
        last_action: Option<&BrowserAction>,
        snapshots: &[Snapshot],
        violations: &[PropertyViolation],
    ) -> Result<()> {
        let entry = TraceEntry {
            timestamp: state.timestamp,
            url: Cow::Borrowed(&state.url),
            hash_previous: self.last_transition_hash,
            hash_current: state.transition_hash,
            action: last_action.map(Cow::Borrowed),
            screenshot: state.screenshot.format.extension(),
            snapshots: Cow::Borrowed(snapshots),
            violations: Cow::Borrowed(violations),
            resources: Cow::Borrowed(&state.resources),
        };
        self.last_transition_hash = state.transition_hash;
        // The screenshot is shared with the state, not copied.
        self.trace
            .write_state(entry.to_schema(), Some(state.screenshot.data.clone()))
    }
}
