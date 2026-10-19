//! Terminal traces: `trace.bin` files (see [`bombadil_trace`]) written by
//! the terminal driver.
//!
//! Each state's metadata is a [`TerminalTraceEntry`] (action, cursor, exit
//! status, extractor snapshots, violations), and its payload is ghostty's
//! binary snapshot of the whole terminal (`GHOSTSNP`), holding the grid,
//! scrollback and modes. The payload is left out when no snapshot could be
//! encoded for that state. Each snapshot carries its own format version,
//! which ghostty's decoder checks; that format has no cross-version
//! compatibility guarantee yet.

use std::{collections::BTreeMap, fs::File, io::BufReader, path::Path};

use anyhow::{Result, anyhow};
use bombadil::shrink::TraceActions;
use bombadil::{
    driver::{OutputWriter, RunId, TraceWriter},
    runner::PropertyViolation,
    specification::{convert::ToSchema, domain::Snapshot},
};
use bombadil_schema::Time;
use bombadil_schema::terminal::{
    self as schema, TerminalStateSummary, TerminalTraceEntry,
};
use bombadil_trace::{StateReader, TraceDriver, TraceFileWriter, open_trace};
use libghostty_vt::fmt::{Format, Formatter, FormatterOptions};

use crate::{
    driver::{TerminalAction, TerminalSession},
    state::TerminalState,
};

pub use bombadil_trace::TRACE_FILE_NAME;

pub struct TerminalOutputWriter {
    pub root_path: std::path::PathBuf,
    pub overwrite: bool,
}

impl OutputWriter<TerminalSession> for TerminalOutputWriter {
    type TraceWriter = TerminalTraceWriter;

    fn trace_writer(&mut self, run_id: RunId) -> Result<Self::TraceWriter> {
        let run_path =
            self.root_path.join("runs").join(format!("{}", run_id.0));
        TerminalTraceWriter::initialize(&run_path, self.overwrite)
    }
}

pub struct TerminalTraceWriter(TraceFileWriter<TerminalTraceEntry>);

impl TerminalTraceWriter {
    pub fn initialize(run_path: &Path, overwrite: bool) -> Result<Self> {
        TraceFileWriter::create(run_path, TraceDriver::Terminal, overwrite)
            .map(TerminalTraceWriter)
    }

    pub fn flush(&mut self) -> Result<()> {
        self.0.flush()
    }
}

impl TraceWriter<TerminalSession> for TerminalTraceWriter {
    #[hotpath::measure]
    fn write(
        &mut self,
        state: &TerminalState,
        last_action: Option<&TerminalAction>,
        snapshots: &[Snapshot],
        violations: &[PropertyViolation],
    ) -> Result<()> {
        let entry = TerminalTraceEntry {
            timestamp: Time::from_system_time(state.timestamp),
            action: last_action.map(ToSchema::to_schema),
            state: TerminalStateSummary {
                scroll_offset: state.scroll_offset,
                cursor: state.cursor.clone(),
                exit_status: state.exit_status.clone(),
            },
            snapshots: snapshots.iter().map(|s| s.to_schema()).collect(),
            violations: violations.iter().map(|v| v.to_schema()).collect(),
        };
        // The snapshot is shared with the state, not copied.
        self.0.write_state(entry, state.terminal_snapshot.clone())
    }
}

pub type TerminalStates = StateReader<BufReader<File>, TerminalTraceEntry>;

/// Opens a terminal trace file, or the trace in a run directory.
pub fn open_terminal_trace(path: &Path) -> Result<TerminalStates> {
    open_trace(path)?.expect_driver(TraceDriver::Terminal)
}

/// The actions of a trace (file or run directory) and the earliest
/// violation of each property, for shrinking.
pub fn load_trace_actions(
    path: &Path,
) -> Result<TraceActions<schema::TerminalAction>> {
    let mut actions = Vec::new();
    let mut violations_earliest = BTreeMap::new();
    for state in open_terminal_trace(path)? {
        let entry = state?.metadata;
        if let Some(action) = entry.action {
            actions.push(action);
        }
        for violation in entry.violations {
            violations_earliest
                .entry(violation.name)
                .or_insert(actions.len());
        }
    }
    Ok(TraceActions {
        actions,
        violations_earliest,
    })
}

