use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::{Result, anyhow, bail};
use bombadil::driver::{DriverEvent, InterfaceDriver};
use bombadil::specification::bundler::bundle;
use bombadil::specification::domain::Snapshot;
use bombadil::specification::verifier::Specification;
use bombadil::specification::worker::VerifierWorker;
use boa_engine::{
    Context, JsObject, JsString, JsValue, NativeFunction, Source,
    context::ContextBuilder, js_string,
};
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
const RANDOM_BYTES_COUNT_MAX: usize = 4096;

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
    TypeText { text: String },
    #[serde(rename_all = "camelCase")]
    PressKey { code: u32 },
    #[serde(rename_all = "camelCase")]
    Resize { size: Size },
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
/// spec and serves `runExtractors` calls for the terminal driver. Kept
/// distinct from the verifier's Boa: extractors and property evaluation
/// must not share state.
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
                            let result =
                                run_extractors(&mut state, state_json);
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

    context.register_global_builtin_callable(
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
    )?;

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
    context.register_global_property(
        js_string!("console"),
        console_obj,
        boa_engine::property::Attribute::all(),
    )?;

    context
        .eval(Source::from_bytes(bundle_code))
        .map_err(|e| anyhow!("bundle eval failed: {e}"))?;

    let require_fn = context
        .global_object()
        .get(js_string!("__bombadilRequire"), &mut context)?
        .as_callable()
        .ok_or(anyhow!("__bombadilRequire is not callable"))?;

    let module_value = require_fn.call(
        &JsValue::undefined(),
        &[JsValue::from(JsString::from(runtime_module))],
        &mut context,
    )?;
    let module_obj = module_value
        .as_object()
        .ok_or(anyhow!("runtime module is not an object"))?
        .clone();
    let runtime = module_obj
        .get(js_string!("runtime"), &mut context)?
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
    let state_value = JsValue::from_json(&state_json, &mut state.context)?;
    let run_extractors_fn = state
        .runtime
        .get(js_string!("runExtractors"), &mut state.context)?
        .as_callable()
        .ok_or(anyhow!("runExtractors is not callable"))?;
    let result = run_extractors_fn.call(
        &JsValue::from(state.runtime.clone()),
        &[state_value],
        &mut state.context,
    )?;
    let result_json = result
        .to_json(&mut state.context)?
        .ok_or(anyhow!("runExtractors returned undefined"))?;
    let partials: Vec<PartialSnapshot> = json::from_value(result_json)?;
    Ok(partials)
}

pub struct TerminalDriver {
    terminal: Terminal,
    process: PtyProcess,
    output: PtyOutput,
    size: Size,
    last_action: Option<TerminalAction>,
    extractor: ExtractorWorker,
}

impl TerminalDriver {
    pub async fn launch(
        specification: Specification,
        size: Size,
        max_scrollback: u32,
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

        let terminal = Terminal::new(TerminalOptions {
            cols: size.columns,
            rows: size.rows,
            max_scrollback,
        })?;
        let (process, output) =
            PtyProcess::spawn(size, program, args).await?;

        let driver = Self {
            terminal,
            process,
            output,
            size,
            last_action: None,
            extractor,
        };
        Ok((driver, verifier))
    }

    fn drain_output(&mut self) -> Result<()> {
        while let Some(data) = self.output.try_read() {
            self.terminal.vt_write(&data.into_bytes());
        }
        Ok(())
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
}

impl InterfaceDriver for TerminalDriver {
    type Action = TerminalAction;
    type JsAction = TerminalAction;
    type State = TerminalState;

    async fn initiate(&mut self) -> Result<()> {
        // Give the child a moment to produce its initial output, then
        // let next_event do the rest. 200ms matches the original
        // standalone fuzzer's startup sleep.
        tokio::time::sleep(Duration::from_millis(200)).await;
        Ok(())
    }

    async fn terminate(mut self) -> Result<()> {
        self.process.kill().await;
        Ok(())
    }

    async fn next_event(&mut self) -> Option<DriverEvent<TerminalState>> {
        // Drain PTY output until it stays idle for QUIESCENCE_IDLE, then
        // build a TerminalState and emit it. EOF on the channel means
        // the child has exited.
        let mut got_eof = false;
        loop {
            match tokio::time::timeout(QUIESCENCE_IDLE, self.output.read())
                .await
            {
                Ok(Ok(Some(data))) => {
                    self.terminal.vt_write(&data.into_bytes());
                    if let Err(error) = self.drain_output() {
                        return Some(DriverEvent::Error(Arc::new(error)));
                    }
                }
                Ok(Ok(None)) => {
                    got_eof = true;
                    break;
                }
                Ok(Err(error)) => {
                    return Some(DriverEvent::Error(Arc::new(error)));
                }
                Err(_) => break, // idle for QUIESCENCE_IDLE
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
                    bail!("PressKey: code {} is not a valid unicode scalar", code);
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
