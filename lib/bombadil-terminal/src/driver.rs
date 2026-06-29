use std::cell::RefCell;
use std::ops::RangeInclusive;
use std::rc::Rc;
use std::sync::Arc;
use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Result, anyhow};
use bombadil::driver::{DriverEvent, InterfaceDriver};
use bombadil::specification::bundler::bundle;
use bombadil::specification::convert::{ToInternal, ToSchema};
use bombadil::specification::domain::Snapshot;
use bombadil::specification::generators::StringGenerator;
use bombadil::specification::verifier::{Specification, Verifier};
use bombadil_schema::terminal::{
    self, ProcessExitStatus, TerminalAttributes, TerminalCell, TerminalColor,
    TerminalCursor, TerminalCursorPosition, TerminalCursorVisualStyle,
    TerminalGrid, TerminalSize, TerminalStyle, TerminalUnderline,
};
use libghostty_vt::style as ghostty_style;
use libghostty_vt::{
    RenderState, Terminal, TerminalOptions,
    render::{
        CellIterator, CursorVisualStyle as GhosttyCursorVisualStyle,
        RowIterator, Snapshot as GhosttyRenderSnapshot,
    },
    screen::CellWide,
    terminal::ScrollViewport,
};
use serde::{Deserialize, Serialize};
use small_string::SmallString;

use crate::extractors::Extractors;
use crate::pty::{PtyOutput, PtyProcess, ReadResult};
use crate::state::TerminalState;

const INITIATE_STARTUP_DELAY: Duration = Duration::from_millis(1000);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum TerminalAction<U16 = u16, Text = String> {
    TypeText { text: Text },
    Resize { size: TerminalSize<U16> },
    Click { row: U16, column: U16 },
    ScrollUp {},
    ScrollDown {},
}

pub type TerminalActionTemplate =
    TerminalAction<RangeInclusive<u16>, StringGenerator>;

impl TerminalActionTemplate {
    pub fn generate<Rng: rand::TryRng + rand::RngExt>(
        &self,
        rng: &mut Rng,
    ) -> TerminalAction {
        match self {
            TerminalAction::TypeText { text } => TerminalAction::TypeText {
                text: text.generate(rng),
            },
            TerminalAction::Resize { size } => TerminalAction::Resize {
                size: TerminalSize {
                    rows: rng.random_range(size.rows.clone()),
                    columns: rng.random_range(size.columns.clone()),
                },
            },
            TerminalAction::ScrollUp {} => TerminalAction::ScrollUp {},
            TerminalAction::ScrollDown {} => TerminalAction::ScrollDown {},
            TerminalAction::Click { row, column } => TerminalAction::Click {
                row: rng.random_range(row.clone()),
                column: rng.random_range(column.clone()),
            },
        }
    }

    pub fn accepts(&self, original: &TerminalAction) -> bool {
        match (self, original) {
            (
                TerminalAction::TypeText {
                    text: text_template,
                },
                TerminalAction::TypeText {
                    text: text_original,
                },
            ) => text_template.accepts(text_original),
            (
                TerminalAction::Resize {
                    size: size_template,
                },
                TerminalAction::Resize {
                    size: size_original,
                },
            ) => {
                size_template.rows.contains(&size_original.rows)
                    && size_template.columns.contains(&size_original.columns)
            }
            (
                TerminalAction::Click {
                    row: row_template,
                    column: column_template,
                },
                TerminalAction::Click {
                    row: row_original,
                    column: column_original,
                },
            ) => {
                row_template.contains(row_original)
                    && column_template.contains(column_original)
            }
            (TerminalAction::ScrollUp {}, TerminalAction::ScrollUp {}) => true,
            (TerminalAction::ScrollDown {}, TerminalAction::ScrollDown {}) => {
                true
            }
            _ => false,
        }
    }
}

