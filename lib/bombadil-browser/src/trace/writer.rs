use std::{borrow::Cow, io::Write, path::PathBuf, time::UNIX_EPOCH};

use anyhow::Result;
use bombadil::specification::domain::Snapshot;
use serde_json as json;
use std::fs::File;

use crate::{
    browser::{actions::BrowserAction, state::BrowserState},
    convert::ToSchema,
    strategy::TraceWriter,
    trace::{PropertyViolation, TraceEntry},
};

pub struct FileTraceWriter {
    screenshots_path: PathBuf,
    trace_file: File,
    last_transition_hash: Option<u64>,
}

impl FileTraceWriter {
    pub fn initialize(root_path: PathBuf) -> Result<Self> {
        log::info!(
            "storing trace in {}",
            &root_path
                .to_str()
                .expect("states directory path is not valid unicode")
        );
        let screenshots_path = root_path.join("screenshots");
        std::fs::create_dir_all(&screenshots_path)?;
        let trace_file = File::options()
            .append(true)
            .create(true)
            .open(root_path.join("trace.jsonl"))?;
        Ok(FileTraceWriter {
            screenshots_path,
            trace_file,
            last_transition_hash: None,
        })
    }
}

impl TraceWriter for FileTraceWriter {
    fn write(
        &mut self,
        state: &BrowserState,
        last_action: Option<&BrowserAction>,
        snapshots: &[Snapshot],
        violations: &[PropertyViolation],
    ) -> Result<()> {
        let screenshot_path = self.screenshots_path.join(format!(
            "{}.{}",
            state.timestamp.duration_since(UNIX_EPOCH)?.as_micros(),
            &state.screenshot.format.extension()
        ));
        File::create_new(&screenshot_path)?
            .write_all(&state.screenshot.data)?;

        let entry = TraceEntry {
            timestamp: state.timestamp,
            url: Cow::Borrowed(&state.url),
            hash_previous: self.last_transition_hash,
            hash_current: state.transition_hash,
            action: last_action.map(Cow::Borrowed),
            screenshot: Cow::Owned(screenshot_path),
            snapshots: Cow::Borrowed(snapshots),
            violations: Cow::Borrowed(violations),
            resources: Cow::Borrowed(&state.resources),
        };

        self.last_transition_hash = state.transition_hash;

        self.trace_file
            .write_all(json::to_string(&entry.to_schema())?.as_bytes())?;
        self.trace_file.write_all(b"\n")?;

        Ok(())
    }
}
