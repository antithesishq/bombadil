use std::path::PathBuf;

use anyhow::Result;
use bombadil::{
    driver::{OutputWriter, RunId, TraceWriter},
    model::{Picked, Recorded, StepModel},
    specification::domain::Snapshot,
    tree::Leaves,
};
use bombadil_schema::{Time, TraceEntry, browser::BrowserStateSummary};
use bombadil_trace::{StepError, TraceDriver, TraceFileWriter};
use serde_json::value::RawValue;

use crate::{
    browser::{
        actions::{BrowserAction, BrowserActionTemplate},
        state::BrowserState,
    },
    convert::ToSchema,
    driver::BrowserSession,
    trace::PropertyViolation,
};

pub struct FileOutputWriter {
    pub root_path: PathBuf,
    pub overwrite: bool,
}

impl OutputWriter<BrowserSession> for FileOutputWriter {
    type TraceWriter = FileTraceWriter;

    fn trace_writer(&mut self, run_id: RunId) -> Result<Self::TraceWriter> {
        let run_path =
            bombadil_trace::numbered_trace_path(&self.root_path, run_id.0);
        FileTraceWriter::initialize(run_path, self.overwrite)
    }
}

/// Writes a browser trace file: each state's metadata is a
/// [`BrowserTraceEntry`], with the screenshot's format in its
/// `screenshot` field and the screenshot itself as the state's payload.
pub struct FileTraceWriter {
    trace: TraceFileWriter<TraceEntry<Box<RawValue>, BrowserStateSummary>>,
    // The last action, as encoded, for the state it leads to.
    action: Option<Box<RawValue>>,
}

impl FileTraceWriter {
    /// Creates the trace file at `run_path`, refusing an existing trace
    /// unless `overwrite` is set. Other files are left alone.
    pub fn initialize(run_path: PathBuf, overwrite: bool) -> Result<Self> {
        let trace = TraceFileWriter::create(
            &run_path,
            TraceDriver::Browser,
            overwrite,
        )?;
        Ok(FileTraceWriter {
            trace,
            action: None,
        })
    }
}

fn encode(action: &BrowserAction) -> Result<Box<RawValue>> {
    Ok(serde_json::value::to_raw_value(&action.to_schema())?)
}

impl TraceWriter<BrowserSession> for FileTraceWriter {
    fn write(
        &mut self,
        state: &BrowserState,
        snapshots: &[Snapshot],
        violations: &[PropertyViolation],
        actions_available: &Leaves<'_, BrowserActionTemplate>,
        action_previous: Option<&Picked<BrowserAction>>,
    ) -> Result<()> {
        let model = StepModel::new(
            actions_available,
            action_previous.map(|picked| picked.template),
        );
        let action = match (self.action.take(), action_previous) {
            (Some(action), Some(_)) => Some(action),
            (_, Some(picked)) => Some(encode(&picked.action)?),
            (_, None) => None,
        };
        let entry = TraceEntry {
            timestamp: Time::from_system_time(state.timestamp),
            action,
            state: BrowserStateSummary {
                url: state.url.to_string(),
                screenshot: state.screenshot.format.extension().to_string(),
                resources: state.resources.to_schema(),
                actions_available: Some(model.actions_available),
                action_taken: model.action_taken,
                branches: Some(state.coverage.branches.clone()),
            },
            snapshots: snapshots.iter().map(|s| s.to_schema()).collect(),
            violations: violations.iter().map(|v| v.to_schema()).collect(),
        };
        // The screenshot is shared with the state, not copied.
        self.trace
            .write_state(entry, Some(state.screenshot.data.clone()))
    }

    fn write_action(&mut self, picked: &Picked<BrowserAction>) -> Result<()> {
        let action = encode(&picked.action)?;
        self.trace.write_action(&Recorded {
            action: &*action,
            template: Some(picked.template),
        })?;
        self.action = Some(action);
        Ok(())
    }

    fn write_error(&mut self, error: StepError) -> Result<()> {
        self.trace.write_error(error)
    }

    fn finish(&mut self) -> Result<()> {
        self.trace.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bombadil_trace::TRACE_FILE_NAME;

    fn initialize(run_path: &std::path::Path, overwrite: bool) -> Result<()> {
        FileTraceWriter::initialize(run_path.to_path_buf(), overwrite).map(drop)
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
