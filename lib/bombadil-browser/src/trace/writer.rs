use std::{
    borrow::Cow,
    io::{BufWriter, Write},
    path::PathBuf,
    time::UNIX_EPOCH,
};

use anyhow::Result;
use bombadil::{
    driver::{RunId, TraceWriter, TraceWriterOutput},
    specification::domain::Snapshot,
};
use serde_json as json;
use std::fs::File;

use crate::{
    browser::{actions::BrowserAction, state::BrowserState},
    convert::ToSchema,
    driver::BrowserSession,
    trace::{PropertyViolation, TraceEntry},
};

pub struct FileTraceWriter {
    screenshots_path: PathBuf,
    trace_file: BufWriter<File>,
    last_transition_hash: Option<u64>,
}

impl FileTraceWriter {
    pub fn initialize(
        output: &TraceWriterOutput,
        run_id: RunId,
    ) -> Result<Self> {
        let run_path =
            output.root_path.join("runs").join(format!("{}", run_id.0));
        log::info!(
            "storing run trace in {}",
            run_path
                .to_str()
                .expect("states directory path is not valid unicode")
        );
        let trace_file_path = run_path.join("trace.jsonl");
        if trace_file_path.try_exists()? {
            if !output.overwrite {
                anyhow::bail!(
                    "trace.jsonl already exists at {}. \
                     Use --output-path-overwrite to overwrite, or choose a different --output-path.",
                    trace_file_path.display(),
                );
            }
            std::fs::remove_file(&trace_file_path)?;
        }
        let screenshots_path = run_path.join("screenshots");
        std::fs::create_dir_all(&screenshots_path)?;
        let trace_file = File::options()
            .write(true)
            .create_new(true)
            .open(&trace_file_path)?;
        Ok(FileTraceWriter {
            screenshots_path,
            trace_file: BufWriter::new(trace_file),
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
        let screenshot_path = self.screenshots_path.join(format!(
            "{}.{}",
            state.timestamp.duration_since(UNIX_EPOCH)?.as_micros(),
            state.screenshot.format.extension()
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

        json::to_writer(&mut self.trace_file, &entry.to_schema())?;
        self.trace_file.write_all(b"\n")?;
        self.trace_file.flush()?;

        Ok(())
    }
}
