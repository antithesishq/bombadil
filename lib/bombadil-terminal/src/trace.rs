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

use anyhow::{Result, anyhow, bail};
use bombadil::shrink::TraceActions;
use bombadil::{
    driver::{OutputWriter, RunId, TraceWriter},
    runner::PropertyViolation,
    specification::{convert::ToSchema, domain::Snapshot},
};
use bombadil_schema::Time;
use bombadil_schema::terminal::TerminalSize;
use bombadil_schema::terminal::{
    self as schema, TerminalStateSummary, TerminalTraceEntry,
};
use bombadil_trace::{StateReader, TraceDriver, TraceFileWriter, open_trace};
use libghostty_vt::{
    RenderState,
    fmt::{Extras, Format, Formatter},
    point::{Coordinate, Point},
    render::{CellIterator, RowIterator},
    selection::Selection,
    terminal::ScrollViewport,
};

use crate::{
    driver::{TerminalAction, TerminalSession, viewport_grid},
    js::row_text,
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
            bombadil_trace::numbered_trace_path(&self.root_path, run_id.0);
        TerminalTraceWriter::initialize(&run_path, self.overwrite)
    }
}

pub struct TerminalTraceWriter(TraceFileWriter<TerminalTraceEntry>);

impl TerminalTraceWriter {
    /// Creates the trace file at `run_path`, replacing an existing file
    /// only when `overwrite` is set. Parent directories must exist.
    pub fn initialize(run_path: &Path, overwrite: bool) -> Result<Self> {
        TraceFileWriter::create(run_path, TraceDriver::Terminal, overwrite)
            .map(TerminalTraceWriter)
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

    fn finish(&mut self) -> Result<()> {
        self.0.finish()
    }
}

pub type TerminalStates = StateReader<BufReader<File>, TerminalTraceEntry>;

/// Opens a terminal trace file.
pub fn open_terminal_trace(path: &Path) -> Result<TerminalStates> {
    open_trace(path)?.expect_driver(TraceDriver::Terminal)
}

/// The actions of a trace file and the earliest
/// violation of each property, for reproducing and shrinking.
pub fn load_trace_actions(
    path: &Path,
) -> Result<TraceActions<schema::TerminalAction>> {
    let mut trace = TraceActions {
        actions: Vec::new(),
        violations_earliest: BTreeMap::new(),
    };
    let mut states = 0;
    for state in open_terminal_trace(path)?.without_payloads() {
        states += 1;
        let entry = state?.metadata;
        if let Some(action) = entry.action {
            trace.actions.push(action);
        }
        for violation in entry.violations {
            trace
                .violations_earliest
                .entry(violation.name)
                .or_insert(trace.actions.len());
        }
    }
    if states == 0 {
        bail!("{} holds no states", path.display());
    }
    Ok(trace)
}

/// Renders the screen of a terminal snapshot as text, one line per row
/// of the viewport, blank rows included, so screens of consecutive states
/// line up. The viewport is `scroll_offset` rows from the top of the
/// scrollback (the state's `scroll_offset`), as the state's properties
/// saw it, or the bottom (the active screen) if `None`. Trailing
/// spaces are trimmed from each line. Its colors and styles are ANSI
/// escape sequences if `color` is set. The text is the terminal's, so
/// callers showing it must escape it.
pub fn render_screen(
    snapshot: &[u8],
    scroll_offset: Option<usize>,
    color: bool,
) -> Result<Vec<String>> {
    let render_error =
        |error| anyhow!("failed to render terminal snapshot: {error}");
    let mut terminal = libghostty_vt::snapshot::Decoder::new_buf(snapshot)
        .and_then(|decoder| decoder.decode())
        .map_err(|error| {
            anyhow!("failed to decode terminal snapshot: {error}")
        })?;
    terminal.scroll_viewport(match scroll_offset {
        Some(offset) => ScrollViewport::Row(offset),
        None => ScrollViewport::Bottom,
    });
    let (columns, rows) = (
        terminal.cols().map_err(render_error)?,
        terminal.rows().map_err(render_error)?,
    );
    if !color {
        // From the cells as the properties saw them: ghostty's plain
        // formatter leaves out marks on blank cells.
        let mut render_state = RenderState::new()?;
        let snapshot = render_state.update(&terminal)?;
        let grid = viewport_grid(
            &snapshot,
            &mut RowIterator::new()?,
            &mut CellIterator::new()?,
            TerminalSize { columns, rows },
        )?;
        return Ok((0..rows)
            .map(|y| row_text(&grid, y).trim_end_matches(' ').to_string())
            .collect());
    }
    let corner = |x, y| {
        terminal
            .grid_ref(Point::Viewport(Coordinate { x, y }))
            .map_err(render_error)
    };
    // Row by row: formatting the viewport at once leaves out blank rows
    // at its top and bottom.
    (0..u32::from(rows))
        .map(|y| {
            let row =
                Selection::new(corner(0, y)?, corner(columns - 1, y)?, false);
            let mut formatter =
                Formatter::new(&terminal).map_err(render_error)?;
            formatter
                .set_format(Format::Vt)?
                .set_extras(Extras::NONE)?
                .set_trim(true)?;
            // SAFETY: Both endpoints came from this terminal's active
            // screen above, and it cannot change while the formatter lives.
            unsafe { formatter.set_selection(Some(&row)) }
                .map_err(render_error)?;
            let line = formatter.format_alloc(None).map_err(render_error)?;
            let mut line = String::from_utf8_lossy(&line).into_owned();
            // VT formatting emits cell hyperlinks as OSC 8, independently
            // of the extra cursor hyperlink option. Keep only their text.
            while let Some(start) = line.find("\x1b]8;") {
                let Some(end) = line[start..].find("\x1b\\") else {
                    break;
                };
                line.replace_range(start..start + end + 2, "");
            }
            Ok(line)
        })
        .collect()
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
        let mut writer = TerminalTraceWriter::initialize(
            &directory.path().join(TRACE_FILE_NAME),
            false,
        )
        .unwrap();
        writer
            .write(&state(Some(snapshot.clone())), Some(&action), &[], &[])
            .unwrap();
        writer.write(&state(None), None, &[], &[]).unwrap();
        writer.finish().unwrap();

        let mut reader =
            open_terminal_trace(&directory.path().join(TRACE_FILE_NAME))
                .unwrap();
        let states = reader.by_ref().collect::<Result<Vec<_>>>().unwrap();
        assert!(reader.finished());
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

    /// A sibling browser trace is neither a conflict nor cleanup material.
    #[test]
    fn test_leaves_sibling_browser_traces_alone() {
        let directory = tempfile::tempdir().unwrap();
        let browser_trace = directory.path().join("trace.jsonl");
        std::fs::write(&browser_trace, "previous").unwrap();
        let path = directory.path().join("arbitrary.custom");
        for overwrite in [false, true] {
            TerminalTraceWriter::initialize(&path, overwrite)
                .unwrap()
                .finish()
                .unwrap();
            assert_eq!(std::fs::read(&browser_trace).unwrap(), b"previous");
            assert!(path.is_file());
        }
    }

    #[test]
    fn test_rejects_other_drivers() {
        let directory = tempfile::tempdir().unwrap();
        let mut file = Vec::new();
        write_envelope(&mut file, TraceDriver::Browser).unwrap();
        std::fs::write(directory.path().join(TRACE_FILE_NAME), file).unwrap();
        let error =
            open_terminal_trace(&directory.path().join(TRACE_FILE_NAME))
                .err()
                .unwrap();
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
        write_record(&mut file, tag::END, &[]).unwrap();
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join(TRACE_FILE_NAME), &file).unwrap();

        let trace = load_trace_actions(&directory.path().join(TRACE_FILE_NAME))
            .unwrap();
        assert_eq!(trace.actions.len(), 3);
        assert_eq!(trace.select_property(Some("p")).unwrap(), ("p".into(), 1));
        assert_eq!(trace.select_property(Some("q")).unwrap(), ("q".into(), 2));

        // A trace file of any name, as `--trace` and `--reproduce` take.
        let renamed = directory.path().join("run-k2.bin");
        std::fs::rename(directory.path().join(TRACE_FILE_NAME), &renamed)
            .unwrap();
        assert_eq!(load_trace_actions(&renamed).unwrap().actions.len(), 3);
    }

