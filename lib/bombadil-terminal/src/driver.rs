use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::{Result, anyhow, bail};
use boa_engine::{
    Context, JsError, JsObject, JsString, JsValue, NativeFunction, Source,
    context::ContextBuilder, js_string,
};
use bombadil::driver::{DriverEvent, InterfaceDriver};
use bombadil::specification::bundler::bundle;
use bombadil::specification::domain::Snapshot;
use bombadil::specification::verifier::Specification;
use bombadil::specification::worker::VerifierWorker;
use bombadil_schema::Time;
use libghostty_vt::{
    RenderState, Terminal, TerminalOptions,
    render::{CellIterator, RowIterator},
    terminal::ScrollViewport,
};
use serde::{Deserialize, Serialize};
use serde_json as json;
use tokio::sync::{mpsc, oneshot};

use crate::pty::{PtyOutput, PtyProcess};

const QUIESCENCE_IDLE: Duration = Duration::from_millis(50);
const EXTRACTOR_STACK_SIZE: usize = 16 * 1024 * 1024;
const TERMINAL_WORKER_STACK_SIZE: usize = 4 * 1024 * 1024;
const RANDOM_BYTES_COUNT_MAX: usize = 4096;
const INITIATE_STARTUP_DELAY: Duration = Duration::from_millis(200);

/// JsError holds Boa GC pointers and is therefore neither Send nor Sync,
/// so it can't auto-convert into anyhow::Error. Stringify eagerly at
/// every Boa call site.
fn js_err(error: JsError) -> anyhow::Error {
    anyhow!("{}", error)
}

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

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalState {
    #[serde(skip)]
    pub timestamp: SystemTime,
    pub size: Size,
    pub rows: Vec<String>,
    pub scrollback: Vec<String>,
    pub scroll_offset: u32,
    pub finished: bool,
    pub last_action: Option<TerminalAction>,
}

/// A separate Boa context, on its own OS thread, that owns the bundled
/// specification and serves `runExtractors` calls for the terminal
/// driver. Kept distinct from the verifier's Boa: extractors and
/// property evaluation must not share state.
pub struct ExtractorWorker {
    tx: mpsc::Sender<ExtractorCommand>,
}

enum ExtractorCommand {
    RunExtractors {
        state_json: json::Value,
        reply: oneshot::Sender<Result<Vec<PartialSnapshot>>>,
    },
}

#[derive(Debug, Clone, Deserialize)]
struct PartialSnapshot {
    index: usize,
    name: Option<String>,
    value: json::Value,
}

impl ExtractorWorker {
    pub async fn start(
        bundle_code: String,
        runtime_module: String,
    ) -> Result<Self> {
        let (ready_tx, ready_rx) =
            oneshot::channel::<Result<(), anyhow::Error>>();
        let (tx, mut rx) = mpsc::channel::<ExtractorCommand>(32);

        std::thread::Builder::new()
            .stack_size(EXTRACTOR_STACK_SIZE)
            .spawn(move || {
                let mut state =
                    match init_context(&bundle_code, &runtime_module) {
                        Ok(state) => {
                            let _ = ready_tx.send(Ok(()));
                            state
                        }
                        Err(error) => {
                            let _ = ready_tx.send(Err(error));
                            return;
                        }
                    };
                while let Some(command) = rx.blocking_recv() {
                    match command {
                        ExtractorCommand::RunExtractors {
                            state_json,
                            reply,
                        } => {
                            let result = run_extractors(&mut state, state_json);
                            let _ = reply.send(result);
                        }
                    }
                }
            })?;

        ready_rx
            .await
            .map_err(|_| anyhow!("extractor worker died before ready"))??;
        Ok(Self { tx })
    }