impl ToSchema<terminal::TerminalAction> for TerminalAction {
    fn to_schema(&self) -> terminal::TerminalAction {
        match self {
            TerminalAction::TypeText { text } => {
                terminal::TerminalAction::TypeText { text: text.clone() }
            }
            TerminalAction::Resize { size } => {
                terminal::TerminalAction::Resize { size: *size }
            }
            TerminalAction::ScrollUp {} => {
                terminal::TerminalAction::ScrollUp {}
            }
            TerminalAction::ScrollDown {} => {
                terminal::TerminalAction::ScrollDown {}
            }
            TerminalAction::Click { row, column } => {
                terminal::TerminalAction::Click {
                    row: *row,
                    column: *column,
                }
            }
        }
    }
}

impl ToInternal<TerminalAction> for terminal::TerminalAction {
    fn to_internal(&self) -> TerminalAction {
        match self {
            terminal::TerminalAction::TypeText { text } => {
                TerminalAction::TypeText { text: text.clone() }
            }
            terminal::TerminalAction::Resize { size } => {
                TerminalAction::Resize { size: *size }
            }
            terminal::TerminalAction::ScrollUp {} => {
                TerminalAction::ScrollUp {}
            }
            terminal::TerminalAction::ScrollDown {} => {
                TerminalAction::ScrollDown {}
            }
            terminal::TerminalAction::Click { row, column } => {
                TerminalAction::Click {
                    row: *row,
                    column: *column,
                }
            }
        }
    }
}

pub struct TerminalDriver {
    extractor: Extractors,
    terminal: Terminal<'static, 'static>,
    process: Rc<RefCell<PtyProcess>>,
    output: PtyOutput,
    size: TerminalSize,
    quiescence_timeout: Duration,
    last_action: Option<TerminalAction>,
    // Reused across frames: the ghostty render API is stateful and
    // optimized for repeated updates (dirty-region tracking), so these
    // are created once instead of per extracted state.
    render_state: RenderState<'static>,
    row_iterator: RowIterator<'static>,
    cell_iterator: CellIterator<'static>,
}

impl TerminalDriver {
    #[hotpath::measure]
    pub fn launch(
        specification: Specification,
        size: TerminalSize,
        scrollback_lines_max: usize,
        quiescence_timeout: Duration,
        program: &str,
        arguments: &[String],
    ) -> Result<(Self, Verifier)> {
        let bundle_code = bundle(".", &specification.module_specifier)
            .map_err(|e| anyhow!("bundle failed: {e}"))?;

        let extractor = Extractors::initialize(&bundle_code)?;
        let verifier = Verifier::new(&bundle_code)?;

        let program = program.to_string();
        let arguments = arguments.to_vec();

        let mut terminal = Terminal::new(TerminalOptions {
            cols: size.columns,
            rows: size.rows,
            max_scrollback: scrollback_lines_max,
        })?;

        let (process, output) = PtyProcess::spawn(size, &program, &arguments)?;
        let process = Rc::new(RefCell::new(process));

        let callback_process = process.clone();
        terminal.on_pty_write(move |_, data| {
            let mut process = callback_process.borrow_mut();
            process.write(data);
        })?;

        Ok((
            Self {
                extractor,
                terminal,
                process,
                output,
                size,
                quiescence_timeout,
                last_action: None,
                render_state: RenderState::new()?,
                row_iterator: RowIterator::new()?,
                cell_iterator: CellIterator::new()?,
            },
            verifier,
        ))
    }

    #[hotpath::measure]
    fn drain_output(&mut self, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        while let ReadResult::Chunk(data) = self.output.try_read()
            && Instant::now() < deadline
        {
            self.terminal.vt_write(&data);
        }
    }

