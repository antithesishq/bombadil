use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;

use anyhow::Result;
use bombadil::runner::PropertyViolation;
use bombadil::specification::convert::ToSchema;
use bombadil::specification::domain::Snapshot;
use bombadil_schema::Time;
use bombadil_schema::swiftui::{self, ProcessExitStatus, SwiftUINode};
use serde::Serialize;

use crate::driver::SwiftUIAction;
use crate::state::SwiftUIState;

/// Writes one trace entry per state as JSON lines, serialized
/// byte-identically to [`swiftui::SwiftUITraceEntry`] but from borrowed
/// data so the node tree isn't cloned on the test loop. Unlike the
/// terminal's grid states, SwiftUI trees are small enough to serialize
/// inline.
pub struct TraceWriter {
    writer: BufWriter<File>,
}

/// Borrowing mirror of [`swiftui::SwiftUITraceEntry`]; field names and
/// order must match so reproduction can read the entries back.
#[derive(Serialize)]
struct TraceEntryRef<'a> {
    timestamp: Time,
    action: Option<swiftui::SwiftUIAction>,
    state: StateSummaryRef<'a>,
    snapshots: Vec<bombadil_schema::Snapshot>,
    violations: Vec<bombadil_schema::PropertyViolation>,
}

/// Borrowing mirror of [`swiftui::SwiftUIStateSummary`].
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StateSummaryRef<'a> {
    root: &'a Option<SwiftUINode>,
    exit_status: &'a Option<ProcessExitStatus>,
}

impl TraceWriter {
    pub fn initialize(
        root_path: PathBuf,
        output_path_overwrite: bool,
    ) -> Result<Self> {
        std::fs::create_dir_all(&root_path)?;
        let trace_path = root_path.join("trace.jsonl");
        if trace_path.try_exists()? {
            if !output_path_overwrite {
                anyhow::bail!(
                    "trace.jsonl already exists at {}. \
                     Use --output-path-overwrite to overwrite, or choose a different --output-path.",
                    trace_path.display(),
                );
            }
            std::fs::remove_file(&trace_path)?;
        }
        let trace_file = File::options()
            .write(true)
            .create_new(true)
            .open(&trace_path)?;
        log::info!("storing trace in {}", root_path.display());
        Ok(Self {
            writer: BufWriter::new(trace_file),
        })
    }

    pub fn write(
        &mut self,
        state: &SwiftUIState,
        last_action: Option<&SwiftUIAction>,
        snapshots: &[Snapshot],
        violations: &[PropertyViolation],
    ) -> Result<()> {
        let entry = TraceEntryRef {
            timestamp: Time::from_system_time(state.timestamp),
            action: last_action.map(ToSchema::to_schema),
            state: StateSummaryRef {
                root: &state.root,
                exit_status: &state.exit_status,
            },
            snapshots: snapshots.iter().map(ToSchema::to_schema).collect(),
            violations: violations.iter().map(ToSchema::to_schema).collect(),
        };
        serde_json::to_writer(&mut self.writer, &entry)?;
        self.writer.write_all(b"\n")?;
        Ok(())
    }

    pub fn flush(&mut self) -> Result<()> {
        self.writer.flush()?;
        Ok(())
    }
}
