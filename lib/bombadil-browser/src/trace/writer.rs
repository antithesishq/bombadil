use std::{borrow::Cow, path::PathBuf};

use anyhow::Result;
use bombadil::{
    driver::{OutputWriter, RunId, TraceWriter},
    model::{Picked, StepModel, TemplateRegistry},
    specification::domain::Snapshot,
    tree::Leaves,
};
use bombadil_schema::browser::BrowserTraceEntry;
use bombadil_trace::{TraceDriver, TraceFileWriter};

use crate::{
    browser::{
        actions::{BrowserAction, BrowserActionTemplate},
        state::BrowserState,
    },
    convert::ToSchema,
    driver::BrowserSession,
    trace::{PropertyViolation, TraceEntry},
};

pub struct FileOutputWriter {
    pub root_path: PathBuf,
    pub overwrite: bool,
    pub templates: Option<TemplateRegistry>,
}

impl OutputWriter<BrowserSession> for FileOutputWriter {
    type TraceWriter = FileTraceWriter;

    fn trace_writer(&mut self, run_id: RunId) -> Result<Self::TraceWriter> {
        let run_path =
            bombadil_trace::numbered_trace_path(&self.root_path, run_id.0);
        FileTraceWriter::initialize(run_path, self.overwrite)
            .map(|writer| writer.with_templates(self.templates.clone()))
    }
}

/// Writes a browser trace file: each state's metadata is a
/// [`BrowserTraceEntry`], with the screenshot's format in its
/// `screenshot` field and the screenshot itself as the state's payload.
pub struct FileTraceWriter {
    trace: TraceFileWriter<BrowserTraceEntry>,
    last_transition_hash: Option<u64>,
    templates: Option<TemplateRegistry>,
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
            last_transition_hash: None,
            templates: None,
        })
    }

    /// Records the templates of the actions this trace offers in `templates`.
    pub fn with_templates(self, templates: Option<TemplateRegistry>) -> Self {
        FileTraceWriter { templates, ..self }
    }
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
        let last_action = action_previous.map(|picked| &picked.action);
        let model = StepModel::new(
            actions_available,
            action_previous.map(|picked| picked.template),
        );
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
            actions_available: Cow::Borrowed(&model.actions_available),
            action_taken: model.action_taken,
            branches: Cow::Borrowed(&state.coverage.branches),
        };
        self.last_transition_hash = state.transition_hash;
        if let Some(templates) = &self.templates {
            templates.record(actions_available, &model)?;
        }
        // The screenshot is shared with the state, not copied.
        self.trace
            .write_state(entry.to_schema(), Some(state.screenshot.data.clone()))
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
