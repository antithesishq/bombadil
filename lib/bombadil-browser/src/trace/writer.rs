use std::{
    borrow::Cow,
    io::{BufWriter, Write},
    path::PathBuf,
    time::UNIX_EPOCH,
};

use anyhow::{Context, Result};
use bombadil::{
    driver::{OutputWriter, RunId, TraceWriter},
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

pub struct FileTraceWriter {
    screenshots_path: PathBuf,
    trace_file: BufWriter<File>,
    last_transition_hash: Option<u64>,
}

impl FileTraceWriter {
    pub fn initialize(run_path: PathBuf, overwrite: bool) -> Result<Self> {
        log::info!(
            "storing run trace in {}",
            run_path
                .to_str()
                .expect("states directory path is not valid unicode")
        );
        let trace_file_path = run_path.join("trace.jsonl");
        // A terminal run's binary trace counts as an existing trace too,
        // and is removed when overwriting so the formats don't mix.
        for name in ["trace.jsonl", "trace.bin"] {
            let path = run_path.join(name);
            if path.try_exists()? {
                if !overwrite {
                    anyhow::bail!(
                        "{name} already exists at {}. \
                         Use --output-path-overwrite to overwrite, or choose a different --output-path.",
                        path.display(),
                    );
                }
                std::fs::remove_file(&path).with_context(|| {
                    format!("failed to remove {}", path.display())
                })?;
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A terminal run's `trace.bin` is refused without overwriting, and
    /// removed when overwriting, so the directory holds one trace.
    #[test]
    fn refuses_or_replaces_a_binary_trace() {
        let directory = tempfile::tempdir().unwrap();
        let binary_trace = directory.path().join("trace.bin");
        std::fs::write(&binary_trace, "previous").unwrap();
        let error =
            FileTraceWriter::initialize(directory.path().to_path_buf(), false)
                .err()
                .unwrap();
        assert!(
            error.to_string().starts_with("trace.bin already exists at"),
            "{error}"
        );
        assert!(binary_trace.exists());
        assert!(!directory.path().join("trace.jsonl").exists());

        FileTraceWriter::initialize(directory.path().to_path_buf(), true)
            .unwrap();
        assert!(!binary_trace.exists());
        assert!(directory.path().join("trace.jsonl").is_file());
    }
}