    #[test]
    fn test_render_screen() {
        let snapshot = snapshot(b"hello\r\n\x1b[31mred\x1b[0m");
        assert_eq!(
            render_screen(&snapshot, Some(0), false).unwrap(),
            ["hello", "red"]
        );
        let colored = render_screen(&snapshot, Some(0), true).unwrap();
        assert_eq!(colored.len(), 2);
        assert!(
            colored[1].contains("red") && colored[1].contains("\x1b["),
            "{colored:?}"
        );
    }

    /// Marks on blank cells, which the properties see, are kept.
    #[test]
    fn test_render_screen_keeps_marks_on_blank_cells() {
        let snapshot = snapshot("B: \u{301}|\r\nD: \u{202e}|".as_bytes());
        let screen = ["B: \u{301}|", "D: \u{202e}|"];
        assert_eq!(render_screen(&snapshot, Some(0), false).unwrap(), screen);
        let colored = render_screen(&snapshot, Some(0), true).unwrap();
        assert_eq!(colored, screen);
    }

    #[test]
    fn test_render_screen_shows_the_viewport_only() {
        // Two rows of screen below two of scrollback.
        let snapshot = snapshot(b"one\r\ntwo\r\nthree\r\nfour");
        assert_eq!(
            render_screen(&snapshot, Some(2), false).unwrap(),
            ["three", "four"]
        );
        assert_eq!(
            render_screen(&snapshot, Some(1), false).unwrap(),
            ["two", "three"]
        );
    }