    #[hotpath::measure]
    fn extract_state(&mut self) -> Result<TerminalState> {
        let snapshot = self.render_state.update(&self.terminal)?;
        let cursor = cursor_from_libghostty(&self.terminal, &snapshot)?;
        let mut row_iter = self.row_iterator.update(&snapshot)?;

        let mut cells = Vec::with_capacity(
            usize::from(self.size.columns) * usize::from(self.size.rows),
        );
        while let Some(row) = row_iter.next() {
            let mut cell_iter = self.cell_iterator.update(row)?;
            while let Some(cell) = cell_iter.next() {
                let style = style_from_ghostty(&cell.style()?);
                cells.push(match cell.raw_cell()?.wide()? {
                    // Trailing column of a wide character.
                    CellWide::SpacerTail => {
                        TerminalCell::Continuation { style }
                    }
                    // Right-margin placeholder left behind when a wide
                    // character does not fit and soft-wraps to the next line;
                    // nothing renders here.
                    CellWide::SpacerHead => TerminalCell::Empty { style },
                    kind => {
                        let length = cell.graphemes_len()?;
                        if length == 0 {
                            TerminalCell::Empty { style }
                        } else {
                            let mut contents =
                                SmallString::null_with_size(length);
                            cell.graphemes_buf(&mut contents[0..length])?;
                            TerminalCell::Occupied {
                                contents,
                                wide: kind == CellWide::Wide,
                                style,
                            }
                        }
                    }
                });
            }
        }
        let grid = TerminalGrid::from_cells(self.size, cells);

        let scroll_offset = self
            .terminal
            .scrollbar()
            .map(|s| s.offset as u32)
            .unwrap_or(0);

        Ok(TerminalState {
            timestamp: SystemTime::now(),
            grid,
            scrollback: TerminalGrid::with_size(TerminalSize {
                rows: 0,
                ..self.size
            }),
            scroll_offset,
            cursor,
            exit_status: self.process.borrow_mut().exit_status()?.map(
                |status| ProcessExitStatus {
                    signal: status.signal().map(ToString::to_string),
                    code: status.exit_code(),
                },
            ),
            last_action: self.last_action.clone(),
        })
    }
}

impl InterfaceDriver for TerminalDriver {
    type Action = TerminalAction;
    type ActionTemplate = TerminalActionTemplate;
    type State = TerminalState;

    #[hotpath::measure]
    fn initiate(&mut self) -> Result<()> {
        sleep(INITIATE_STARTUP_DELAY);
        Ok(())
    }

    fn terminate(self) -> Result<()> {
        self.process.borrow_mut().kill();
        Ok(())
    }

    #[hotpath::measure]
    fn next_event(&mut self) -> Option<DriverEvent<TerminalState>> {
        match self.output.try_read() {
            ReadResult::Chunk(data) => {
                assert!(!data.is_empty(), "chunk is empty");
                self.terminal.vt_write(&data);
                self.drain_output(self.quiescence_timeout);
            }
            ReadResult::Empty => {}
            ReadResult::Ended => {}
        }

        match self.extract_state() {
            Ok(state) => Some(DriverEvent::StateChanged(state)),
            Err(error) => Some(DriverEvent::Error(Arc::new(error))),
        }
    }

    #[hotpath::measure]
    fn apply(&mut self, action: TerminalAction) -> Result<()> {
        match &action {
            TerminalAction::TypeText { text } => {
                self.process.borrow_mut().write(text.as_bytes());
            }
            TerminalAction::Resize { size } => {
                self.size = *size;
                self.terminal.resize(size.columns, size.rows, 0, 0)?;
                self.process.borrow_mut().resize(*size)?;
            }
            TerminalAction::ScrollUp {} => {
                self.terminal.scroll_viewport(ScrollViewport::Top);
            }
            TerminalAction::ScrollDown {} => {
                self.terminal.scroll_viewport(ScrollViewport::Bottom);
            }
            TerminalAction::Click { row, column } => {
                self.process.borrow_mut().write(
                    &format!(
                        "\x1b[<0;{};{}M\x1b[<0;{};{}m",
                        column + 1,
                        row + 1,
                        column + 1,
                        row + 1,
                    )
                    .into_bytes(),
                );
            }
        }
        self.last_action = Some(action);
        Ok(())
    }

