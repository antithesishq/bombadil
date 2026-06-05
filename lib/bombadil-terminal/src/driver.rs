use std::sync::Arc;
use std::time::{Duration, SystemTime};

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
    terminal::ScrollViewport,
};
use serde::{Deserialize, Serialize};
use small_string::SmallString;
use tokio::sync::{mpsc, oneshot};
use unicode_width::UnicodeWidthChar;

use crate::extractors::Extractors;
use crate::pty::{PtyOutput, PtyProcess};
use crate::state::TerminalState;

const QUIESCENCE_IDLE: Duration = Duration::from_millis(1);
const TERMINAL_WORKER_STACK_SIZE: usize = 4 * 1024 * 1024;
const INITIATE_STARTUP_DELAY: Duration = Duration::from_millis(200);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum TerminalAction {
    TypeText { text: String },
    PressKey { code: u32 },
    Resize { size: TerminalSize },
    ScrollUp {},
    ScrollDown {},
}

enum TerminalCommand {
    Initiate {
        reply: oneshot::Sender<Result<()>>,
    },
    NextEvent {
        reply: oneshot::Sender<Option<DriverEvent<TerminalState>>>,
    },
    Apply {
        action: TerminalAction,
        reply: oneshot::Sender<anyhow::Result<()>>,
    },
    Terminate {
        reply: oneshot::Sender<Result<()>>,
    },
}

struct TerminalWorkerState {
    terminal: Terminal<'static, 'static>,
    process: PtyProcess,
    output: PtyOutput,
    size: TerminalSize,
    last_action: Option<TerminalAction>,
}