    /// Without a recorded scroll offset, the screen is the active one at
    /// the bottom of the scrollback, not the top.
    #[test]
    fn test_render_screen_defaults_to_the_bottom() {
        let mut terminal = libghostty_vt::Terminal::new(10, 2).unwrap();
        terminal.vt_write(b"one\r\ntwo\r\nthree\r\nfour");
        // Scrolled away when encoded, which the snapshot doesn't keep.
        terminal.scroll_viewport(ScrollViewport::Top);
        let mut snapshot = Vec::new();
        terminal.encode_snapshot(&mut snapshot).unwrap();
        assert_eq!(
            render_screen(&snapshot, None, false).unwrap(),
            ["three", "four"]
        );
        assert_eq!(
            render_screen(&snapshot, Some(0), false).unwrap(),
            ["one", "two"]
        );
    }

    /// Every row of the viewport is a line, blank rows at the top and
    /// bottom included, with trailing spaces trimmed; soft-wrapped
    /// rows stay apart.
    #[test]
    fn test_render_screen_is_full_height() {
        let mut terminal = libghostty_vt::Terminal::new(6, 6).unwrap();
        terminal.vt_write(b"\r\n a  \r\n\r\nabcdefgh");
        let mut snapshot = Vec::new();
        terminal.encode_snapshot(&mut snapshot).unwrap();
        let screen = ["", " a", "", "abcdef", "gh", ""];
        assert_eq!(render_screen(&snapshot, None, false).unwrap(), screen);
        assert_eq!(render_screen(&snapshot, None, true).unwrap().len(), 6);
        // Blank.
        let blank = self::snapshot(b"");
        assert_eq!(render_screen(&blank, None, false).unwrap(), ["", ""]);
    }

    /// A file that isn't a binary trace is reported with its path, and a
    /// trace with no states is refused, as there is nothing to reproduce
    /// or shrink.
    #[test]
    fn test_load_trace_actions_refuses_non_traces_and_empty_traces() {
        let directory = tempfile::tempdir().unwrap();
        for name in ["random.dat", "package.json", TRACE_FILE_NAME] {
            let file = directory.path().join(name);
            std::fs::write(&file, b"{\"name\":\"p\"}\n").unwrap();
            let error = load_trace_actions(&file).err().unwrap();
            assert_eq!(
                error.to_string(),
                format!("failed to read {}", file.display())
            );
            assert_eq!(error.root_cause().to_string(), "not a bombadil trace");
        }

        // A binary trace with no states.
        let mut file = Vec::new();
        write_envelope(&mut file, TraceDriver::Terminal).unwrap();
        let trace = directory.path().join(TRACE_FILE_NAME);
        std::fs::write(&trace, file).unwrap();
        assert!(load_trace_actions(directory.path()).is_err());
        let error = load_trace_actions(&trace).err().unwrap();
        assert_eq!(
            error.to_string(),
            format!("{} holds no states", trace.display())
        );
    }
}