    fn extract_snapshots(
        &mut self,
        state: Arc<TerminalState>,
        _last_action: Option<&TerminalAction>,
    ) -> Result<Vec<Snapshot>> {
        self.extractor.run_extractors(state)
    }

    fn state_timestamp(state: &TerminalState) -> SystemTime {
        state.timestamp
    }
}

fn cursor_from_libghostty(
    terminal: &Terminal<'_, '_>,
    snapshot: &GhosttyRenderSnapshot<'_, '_>,
) -> Result<TerminalCursor> {
    Ok(TerminalCursor {
        position: TerminalCursorPosition {
            column: terminal.cursor_x()?,
            row: terminal.cursor_y()?,
        },
        visible: snapshot.cursor_visible()?,
        blinking: snapshot.cursor_blinking()?,
        visual_style: terminal_cursor_visual_style_from_libghostty(
            snapshot.cursor_visual_style()?,
        ),
        color: snapshot.cursor_color()?.map_or(
            TerminalColor::None,
            |ghostty_style::RgbColor { r, g, b }| TerminalColor::RGB {
                r,
                g,
                b,
            },
        ),
    })
}

#[hotpath::measure]
fn style_from_ghostty(value: &ghostty_style::Style) -> TerminalStyle {
    let mut result = TerminalStyle {
        foreground_color: color_from_ghostty(&value.fg_color),
        background_color: color_from_ghostty(&value.bg_color),
        underline_color: color_from_ghostty(&value.underline_color),
        underline: match value.underline {
            ghostty_style::Underline::None => TerminalUnderline::None,
            ghostty_style::Underline::Single => TerminalUnderline::Single,
            ghostty_style::Underline::Double => TerminalUnderline::Double,
            ghostty_style::Underline::Curly => TerminalUnderline::Curly,
            ghostty_style::Underline::Dotted => TerminalUnderline::Dotted,
            ghostty_style::Underline::Dashed => TerminalUnderline::Dashed,
            _ => {
                log::warn!("got unknown underline type from ghostty");
                TerminalUnderline::None
            }
        },
        ..TerminalStyle::default()
    };

    result.attributes.set(TerminalAttributes::BOLD, value.bold);
    result
        .attributes
        .set(TerminalAttributes::ITALIC, value.italic);
    result
        .attributes
        .set(TerminalAttributes::BLINK, value.blink);
    result
        .attributes
        .set(TerminalAttributes::INVERSE, value.inverse);
    result
        .attributes
        .set(TerminalAttributes::STRIKETHROUGH, value.strikethrough);
    result.attributes.set(TerminalAttributes::DIM, value.faint);
    result
        .attributes
        .set(TerminalAttributes::INVISIBLE, value.invisible);
    result
        .attributes
        .set(TerminalAttributes::OVERLINE, value.overline);

    result
}

fn color_from_ghostty(value: &ghostty_style::StyleColor) -> TerminalColor {
    match value {
        ghostty_style::StyleColor::None => TerminalColor::None,
        ghostty_style::StyleColor::Palette(ghostty_style::PaletteIndex(
            index,
        )) => TerminalColor::Palette(*index),
        ghostty_style::StyleColor::Rgb(ghostty_style::RgbColor { r, g, b }) => {
            TerminalColor::RGB {
                r: *r,
                g: *g,
                b: *b,
            }
        }
    }
}

fn terminal_cursor_visual_style_from_libghostty(
    value: GhosttyCursorVisualStyle,
) -> TerminalCursorVisualStyle {
    match value {
        GhosttyCursorVisualStyle::Bar => TerminalCursorVisualStyle::Bar,
        GhosttyCursorVisualStyle::Block => TerminalCursorVisualStyle::Block,
        GhosttyCursorVisualStyle::Underline => {
            TerminalCursorVisualStyle::Underline
        }
        GhosttyCursorVisualStyle::BlockHollow => {
            TerminalCursorVisualStyle::BlockHollow
        }
        _ => TerminalCursorVisualStyle::Unknown,
    }
}
