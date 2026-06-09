use std::sync::Arc;
use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime};

use antithesis_sdk::random::AntithesisRng;
use anyhow::{Result, anyhow, bail};
use bombadil::driver::{DriverEvent, InterfaceDriver};
use bombadil::specification::bundler::bundle;
use bombadil::specification::domain::Snapshot;
use bombadil::specification::verifier::{Specification, Verifier};
use bombadil_schema::{
    TerminalAttributes, TerminalCell, TerminalColor, TerminalGrid,
    TerminalSize, TerminalStyle, TerminalUnderline,
};
use libghostty_vt::style as ghostty_style;
use libghostty_vt::{
    RenderState, Terminal, TerminalOptions,
    render::{CellIterator, RowIterator},
    screen::CellWide,
    terminal::ScrollViewport,
};
use serde::{Deserialize, Serialize};
use small_string::SmallString;

use crate::extractors::Extractors;
use crate::pty::{PtyOutput, PtyProcess, ReadResult};
use crate::state::TerminalState;

const INITIATE_STARTUP_DELAY: Duration = Duration::from_millis(200);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum TerminalAction {
    TypeText { text: String },
    PressKey { code: u32 },
    Resize { size: TerminalSize },
    ScrollUp {},
    ScrollDown {},
}

pub struct TerminalDriver {
    extractor: Extractors,
    terminal: Terminal<'static, 'static>,
    process: PtyProcess,
    output: PtyOutput,
    size: TerminalSize,
    quiescence_timeout: Duration,
    last_action: Option<TerminalAction>,
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

        let extractor = Extractors::initialize(&bundle_code, AntithesisRng)?;
        let verifier = Verifier::new(&bundle_code, AntithesisRng)?;

        let program = program.to_string();
        let arguments = arguments.to_vec();

        let terminal = Terminal::new(TerminalOptions {
            cols: size.columns,
            rows: size.rows,
            max_scrollback: scrollback_lines_max,
        })?;

        let (process, output) = PtyProcess::spawn(size, &program, &arguments)?;

        Ok((
            Self {
                extractor,
                terminal,
                process,
                output,
                size,
                quiescence_timeout,
                last_action: None,
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
    fn extract_state(&mut self, terminated: bool) -> Result<TerminalState> {
        let mut render_state = RenderState::new()?;
        let mut row_iter_state = RowIterator::new()?;
        let mut cell_iter_state = CellIterator::new()?;

        let snapshot = render_state.update(&self.terminal)?;
        let mut row_iter = row_iter_state.update(&snapshot)?;

        let mut cells = Vec::with_capacity(
            usize::from(self.size.columns) * usize::from(self.size.rows),
        );
        while let Some(row) = row_iter.next() {
            let mut cell_iter = cell_iter_state.update(row)?;
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
            terminated,
            last_action: self.last_action.clone(),
        })
    }
}

impl InterfaceDriver for TerminalDriver {
    type Action = TerminalAction;
    type State = TerminalState;

    #[hotpath::measure]
    fn initiate(&mut self) -> Result<()> {
        sleep(INITIATE_STARTUP_DELAY);
        Ok(())
    }

    fn terminate(mut self) -> Result<()> {
        self.process.kill();
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

        let terminated = matches!(self.process.is_terminated(), Ok(true));
        match self.extract_state(terminated) {
            Ok(state) => Some(DriverEvent::StateChanged(state)),
            Err(error) => Some(DriverEvent::Error(Arc::new(error))),
        }
    }

    #[hotpath::measure]
    fn apply(&mut self, action: TerminalAction) -> Result<()> {
        match &action {
            TerminalAction::TypeText { text } => {
                self.process.write(text.as_bytes());
            }
            TerminalAction::PressKey { code } => {
                if let Some(ch) = char::from_u32(*code) {
                    let mut buf = [0u8; 4];
                    self.process.write(ch.encode_utf8(&mut buf).as_bytes());
                } else {
                    bail!(
                        "PressKey: code {} is not a valid unicode scalar",
                        code
                    );
                }
            }
            TerminalAction::Resize { size } => {
                self.size = *size;
                self.terminal.resize(size.columns, size.rows, 0, 0)?;
                self.process.resize(*size)?;
            }
            TerminalAction::ScrollUp {} => {
                self.terminal.scroll_viewport(ScrollViewport::Top);
            }
            TerminalAction::ScrollDown {} => {
                self.terminal.scroll_viewport(ScrollViewport::Bottom);
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
