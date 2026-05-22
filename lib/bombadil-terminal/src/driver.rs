use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::{Result, anyhow, bail};
use bombadil::driver::{DriverEvent, FromGeneratedAction, InterfaceDriver};
use bombadil::specification::bundler::bundle;
use bombadil::specification::domain::Snapshot;
use bombadil::specification::verifier::Specification;
use bombadil::specification::worker::VerifierWorker;
use libghostty_vt::{
    RenderState, Terminal, TerminalOptions,
    render::{CellIterator, RowIterator},
    terminal::ScrollViewport,
};
use serde::{Deserialize, Serialize};
use serde_json as json;
use tokio::sync::{mpsc, oneshot};

use crate::extractors::ExtractorWorker;
use crate::pty::{PtyOutput, PtyProcess};
use crate::state::TerminalState;

const QUIESCENCE_IDLE: Duration = Duration::from_millis(1);
const TERMINAL_WORKER_STACK_SIZE: usize = 4 * 1024 * 1024;
const INITIATE_STARTUP_DELAY: Duration = Duration::from_millis(200);

#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Size {
    pub columns: u16,
    pub rows: u16,
}

impl Size {
    pub fn cell_count(&self) -> u32 {
        self.columns as u32 * self.rows as u32
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum TerminalAction {
    #[serde(rename_all = "camelCase")]
    TypeText {
        text: String,
    },
    #[serde(rename_all = "camelCase")]
    PressKey {
        code: u32,
    },
    #[serde(rename_all = "camelCase")]
    Resize {
        size: Size,
    },
    ScrollUp {},
    ScrollDown {},
}

impl FromGeneratedAction for TerminalAction {
    fn from_generated(value: json::Value) -> Result<Self> {
        Ok(json::from_value(value)?)
    }
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
    size: Size,
    last_action: Option<TerminalAction>,
}

impl TerminalWorkerState {
    fn drain_output(&mut self) {
        while let Some(data) = self.output.try_read() {
            self.terminal.vt_write(&data.into_bytes());
        }
    }

    fn extract_state(&mut self, terminated: bool) -> Result<TerminalState> {
        let mut render_state = RenderState::new()?;
        let mut row_iter_state = RowIterator::new()?;
        let mut cell_iter_state = CellIterator::new()?;

        let snapshot = render_state.update(&self.terminal)?;
        let mut row_iter = row_iter_state.update(&snapshot)?;

        let mut rows = Vec::with_capacity(self.size.rows as usize);
        while let Some(row) = row_iter.next() {
            let mut cell_iter = cell_iter_state.update(row)?;
            let mut line =
                String::with_capacity(self.size.columns as usize * 2);
            while let Some(cell) = cell_iter.next() {
                let graphemes: Vec<char> = cell.graphemes()?;
                if graphemes.is_empty() {
                    line.push(' ');
                } else {
                    line.extend(graphemes);
                }
            }
            rows.push(line);
        }

        let scroll_offset = self
            .terminal
            .scrollbar()
            .map(|s| s.offset as u32)
            .unwrap_or(0);

        Ok(TerminalState {
            timestamp: SystemTime::now(),
            size: self.size,
            rows,
            scrollback: Vec::new(),
            scroll_offset,
            terminated,
            last_action: self.last_action.clone(),
        })
    }

    async fn next_event(&mut self) -> Option<DriverEvent<TerminalState>> {
        let mut got_eof = false;
        loop {
            match tokio::time::timeout(QUIESCENCE_IDLE, self.output.read())
                .await
            {
                Ok(Ok(Some(data))) => {
                    self.terminal.vt_write(&data.into_bytes());
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
    size: Size,
    max_scrollback: usize,
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
            max_scrollback,
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
    extractor: ExtractorWorker,
}

impl TerminalDriver {
    pub async fn launch(
        specification: Specification,
        size: Size,
        max_scrollback: usize,
        program: &str,
        arguments: &[String],
    ) -> Result<(Self, Arc<VerifierWorker>)> {
        let bundle_code = bundle(".", &specification.module_specifier)
            .await
            .map_err(|e| anyhow!("bundle failed: {e}"))?;

        let extractor = ExtractorWorker::start(bundle_code).await?;

        let verifier = VerifierWorker::start(specification).await?;

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
                    max_scrollback,
                    program,
                    arguments,
                    command_recv,
                    ready_send,
                );
            })?;

        ready_recv
            .await
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

    async fn initiate(&mut self) -> Result<()> {
        let (reply_send, reply_recv) = oneshot::channel();
        self.command_send
            .send(TerminalCommand::Initiate { reply: reply_send })
            .await?;
        reply_recv
            .await
            .map_err(|_| anyhow!("terminal worker gone"))?
    }

    async fn terminate(self) -> Result<()> {
        let (reply_send, reply_recv) = oneshot::channel();
        self.command_send
            .send(TerminalCommand::Terminate { reply: reply_send })
            .await?;
        reply_recv
            .await
            .map_err(|_| anyhow!("terminal worker gone"))?
    }

    async fn next_event(&mut self) -> Option<DriverEvent<TerminalState>> {
        let (reply_send, reply_recv) = oneshot::channel();
        if self
            .command_send
            .send(TerminalCommand::NextEvent { reply: reply_send })
            .await
            .is_err()
        {
            return None;
        }
        reply_recv.await.ok().flatten()
    }

    async fn apply(&mut self, action: TerminalAction) -> Result<()> {
        if let TerminalAction::PressKey { code } = &action
            && char::from_u32(*code).is_none()
        {
            bail!("PressKey: code {} is not valid unicode", code);
        }
        let (reply_send, reply_recv) = oneshot::channel();
        self.command_send
            .send(TerminalCommand::Apply {
                action,
                reply: reply_send,
            })
            .await?;
        reply_recv.await?
    }

    async fn extract_snapshots(
        &self,
        state: &TerminalState,
        _last_action: Option<&TerminalAction>,
    ) -> Result<Vec<Snapshot>> {
        self.extractor.run_extractors(state).await
    }

    fn state_timestamp(state: &TerminalState) -> SystemTime {
        state.timestamp
    }
}