    pub async fn run_extractors(
        &self,
        state: &TerminalState,
    ) -> Result<Vec<Snapshot>> {
        let time = Time::from_system_time(state.timestamp);
        let state_json = json::to_value(state)?;
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(ExtractorCommand::RunExtractors {
                state_json,
                reply: reply_tx,
            })
            .await
            .map_err(|_| anyhow!("extractor worker gone"))?;
        let partials = reply_rx
            .await
            .map_err(|_| anyhow!("extractor worker gone"))??;
        Ok(partials
            .into_iter()
            .map(|p| Snapshot {
                index: p.index,
                name: p.name,
                value: p.value,
                time,
            })
            .collect())
    }
}

struct ExtractorState {
    context: Context,
    runtime: JsObject,
}

fn init_context(
    bundle_code: &str,
    runtime_module: &str,
) -> Result<ExtractorState> {
    let mut context = ContextBuilder::default()
        .build()
        .map_err(|e| anyhow!("Boa build: {e}"))?;

    context
        .register_global_builtin_callable(
            js_string!("__bombadil_random_bytes"),
            1,
            NativeFunction::from_copy_closure(|_this, args, context| {
                let n = args
                    .first()
                    .map(|v| v.to_u32(context))
                    .transpose()?
                    .unwrap_or(0) as usize;
                let n = n.min(RANDOM_BYTES_COUNT_MAX);
                let mut buf = vec![0u8; n];
                rand::fill(&mut buf[..]);
                Ok(boa_engine::object::builtins::JsUint8Array::from_iter(
                    buf, context,
                )?
                .into())
            }),
        )
        .map_err(js_err)?;

    let console_obj = boa_engine::object::ObjectInitializer::new(&mut context)
        .function(
            NativeFunction::from_copy_closure(|_this, args, _context| {
                log::info!("{}", format_console_args(args));
                Ok(JsValue::undefined())
            }),
            js_string!("log"),
            0,
        )
        .function(
            NativeFunction::from_copy_closure(|_this, args, _context| {
                log::warn!("{}", format_console_args(args));
                Ok(JsValue::undefined())
            }),
            js_string!("warn"),
            0,
        )
        .function(
            NativeFunction::from_copy_closure(|_this, args, _context| {
                log::error!("{}", format_console_args(args));
                Ok(JsValue::undefined())
            }),
            js_string!("error"),
            0,
        )
        .build();
    context
        .register_global_property(
            js_string!("console"),
            console_obj,
            boa_engine::property::Attribute::all(),
        )
        .map_err(js_err)?;

    context
        .eval(Source::from_bytes(bundle_code))
        .map_err(|e| anyhow!("bundle eval failed: {e}"))?;

    let require_fn = context
        .global_object()
        .get(js_string!("__bombadilRequire"), &mut context)
        .map_err(js_err)?
        .as_callable()
        .ok_or(anyhow!("__bombadilRequire is not callable"))?;

    let module_value = require_fn
        .call(
            &JsValue::undefined(),
            &[JsValue::from(JsString::from(runtime_module))],
            &mut context,
        )
        .map_err(js_err)?;
    let module_obj = module_value
        .as_object()
        .ok_or(anyhow!("runtime module is not an object"))?
        .clone();
    let runtime = module_obj
        .get(js_string!("runtime"), &mut context)
        .map_err(js_err)?
        .as_object()
        .ok_or(anyhow!("runtime is not an object"))?
        .clone();

    Ok(ExtractorState { context, runtime })
}

