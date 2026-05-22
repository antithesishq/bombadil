/// A separate Boa context, running on its own OS thread, used only to run JS
/// extractors over terminal state.
use anyhow::{Result, anyhow};
use boa_engine::{
    Context, JsError, JsObject, JsValue, NativeFunction, Source,
    context::ContextBuilder, js_string,
};
use bombadil::specification::domain::Snapshot;
use bombadil_schema::Time;
use serde::Deserialize;
use serde_json as json;
use tokio::sync::{mpsc, oneshot};

use crate::state::TerminalState;

const EXTRACTOR_STACK_SIZE: usize = 16 * 1024 * 1024;
const RANDOM_BYTES_COUNT_MAX: usize = 4096;

pub struct ExtractorWorker {
    send: mpsc::Sender<ExtractorCommand>,
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
    pub async fn start(bundle_code: String) -> Result<Self> {
        let (ready_send, ready_recv) =
            oneshot::channel::<Result<(), anyhow::Error>>();
        let (send, mut recv) = mpsc::channel::<ExtractorCommand>(32);

        std::thread::Builder::new()
            .stack_size(EXTRACTOR_STACK_SIZE)
            .spawn(move || {
                let mut extractors = match Extractors::initialize(&bundle_code)
                {
                    Ok(state) => {
                        let _ = ready_send.send(Ok(()));
                        state
                    }
                    Err(error) => {
                        let _ = ready_send.send(Err(error));
                        return;
                    }
                };
                while let Some(command) = recv.blocking_recv() {
                    match command {
                        ExtractorCommand::RunExtractors {
                            state_json,
                            reply,
                        } => {
                            let result = extractors.run_extractors(state_json);
                            let _ = reply.send(result);
                        }
                    }
                }
            })?;

        ready_recv
            .await
            .map_err(|_| anyhow!("extractor worker died before ready"))??;
        Ok(Self { send })
    }

    pub async fn run_extractors(
        &self,
        state: &TerminalState,
    ) -> Result<Vec<Snapshot>> {
        let time = Time::from_system_time(state.timestamp);
        let state_json = json::to_value(state)?;
        let (reply_send, reply_recv) = oneshot::channel();
        self.send
            .send(ExtractorCommand::RunExtractors {
                state_json,
                reply: reply_send,
            })
            .await
            .map_err(|_| anyhow!("extractor worker gone"))?;
        let partials = reply_recv
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

struct Extractors {
    context: Context,
    runtime: JsObject,
}

impl Extractors {
    fn initialize(bundle_code: &str) -> Result<Self> {
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
            .map_err(from_js_error)?;

        let console_obj =
            boa_engine::object::ObjectInitializer::new(&mut context)
                .function(
                    NativeFunction::from_copy_closure(
                        |_this, args, _context| {
                            log::info!("{}", format_console_args(args));
                            Ok(JsValue::undefined())
                        },
                    ),
                    js_string!("log"),
                    0,
                )
                .function(
                    NativeFunction::from_copy_closure(
                        |_this, args, _context| {
                            log::warn!("{}", format_console_args(args));
                            Ok(JsValue::undefined())
                        },
                    ),
                    js_string!("warn"),
                    0,
                )
                .function(
                    NativeFunction::from_copy_closure(
                        |_this, args, _context| {
                            log::error!("{}", format_console_args(args));
                            Ok(JsValue::undefined())
                        },
                    ),
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
            .map_err(from_js_error)?;

        context
            .eval(Source::from_bytes(bundle_code))
            .map_err(|e| anyhow!("bundle eval failed: {e}"))?;

        let require_fn = context
            .global_object()
            .get(js_string!("__bombadilRequire"), &mut context)
            .map_err(from_js_error)?
            .as_callable()
            .ok_or(anyhow!("__bombadilRequire is not callable"))?;

        let module_value = require_fn
            .call(
                &JsValue::undefined(),
                &[js_string!("@antithesishq/bombadil").into()],
                &mut context,
            )
            .map_err(from_js_error)?;
        let module_obj = module_value
            .as_object()
            .ok_or(anyhow!("runtime module is not an object"))?
            .clone();
        let runtime = module_obj
            .get(js_string!("runtime"), &mut context)
            .map_err(from_js_error)?
            .as_object()
            .ok_or(anyhow!("runtime is not an object"))?
            .clone();

        Ok(Extractors { context, runtime })
    }

    fn run_extractors(
        &mut self,
        state_json: json::Value,
    ) -> Result<Vec<PartialSnapshot>> {
        let state_value = JsValue::from_json(&state_json, &mut self.context)
            .map_err(from_js_error)?;
        let run_extractors_fn = self
            .runtime
            .get(js_string!("runExtractors"), &mut self.context)
            .map_err(from_js_error)?
            .as_callable()
            .ok_or(anyhow!("runExtractors is not callable"))?;
        let result = run_extractors_fn
            .call(
                &JsValue::from(self.runtime.clone()),
                &[state_value],
                &mut self.context,
            )
            .map_err(from_js_error)?;
        let result_json = result
            .to_json(&mut self.context)
            .map_err(from_js_error)?
            .ok_or(anyhow!("runExtractors returned undefined"))?;
        let partials: Vec<PartialSnapshot> = json::from_value(result_json)?;
        Ok(partials)
    }
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

fn from_js_error(error: JsError) -> anyhow::Error {
    anyhow!("{}", error)
}