/// Renders the active screen of a terminal snapshot as text, with its
/// colors and styles as ANSI escape sequences if `color` is set.
pub fn render_screen(snapshot: &[u8], color: bool) -> Result<String> {
    let terminal = libghostty_vt::snapshot::Decoder::new_buf(snapshot)
        .and_then(|decoder| decoder.decode())
        .map_err(|error| {
            anyhow!("failed to decode terminal snapshot: {error}")
        })?;
    let format = if color { Format::Vt } else { Format::Plain };
    let mut formatter = Formatter::new(
        &terminal,
        FormatterOptions::new().with_format(format).with_trim(true),
    )
    .map_err(|error| anyhow!("failed to render terminal snapshot: {error}"))?;
    let screen = formatter.format_alloc(None).map_err(|error| {
        anyhow!("failed to render terminal snapshot: {error}")
    })?;
    Ok(String::from_utf8_lossy(&screen).into_owned())
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::SystemTime};

    use bombadil_schema::terminal::{
        TerminalColor, TerminalCursor, TerminalCursorPosition,
        TerminalCursorVisualStyle, TerminalGrid, TerminalSize,
    };
    use bombadil_trace::{tag, write_envelope, write_record};

    use super::*;

    fn state(terminal_snapshot: Option<Vec<u8>>) -> TerminalState {
        let size = TerminalSize {
            columns: 10,
            rows: 2,
        };
        TerminalState {
            timestamp: SystemTime::now(),
            grid: TerminalGrid::with_size(size),
            scrollback: TerminalGrid::with_size(TerminalSize {
                rows: 0,
                ..size
            }),
            scroll_offset: 3,
            cursor: TerminalCursor {
                position: TerminalCursorPosition { column: 5, row: 0 },
                visible: true,
                blinking: false,
                visual_style: TerminalCursorVisualStyle::Block,
                color: TerminalColor::None,
            },
            exit_status: None,
            last_action: None,
            terminal_snapshot: terminal_snapshot.map(Arc::new),
        }
    }

    fn snapshot(input: &[u8]) -> Vec<u8> {
        let mut terminal = libghostty_vt::Terminal::new(10, 2).unwrap();
        terminal.set_continuation_max_bytes(1024).unwrap();
        terminal.vt_write(input);
        let mut snapshot = Vec::new();
        terminal.encode_snapshot(&mut snapshot).unwrap();
        snapshot
    }

    #[test]
    fn test_trace_round_trip() {
        // End mid-escape-sequence so the snapshot has to carry it.
        let snapshot = snapshot(b"hello\x1b[3");
        let directory = tempfile::tempdir().unwrap();
        let action = TerminalAction::TypeText {
            text: "hi".to_string(),
        };
        let mut writer =
            TerminalTraceWriter::initialize(directory.path(), false).unwrap();
        writer
            .write(&state(Some(snapshot.clone())), Some(&action), &[], &[])
            .unwrap();
        writer.write(&state(None), None, &[], &[]).unwrap();
        drop(writer);

        let states = open_terminal_trace(directory.path())
            .unwrap()
            .collect::<Result<Vec<_>>>()
            .unwrap();
        assert_eq!(states.len(), 2);
        assert!(matches!(
            &states[0].metadata.action,
            Some(schema::TerminalAction::TypeText { text }) if text == "hi"
        ));
        assert_eq!(states[0].metadata.state.scroll_offset, 3);
        assert!(states[1].payload.is_none());

        let restored_snapshot = states[0].payload.as_ref().unwrap();
        assert_eq!(restored_snapshot, &snapshot);
        let mut restored =
            libghostty_vt::snapshot::Decoder::new_buf(restored_snapshot)
                .unwrap()
                .decode()
                .unwrap();
        assert_eq!(restored.cursor_x().unwrap(), 5);
        restored.vt_write(b"1mX");
        assert_eq!(restored.cursor_x().unwrap(), 6);
    }

    #[test]
    fn test_rejects_other_drivers() {
        let directory = tempfile::tempdir().unwrap();
        let mut file = Vec::new();
        write_envelope(&mut file, TraceDriver::Browser).unwrap();
        std::fs::write(directory.path().join(TRACE_FILE_NAME), file).unwrap();
        let error = open_terminal_trace(directory.path()).err().unwrap();
        assert!(
            error.to_string().contains(
                "not a terminal trace (written by the browser driver)"
            ),
            "{error}"
        );
    }

    #[test]
    fn test_load_trace_actions_for_shrinking() {
        let state = |action: &str, violated: &[&str]| {
            let violations = violated
                .iter()
                .map(|name| {
                    format!(
                        r#"{{"name":"{name}","violation":{{"False":{{"time":0,"condition":"c","snapshots":[]}}}}}}"#
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            format!(
                r#"{{"timestamp":0,"action":{action},"state":{{"scroll_offset":0,"cursor":{{"position":{{"column":0,"row":0}},"visible":true,"blinking":false,"visual_style":"Block","color":"None"}},"exit_status":null}},"snapshots":[],"violations":[{violations}]}}"#
            )
        };
        let mut file = Vec::new();
        write_envelope(&mut file, TraceDriver::Terminal).unwrap();
        for metadata in [
            state("null", &[]),
            state(r#"{"ScrollUp":{}}"#, &["p"]),
            state(r#"{"ScrollDown":{}}"#, &["q"]),
            state(r#"{"ScrollUp":{}}"#, &["p"]),
        ] {
            write_record(&mut file, tag::STATE, &[metadata.as_bytes()])
                .unwrap();
        }
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join(TRACE_FILE_NAME), &file).unwrap();

        let trace = load_trace_actions(directory.path()).unwrap();
        assert_eq!(trace.actions.len(), 3);
        assert_eq!(trace.select_property(Some("p")).unwrap(), ("p".into(), 1));
        assert_eq!(trace.select_property(Some("q")).unwrap(), ("q".into(), 2));
    }

    #[test]
    fn test_render_screen() {
        let snapshot = snapshot(b"hello\r\n\x1b[31mred\x1b[0m");
        assert_eq!(render_screen(&snapshot, false).unwrap(), "hello\nred");
        let colored = render_screen(&snapshot, true).unwrap();
        assert!(
            colored.contains("red") && colored.contains("\x1b["),
            "{colored:?}"
        );
    }
}