fn format_console_args(args: &[JsValue]) -> String {
    args.iter()
        .map(|v| match v.as_string() {
            Some(s) => s.to_std_string_escaped(),
            None => v.display().to_string(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn run_extractors(
    state: &mut ExtractorState,
    state_json: json::Value,
) -> Result<Vec<PartialSnapshot>> {
    let state_value =
        JsValue::from_json(&state_json, &mut state.context).map_err(js_err)?;
    let run_extractors_fn = state
        .runtime
        .get(js_string!("runExtractors"), &mut state.context)
        .map_err(js_err)?
        .as_callable()
        .ok_or(anyhow!("runExtractors is not callable"))?;
    let result = run_extractors_fn
        .call(
            &JsValue::from(state.runtime.clone()),
            &[state_value],
            &mut state.context,
        )
        .map_err(js_err)?;
    let result_json = result
        .to_json(&mut state.context)
        .map_err(js_err)?
        .ok_or(anyhow!("runExtractors returned undefined"))?;
    let partials: Vec<PartialSnapshot> = json::from_value(result_json)?;
    Ok(partials)
}

/// Commands the runner sends across the thread boundary to the
/// TerminalWorker. All variants except [`Apply`] carry a oneshot reply
/// channel; `Apply` is fire-and-forget so it can be called from the
/// runner's sync `InterfaceDriver::apply`. Failures inside the worker's
/// apply path are stashed in `pending_error` and surfaced on the next
/// [`NextEvent`].
enum TerminalCommand {
    Initiate {
        reply: oneshot::Sender<Result<()>>,
    },
    NextEvent {
        reply: oneshot::Sender<Option<DriverEvent<TerminalState>>>,
    },
    Apply {
        action: TerminalAction,
    },
    Terminate {
        reply: oneshot::Sender<Result<()>>,
    },
}

/// All the !Send resources (libghostty Terminal, PTY) live here, owned
/// by the TerminalWorker thread.
struct TerminalWorkerState {
    terminal: Terminal<'static, 'static>,
    process: PtyProcess,
    output: PtyOutput,
    size: Size,
    last_action: Option<TerminalAction>,
    pending_error: Option<anyhow::Error>,
}

impl TerminalWorkerState {
    fn drain_output(&mut self) {
        while let Some(data) = self.output.try_read() {
            self.terminal.vt_write(&data.into_bytes());
        }
    }

    fn build_state(&mut self, finished: bool) -> Result<TerminalState> {
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
            finished,
            last_action: self.last_action.clone(),
        })
    }

    async fn next_event(&mut self) -> Option<DriverEvent<TerminalState>> {
        if let Some(error) = self.pending_error.take() {
            return Some(DriverEvent::Error(Arc::new(error)));
        }

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

        let finished =
            got_eof || matches!(self.process.is_finished(), Ok(true));
        match self.build_state(finished) {
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
                // The browser uses Web KeyCode integers and translates
                // them to CDP key events; for a PTY there's no such
                // table. For now interpret `code` as a Unicode scalar
                // value and send it as UTF-8. Specs that need exotic
                // sequences (arrow keys, function keys) should emit
                // TypeText with the appropriate ESC sequence.
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

/// Build the Terminal + PTY inside the worker thread's current_thread
/// runtime — both because libghostty's Terminal is !Send (the runner
/// thread can't own it) and because PtyProcess spawns a tokio task for
/// PTY reads, which must run on a live runtime.
fn run_terminal_worker(
    size: Size,
    max_scrollback: usize,
    program: String,
    args: Vec<String>,
    mut rx: mpsc::UnboundedReceiver<TerminalCommand>,
    ready_tx: oneshot::Sender<Result<()>>,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(error) => {
            let _ = ready_tx.send(Err(error.into()));
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
                let _ = ready_tx.send(Err(error.into()));
                return;
            }
        };

        let (process, output) =
            match PtyProcess::spawn(size, &program, &args).await {
                Ok(x) => x,
                Err(error) => {
                    let _ = ready_tx.send(Err(error));
                    return;
                }
            };

        if ready_tx.send(Ok(())).is_err() {
            // Driver was dropped before we finished setup.
            return;
        }

        let mut state = TerminalWorkerState {
            terminal,
            process,
            output,
            size,
            last_action: None,
            pending_error: None,
        };

        while let Some(command) = rx.recv().await {
            match command {
                TerminalCommand::Initiate { reply } => {
                    tokio::time::sleep(INITIATE_STARTUP_DELAY).await;
                    let _ = reply.send(Ok(()));
                }
                TerminalCommand::NextEvent { reply } => {
                    let event = state.next_event().await;
                    let _ = reply.send(event);
                }
                TerminalCommand::Apply { action } => {
                    if let Err(error) = state.apply(action) {
                        state.pending_error = Some(error);
                    }
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

/// Send handle that talks to a TerminalWorker thread. The driver itself
/// holds nothing !Send; the libghostty Terminal and PtyProcess live in
/// the worker, accessed via channels. The ExtractorWorker is a separate
/// thread that owns its own Boa context.
pub struct TerminalDriver {
    cmd_tx: mpsc::UnboundedSender<TerminalCommand>,
    extractor: ExtractorWorker,
}

impl TerminalDriver {
    pub async fn launch(
        specification: Specification,
        size: Size,
        max_scrollback: usize,
        program: &str,
        args: &[String],
    ) -> Result<(Self, Arc<VerifierWorker>)> {
        let bundle_code = bundle(".", &specification.module_specifier)
            .await
            .map_err(|e| anyhow!("bundle failed: {e}"))?;
        let runtime_module = specification.runtime_module.clone();

        let extractor =
            ExtractorWorker::start(bundle_code, runtime_module).await?;

        let verifier = VerifierWorker::start(specification).await?;

        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (ready_tx, ready_rx) = oneshot::channel();
        let program_owned = program.to_string();
        let args_owned = args.to_vec();

        std::thread::Builder::new()
            .name("bombadil-terminal-worker".to_string())
            .stack_size(TERMINAL_WORKER_STACK_SIZE)
            .spawn(move || {
                run_terminal_worker(
                    size,
                    max_scrollback,
                    program_owned,
                    args_owned,
                    cmd_rx,
                    ready_tx,
                );
            })?;

        ready_rx
            .await
            .map_err(|_| anyhow!("terminal worker died before ready"))??;

        Ok((Self { cmd_tx, extractor }, verifier))
    }
}

impl InterfaceDriver for TerminalDriver {
    type Action = TerminalAction;
    type JsAction = TerminalAction;
    type State = TerminalState;

    async fn initiate(&mut self) -> Result<()> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.cmd_tx
            .send(TerminalCommand::Initiate { reply: reply_tx })
            .map_err(|_| anyhow!("terminal worker gone"))?;
        reply_rx
            .await
            .map_err(|_| anyhow!("terminal worker gone"))?
    }

    async fn terminate(self) -> Result<()> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.cmd_tx
            .send(TerminalCommand::Terminate { reply: reply_tx })
            .map_err(|_| anyhow!("terminal worker gone"))?;
        reply_rx
            .await
            .map_err(|_| anyhow!("terminal worker gone"))?
    }

    async fn next_event(&mut self) -> Option<DriverEvent<TerminalState>> {
        let (reply_tx, reply_rx) = oneshot::channel();
        if self
            .cmd_tx
            .send(TerminalCommand::NextEvent { reply: reply_tx })
            .is_err()
        {
            return None;
        }
        reply_rx.await.ok().flatten()
    }

    fn apply(&mut self, action: TerminalAction) -> Result<()> {
        // Validate Unicode scalar locally so PressKey errors propagate
        // synchronously the way callers expect. Worker-side apply errors
        // (resize failure, PTY-write failure) surface on the next
        // next_event via pending_error.
        if let TerminalAction::PressKey { code } = &action
            && char::from_u32(*code).is_none()
        {
            bail!("PressKey: code {} is not a valid unicode scalar", code);
        }
        self.cmd_tx
            .send(TerminalCommand::Apply { action })
            .map_err(|_| anyhow!("terminal worker gone"))?;
        Ok(())
    }

    async fn extract_snapshots(
        &self,
        state: &TerminalState,
        _last_action: Option<&TerminalAction>,
    ) -> Result<Vec<Snapshot>> {
        self.extractor.run_extractors(state).await
    }

    fn js_action_to_action(js: TerminalAction) -> Result<TerminalAction> {
        Ok(js)
    }

    fn state_timestamp(state: &TerminalState) -> SystemTime {
        state.timestamp
    }
}
