use std::{
    borrow::Cow,
    fs::File,
    io::{BufWriter, Write},
    path::{Path, PathBuf},
};

use anyhow::Result;
use bombadil::{
    driver::{OutputWriter, RunId, TraceWriter},
    specification::domain::Snapshot,
};
use bombadil_schema::browser::BrowserTraceEntry;
use bombadil_trace::{TraceDriver, TraceFileWriter};
use serde_json as json;

use crate::{
    browser::{actions::BrowserAction, state::BrowserState},
    convert::ToSchema,
    driver::BrowserSession,
    instrumentation::BlockRegistry,
    trace::{PropertyViolation, TraceEntry},
};

pub struct FileOutputWriter {
    pub root_path: PathBuf,
    pub overwrite: bool,
    /// The registry of the driver whose runs are written.
    pub blocks: BlockRegistry,
}

impl OutputWriter<BrowserSession> for FileOutputWriter {
    type TraceWriter = FileTraceWriter;

    fn trace_writer(&mut self, run_id: RunId) -> Result<Self::TraceWriter> {
        let run_path =
            bombadil_trace::numbered_trace_path(&self.root_path, run_id.0);
        FileTraceWriter::initialize(
            run_path,
            self.overwrite,
            self.blocks.clone(),
        )
    }
}

/// Writes a browser trace file: each state's metadata is a
/// [`BrowserTraceEntry`], with the screenshot's format in its
/// `screenshot` field and the screenshot itself as the state's payload.
pub struct FileTraceWriter {
    trace: TraceFileWriter<BrowserTraceEntry>,
    last_transition_hash: Option<u64>,
    /// The blocks instrumented by the driver of the run being written.
    blocks: BlockRegistry,
    /// `blocks.jsonl`: the coverage blocks that trace ids refer to.
    blocks_file: BufWriter<File>,
    /// Registered blocks already written to `blocks_file`.
    blocks_written: usize,
}

impl FileTraceWriter {
    /// Creates the trace file at `run_path`, refusing an existing trace
    /// unless `overwrite` is set. Other files are left alone.
    pub fn initialize(
        run_path: PathBuf,
        overwrite: bool,
        blocks: BlockRegistry,
    ) -> Result<Self> {
        let trace = TraceFileWriter::create(
            &run_path,
            TraceDriver::Browser,
            overwrite,
        )?;
        // The block table next to the trace file: `run-0.bin` has
        // `run-0.blocks.jsonl`.
        let blocks_file = File::create(blocks_path(&run_path))?;
        Ok(FileTraceWriter {
            trace,
            last_transition_hash: None,
            blocks,
            blocks_file: BufWriter::new(blocks_file),
            blocks_written: 0,
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
            branches: Cow::Borrowed(&state.coverage.branches),
        };
        self.last_transition_hash = state.transition_hash;

        let (blocks, cursor) = self.blocks.since(self.blocks_written);
        for block in blocks {
            json::to_writer(&mut self.blocks_file, &block)?;
            self.blocks_file.write_all(b"\n")?;
        }
        self.blocks_file.flush()?;
        self.blocks_written = cursor;

        // The screenshot is shared with the state, not copied.
        self.trace
            .write_state(entry.to_schema(), Some(state.screenshot.data.clone()))
    }

    fn finish(&mut self) -> Result<()> {
        self.blocks_file.flush()?;
        self.trace.finish()
    }
}

/// The block table of the trace at `trace_path`: the same name with
/// `.blocks.jsonl` in place of the extension.
pub fn blocks_path(trace_path: &Path) -> PathBuf {
    trace_path.with_extension("blocks.jsonl")
}

#[cfg(test)]
mod tests {
    use super::*;
    use bombadil_trace::TRACE_FILE_NAME;

    fn initialize(run_path: &std::path::Path, overwrite: bool) -> Result<()> {
        FileTraceWriter::initialize(
            run_path.to_path_buf(),
            overwrite,
            BlockRegistry::default(),
        )
        .map(drop)
    }

    #[test]
    fn refuses_an_existing_trace() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(TRACE_FILE_NAME);
        std::fs::write(&path, "previous").unwrap();
        let error = initialize(&path, false).unwrap_err();
        assert_eq!(
            error.to_string(),
            format!(
                "trace already exists at {}. Use \
                 --output-path-overwrite to overwrite, or choose a \
                 different --output-path.",
                path.display()
            )
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "previous");
    }

    #[test]
    fn overwriting_replaces_binary_trace_and_keeps_other_files() {
        let directory = tempfile::tempdir().unwrap();
        let trace = directory.path().join(TRACE_FILE_NAME);
        std::fs::write(&trace, "previous").unwrap();
        let screenshots = directory.path().join("screenshots");
        std::fs::create_dir(&screenshots).unwrap();
        std::fs::write(screenshots.join("0.jpeg"), "image").unwrap();
        initialize(&trace, true).unwrap();
        assert_eq!(
            bombadil_trace::trace_driver(&trace).unwrap(),
            Some(TraceDriver::Browser)
        );
        assert_eq!(
            std::fs::read(screenshots.join("0.jpeg")).unwrap(),
            b"image"
        );
    }
}