impl TerminalWorkerState {
    #[hotpath::measure]
    fn drain_output(&mut self) {
        while let Some(data) = self.output.try_read() {
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

        let mut grid = TerminalGrid::with_size(self.size);
        let mut row_index = 0;
        while let Some(row) = row_iter.next() {
            let mut cell_iter = cell_iter_state.update(row)?;
            let mut column_index = 0;
            while let Some(cell) = cell_iter.next() {
                let mut contents =
                    SmallString::null_with_size(cell.graphemes_len()?);
                cell.graphemes_buf(&mut contents[0..cell.graphemes_len()?])?;
                if contents.contains(&'\u{FFFD}') {
                    eprintln!(
                        "replacement char at ({}, {}): {:?}",
                        row_index, column_index, contents
                    );
                }
                let wide = contents
                    .iter()
                    .map(|c| c.width().unwrap_or(0))
                    .sum::<usize>()
                    == 2usize;

                let style = style_from_ghostty(&cell.style()?);
                grid[(row_index, column_index)] = if contents.is_empty()
                    && style == TerminalStyle::default()
                {
                    TerminalCell::Empty
                } else {
                    TerminalCell::Occupied {
                        contents,
                        wide,
                        style,
                    }
                };
                column_index += 1;

                if wide {
                    cell_iter.next(); // ignored and handled directly
                    grid[(row_index, column_index)] =
                        TerminalCell::Continuation;
                    column_index += 1;
                }
            }
            row_index += 1;
        }

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

    #[hotpath::measure]
    async fn next_event(&mut self) -> Option<DriverEvent<TerminalState>> {
        let mut got_eof = false;
        loop {
            match tokio::time::timeout(QUIESCENCE_IDLE, self.output.read())
                .await
            {
                Ok(Ok(Some(data))) => {
                    self.terminal.vt_write(&data);
                    self.drain_output();
                }
                Ok(Ok(None)) => {
                    got_eof = true;
                    break;
                }
                Ok(Err(error)) => {
                    return Some(DriverEvent::Error(Arc::new(error)));
                }
                Err(_) => break,
            }
        }

        let terminated =
            got_eof || matches!(self.process.is_terminated(), Ok(true));
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
}

// This needs to be single-threaded (but async) due to !Send resources.
fn run_terminal_worker(
    size: TerminalSize,
    scrollback_lines_max: usize,
    program: String,
    args: Vec<String>,
    mut command_receive: mpsc::Receiver<TerminalCommand>,
    ready_send: oneshot::Sender<Result<()>>,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(error) => {
            let _ = ready_send.send(Err(error.into()));
            return;
        }
    };

    runtime.block_on(async move {
        let terminal = match Terminal::new(TerminalOptions {
            cols: size.columns,
            rows: size.rows,
            max_scrollback: scrollback_lines_max,
        }) {
            Ok(t) => t,
            Err(error) => {
                let _ = ready_send.send(Err(error.into()));
                return;
            }
        };

        let (process, output) =
            match PtyProcess::spawn(size, &program, &args).await {
                Ok(x) => x,
                Err(error) => {
                    let _ = ready_send.send(Err(error));
                    return;
                }
            };

        if ready_send.send(Ok(())).is_err() {
            // Driver was dropped before we finished setup.
            return;
        }

        let mut state = TerminalWorkerState {
            terminal,
            process,
            output,
            size,
            last_action: None,
        };

        while let Some(command) = command_receive.recv().await {
            match command {
                TerminalCommand::Initiate { reply } => {
                    tokio::time::sleep(INITIATE_STARTUP_DELAY).await;
                    let _ = reply.send(Ok(()));
                }
                TerminalCommand::NextEvent { reply } => {
                    let event = state.next_event().await;
                    let _ = reply.send(event);
                }
                TerminalCommand::Apply { action, reply } => {
                    let result = state.apply(action);
                    let _ = reply.send(result);
                }
                TerminalCommand::Terminate { reply } => {
                    state.process.kill().await;
                    let _ = reply.send(Ok(()));
                    break;
                }
            }
        }
    });
}

pub struct TerminalDriver {
    command_send: mpsc::Sender<TerminalCommand>,
    extractor: Extractors,
}

impl TerminalDriver {
    #[hotpath::measure]
    pub fn launch(
        specification: Specification,
        size: TerminalSize,
        scrollback_lines_max: usize,
        program: &str,
        arguments: &[String],
    ) -> Result<(Self, Verifier)> {
        let bundle_code = bundle(".", &specification.module_specifier)
            .map_err(|e| anyhow!("bundle failed: {e}"))?;

        let extractor = Extractors::initialize(&bundle_code)?;
        let verifier = Verifier::new(&bundle_code)?;

        let (command_send, command_recv) = mpsc::channel(256);
        let (ready_send, ready_recv) = oneshot::channel();
        let program = program.to_string();
        let arguments = arguments.to_vec();

        std::thread::Builder::new()
            .name("bombadil-terminal-worker".to_string())
            .stack_size(TERMINAL_WORKER_STACK_SIZE)
            .spawn(move || {
                run_terminal_worker(
                    size,
                    scrollback_lines_max,
                    program,
                    arguments,
                    command_recv,
                    ready_send,
                );
            })?;

        ready_recv
            .blocking_recv()
            .map_err(|_| anyhow!("terminal worker died before ready"))??;

        Ok((
            Self {
                command_send,
                extractor,
            },
            verifier,
        ))
    }
}

impl InterfaceDriver for TerminalDriver {
    type Action = TerminalAction;
    type State = TerminalState;

    #[hotpath::measure]
    fn initiate(&mut self) -> Result<()> {
        let (reply_send, reply_recv) = oneshot::channel();
        self.command_send
            .blocking_send(TerminalCommand::Initiate { reply: reply_send })?;
        reply_recv
            .blocking_recv()
            .map_err(|_| anyhow!("terminal worker gone"))?
    }

    fn terminate(self) -> Result<()> {
        let (reply_send, reply_recv) = oneshot::channel();
        self.command_send
            .blocking_send(TerminalCommand::Terminate { reply: reply_send })?;
        reply_recv
            .blocking_recv()
            .map_err(|_| anyhow!("terminal worker gone"))?
    }

    #[hotpath::measure]
    fn next_event(&mut self) -> Option<DriverEvent<TerminalState>> {
        let (reply_send, reply_recv) = oneshot::channel();
        if self
            .command_send
            .blocking_send(TerminalCommand::NextEvent { reply: reply_send })
            .is_err()
        {
            return None;
        }
        reply_recv.blocking_recv().ok().flatten()
    }

    #[hotpath::measure]
    fn apply(&mut self, action: TerminalAction) -> Result<()> {
        if let TerminalAction::PressKey { code } = &action
            && char::from_u32(*code).is_none()
        {
            bail!("PressKey: code {} is not valid unicode", code);
        }
        let (reply_send, reply_recv) = oneshot::channel();
        self.command_send.blocking_send(TerminalCommand::Apply {
            action,
            reply: reply_send,
        })?;
        reply_recv.blocking_recv()?
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
