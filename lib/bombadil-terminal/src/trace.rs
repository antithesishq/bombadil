//! Terminal traces: `trace.bin` files (see [`bombadil_trace`]) written by
//! the terminal driver.

use std::{
    collections::BTreeMap, fmt::Write, fs::File, io::BufReader, path::Path,
    sync::Arc,
};

use anyhow::{Context, Result, bail};
use bombadil::shrink::TraceActions;
use bombadil::{
    driver::{OutputWriter, RunId, TraceWriter},
    model::{Picked, Recorded, StepModel},
    runner::PropertyViolation,
    specification::{convert::ToSchema, domain::Snapshot},
    tree::Leaves,
};
use bombadil_schema::Time;
use bombadil_schema::terminal::{
    self as schema, TerminalAttributes, TerminalCell, TerminalColor,
    TerminalGrid, TerminalStateSummary, TerminalStyle, TerminalTraceEntry,
    TerminalUnderline,
};
use bombadil_trace::{StateReader, TraceDriver, TraceFileWriter, open_trace};

use crate::{
    driver::{TerminalAction, TerminalActionTemplate, TerminalSession},
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
        snapshots: &[Snapshot],
        violations: &[PropertyViolation],
        actions_available: &Leaves<'_, TerminalActionTemplate>,
        action_previous: Option<&Picked<TerminalAction>>,
    ) -> Result<()> {
        let last_action = action_previous.map(|picked| &picked.action);
        let model = StepModel::new(
            actions_available,
            action_previous.map(|picked| picked.template),
        );
        let entry = TerminalTraceEntry {
            timestamp: Time::from_system_time(state.timestamp),
            action: last_action.map(ToSchema::to_schema),
            state: TerminalStateSummary {
                scroll_offset: state.scroll_offset,
                cursor: state.cursor.clone(),
                exit_status: state.exit_status.clone(),
                actions_available: Some(model.actions_available),
                action_taken: model.action_taken,
            },
            snapshots: snapshots.iter().map(|s| s.to_schema()).collect(),
            violations: violations.iter().map(|v| v.to_schema()).collect(),
        };
        self.0.write_state(
            entry,
            Some(Arc::new(serde_json::to_vec(&state.grid)?)),
        )
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
            trace.actions.push(Recorded {
                action,
                template: entry.state.action_taken,
            });
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
/// line up. Trailing spaces are trimmed from each line. Its colors and
/// styles are ANSI escape sequences if `color` is set. The text is the terminal's, so
/// callers showing it must escape it.
pub fn render_screen(snapshot: &[u8], color: bool) -> Result<Vec<String>> {
    let grid: TerminalGrid = serde_json::from_slice(snapshot)
        .context("failed to decode terminal viewport")?;
    if (&grid).into_iter().count() != grid.size.cell_count() as usize {
        bail!("invalid terminal viewport cell count");
    }
    (0..grid.size.rows)
        .map(|y| {
            let plain = row_text(&grid, y);
            if !color {
                return Ok(plain.trim_end_matches(' ').to_string());
            }
            let mut line = String::new();
            let mut current = TerminalStyle::default();
            let mut remaining = plain.trim_end_matches(' ').len();
            for x in 0..grid.size.columns {
                if remaining == 0 {
                    break;
                }
                let (text, style) = match &grid[(y, x)] {
                    TerminalCell::Occupied {
                        contents, style, ..
                    } => (contents.to_string(), style),
                    TerminalCell::Empty { style } => (" ".to_string(), style),
                    TerminalCell::Continuation { .. } => continue,
                };
                let text = &text[..text.len().min(remaining)];
                if *style != current {
                    line.push_str("\x1b[0m");
                    write_style(&mut line, style)?;
                    current = style.clone();
                }
                line.push_str(text);
                remaining -= text.len();
            }
            if current != TerminalStyle::default() {
                line.push_str("\x1b[0m");
            }
            Ok(line)
        })
        .collect()
}

fn write_style(out: &mut String, style: &TerminalStyle) -> std::fmt::Result {
    for (attribute, code) in [
        (TerminalAttributes::BOLD, 1),
        (TerminalAttributes::DIM, 2),
        (TerminalAttributes::ITALIC, 3),
        (TerminalAttributes::BLINK, 5),
        (TerminalAttributes::INVERSE, 7),
        (TerminalAttributes::INVISIBLE, 8),
        (TerminalAttributes::STRIKETHROUGH, 9),
        (TerminalAttributes::OVERLINE, 53),
    ] {
        if style.attributes.contains(attribute) {
            write!(out, "\x1b[{code}m")?;
        }
    }
    let underline = match style.underline {
        TerminalUnderline::None => 0,
        TerminalUnderline::Single => 1,
        TerminalUnderline::Double => 2,
        TerminalUnderline::Curly => 3,
        TerminalUnderline::Dotted => 4,
        TerminalUnderline::Dashed => 5,
    };
    if underline != 0 {
        write!(out, "\x1b[4:{underline}m")?;
    }
    for (color, code) in [
        (&style.foreground_color, 38),
        (&style.background_color, 48),
        (&style.underline_color, 58),
    ] {
        match color {
            TerminalColor::None => {}
            TerminalColor::Palette(index) => {
                write!(out, "\x1b[{code};5;{index}m")?
            }
            TerminalColor::RGB { r, g, b } => {
                write!(out, "\x1b[{code};2;{r};{g};{b}m")?
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use crate::driver::{TerminalAction, viewport_grid};
    use crate::{TerminalStrategy, TerminalTestMode};
    use bombadil::model::action_key;
    use bombadil::specification::convert::ToInternal;
    use bombadil::specification::generators::StringGenerator;
    use bombadil::tree::Tree;
    use bombadil_schema::terminal::{
        TerminalColor, TerminalCursor, TerminalCursorPosition,
        TerminalCursorVisualStyle, TerminalGrid, TerminalSize,
    };
    use bombadil_trace::{tag, write_envelope, write_record};
    use libghostty_vt::{
        RenderState,
        render::{CellIterator, RowIterator},
        terminal::ScrollViewport,
    };
    use rand::{SeedableRng, rngs::StdRng};

    use super::*;

    fn state(payload: Option<Vec<u8>>) -> TerminalState {
        let size = TerminalSize {
            columns: 10,
            rows: 2,
        };
        TerminalState {
            timestamp: SystemTime::now(),
            grid: payload
                .map(|bytes| serde_json::from_slice(&bytes).unwrap())
                .unwrap_or_else(|| TerminalGrid::with_size(size)),
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
        }
    }

    fn snapshot(input: &[u8]) -> Vec<u8> {
        let mut terminal = libghostty_vt::Terminal::new(10, 2).unwrap();
        terminal.vt_write(input);
        capture(&terminal)
    }

    fn capture(terminal: &libghostty_vt::Terminal<'_, '_>) -> Vec<u8> {
        let mut render = RenderState::new().unwrap();
        let snapshot = render.update(terminal).unwrap();
        let grid = viewport_grid(
            &snapshot,
            &mut RowIterator::new().unwrap(),
            &mut CellIterator::new().unwrap(),
            TerminalSize {
                columns: terminal.cols().unwrap(),
                rows: terminal.rows().unwrap(),
            },
        )
        .unwrap();
        serde_json::to_vec(&grid).unwrap()
    }

    #[test]
    fn test_trace_round_trip() {
        let snapshot = snapshot(b"hello\x1b[3");
        let directory = tempfile::tempdir().unwrap();
        let action = TerminalAction::TypeText {
            text: "hi".to_string(),
        };
        let type_text: TerminalActionTemplate = TerminalAction::TypeText {
            text: StringGenerator::Text { length: 1..=8 },
        };
        let scroll_up: TerminalActionTemplate = TerminalAction::ScrollUp {};
        let type_text_key = action_key(&type_text);
        let scroll_up_key = action_key(&scroll_up);
        let mut writer = TerminalTraceWriter::initialize(
            &directory.path().join(TRACE_FILE_NAME),
            false,
        )
        .unwrap();
        let tree = Tree::Branch {
            branches: vec![
                (1, Tree::Leaf { value: type_text }),
                (1, Tree::Leaf { value: scroll_up }),
            ],
        };
        writer
            .write(
                &state(Some(snapshot.clone())),
                &[],
                &[],
                &tree.leaves(),
                Some(&Picked {
                    action: action.clone(),
                    template: type_text_key,
                }),
            )
            .unwrap();
        writer
            .write(&state(None), &[], &[], &tree.leaves(), None)
            .unwrap();
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
        assert_eq!(
            states[0].metadata.state.actions_available,
            Some(vec![(type_text_key, 0.5), (scroll_up_key, 0.5)])
        );
        assert_eq!(states[0].metadata.state.action_taken, Some(type_text_key));
        assert_eq!(states[1].metadata.state.action_taken, None);
        assert_eq!(
            render_screen(states[1].payload.as_ref().unwrap(), false).unwrap(),
            ["", ""]
        );

        let restored_snapshot = states[0].payload.as_ref().unwrap();
        assert_eq!(restored_snapshot, &snapshot);
        assert_eq!(
            render_screen(restored_snapshot, false).unwrap(),
            ["hello", ""]
        );
        let restored: TerminalGrid =
            serde_json::from_slice(restored_snapshot).unwrap();
        assert_eq!(restored, state(Some(snapshot)).grid);
    }

    /// Written, loaded and reproduced: the action keeps its template,
    /// though an earlier one accepts it too.
    #[test]
    fn test_reproduced_action_keeps_its_recorded_template() {
        let text = |length| -> TerminalActionTemplate {
            TerminalAction::TypeText {
                text: StringGenerator::Text { length },
            }
        };
        let (wide, narrow) = (text(1..=8), text(2..=4));
        let picked = Picked {
            action: TerminalAction::TypeText {
                text: "hi".to_string(),
            },
            template: action_key(&narrow),
        };
        let tree = Tree::Branch {
            branches: vec![
                (1, Tree::Leaf { value: wide }),
                (1, Tree::Leaf { value: narrow }),
            ],
        };
        let leaves = tree.leaves();

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(TRACE_FILE_NAME);
        let mut writer = TerminalTraceWriter::initialize(&path, false).unwrap();
        writer.write(&state(None), &[], &[], &leaves, None).unwrap();
        writer
            .write(&state(None), &[], &[], &leaves, Some(&picked))
            .unwrap();
        writer.finish().unwrap();

        let actions = load_trace_actions(&path)
            .unwrap()
            .map_actions(|action| action.to_internal())
            .actions;
        let mut strategy = TerminalStrategy {
            rng: StdRng::seed_from_u64(0),
            mode: TerminalTestMode::Reproduce(actions.into()),
            test_start: None,
            violations_count: 0,
            exit_on_violation: false,
            deadline: None,
            states_seen: 0,
            quiet: true,
        };
        assert_eq!(strategy.pick_action(&leaves).unwrap(), picked);
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
        assert_eq!(render_screen(&snapshot, false).unwrap(), ["hello", "red"]);
        let colored = render_screen(&snapshot, true).unwrap();
        assert_eq!(colored.len(), 2);
        assert!(
            colored[1].contains("red") && colored[1].contains("\x1b["),
            "{colored:?}"
        );
    }

    #[test]
    fn test_render_screen_preserves_cell_styles_and_wide_graphemes() {
        let mut terminal = libghostty_vt::Terminal::new(24, 2).unwrap();
        terminal.vt_write(
            "\x1b[1;2;3;5;7;8;9;53;4:3;38;2;11;22;33;48;5;42;58;2;44;55;66mA界e\u{301} B\x1b[0mZ\r\nC\x1b[48;2;7;8;9m \x1b[0mD".as_bytes(),
        );
        let payload = capture(&terminal);
        assert_eq!(
            render_screen(&payload, false).unwrap(),
            ["A界e\u{301} BZ", "C D"]
        );
        let rendered = render_screen(&payload, true).unwrap().join("\r\n");
        let mut restored = libghostty_vt::Terminal::new(24, 2).unwrap();
        restored.vt_write(rendered.as_bytes());
        let expected: TerminalGrid = serde_json::from_slice(&payload).unwrap();
        let actual: TerminalGrid =
            serde_json::from_slice(&capture(&restored)).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_trace_payload_does_not_grow_with_history() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(TRACE_FILE_NAME);
        let mut writer = TerminalTraceWriter::initialize(&path, false).unwrap();
        let tree = Tree::Leaf {
            value: TerminalActionTemplate::ScrollUp {},
        };
        let mut terminal = libghostty_vt::Terminal::new(10, 2).unwrap();
        terminal
            .set_scrollback_max_bytes(Some(usize::MAX))
            .unwrap()
            .set_scrollback_max_lines(Some(10_000))
            .unwrap();
        for _ in 0..100 {
            terminal.vt_write(b"\r\nhidden\r\nsame\r\nview");
            let mut captured = state(Some(capture(&terminal)));
            captured.scroll_offset =
                terminal.scrollbar().unwrap().offset as u32;
            writer
                .write(&captured, &[], &[], &tree.leaves(), None)
                .unwrap();
        }
        writer.finish().unwrap();
        let states = open_terminal_trace(&path)
            .unwrap()
            .collect::<Result<Vec<_>>>()
            .unwrap();
        assert_eq!(states.len(), 100);
        assert!(
            states.last().unwrap().metadata.state.scroll_offset
                > states[0].metadata.state.scroll_offset
        );
        let payload = states[0].payload.as_ref().unwrap();
        assert_eq!(render_screen(payload, false).unwrap(), ["same", "view"]);
        for state in &states {
            assert_eq!(state.payload.as_ref().unwrap(), payload);
        }
    }

    #[test]
    fn test_render_screen_rejects_invalid_payloads() {
        assert!(render_screen(b"GHOSTSNP", false).is_err());
        for color in [false, true] {
            assert!(
                render_screen(
                    br#"{"size":{"rows":2,"columns":10},"cells":[]}"#,
                    color
                )
                .is_err()
            );
        }
    }

    /// Marks on blank cells, which the properties see, are kept.
    #[test]
    fn test_render_screen_keeps_marks_on_blank_cells() {
        let snapshot = snapshot("B: \u{301}|\r\nD: \u{202e}|".as_bytes());
        let screen = ["B: \u{301}|", "D: \u{202e}|"];
        assert_eq!(render_screen(&snapshot, false).unwrap(), screen);
        let colored = render_screen(&snapshot, true).unwrap();
        assert_eq!(colored, screen);
    }

    #[test]
    fn test_render_screen_shows_the_viewport_only() {
        // Two rows of screen below two of scrollback.
        let snapshot = snapshot(b"one\r\ntwo\r\nthree\r\nfour");
        assert_eq!(render_screen(&snapshot, false).unwrap(), ["three", "four"]);
    }

    #[test]
    fn test_render_screen_keeps_scrolled_viewport() {
        let mut terminal = libghostty_vt::Terminal::new(10, 2).unwrap();
        terminal.vt_write(b"one\r\ntwo\r\nthree\r\nfour");
        terminal.scroll_viewport(ScrollViewport::Top);
        let snapshot = capture(&terminal);
        assert_eq!(render_screen(&snapshot, false).unwrap(), ["one", "two"]);
    }

    /// Every row of the viewport is a line, blank rows at the top and
    /// bottom included, with trailing spaces trimmed; soft-wrapped
    /// rows stay apart.
    #[test]
    fn test_render_screen_is_full_height() {
        let mut terminal = libghostty_vt::Terminal::new(6, 6).unwrap();
        terminal.vt_write(b"\r\n a  \r\n\r\nabcdefgh");
        let snapshot = capture(&terminal);
        let screen = ["", " a", "", "abcdef", "gh", ""];
        assert_eq!(render_screen(&snapshot, false).unwrap(), screen);
        assert_eq!(render_screen(&snapshot, true).unwrap(), screen);
        // Blank.
        let blank = self::snapshot(b"");
        assert_eq!(render_screen(&blank, false).unwrap(), ["", ""]);
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
