use std::sync::Arc;

use anyhow::{Result, anyhow};
use boa_engine::{
    Context, JsError, JsObject, JsValue, NativeFunction, Source,
    context::ContextBuilder, js_string,
};
use bombadil::specification::domain::Snapshot;
use bombadil_schema::Time;
use serde::Deserialize;
use serde_json as json;

use crate::{js::terminal_state_to_js, state::TerminalState};

#[derive(Debug, Clone, Deserialize)]
struct PartialSnapshot {
    pub index: usize,
    pub name: Option<String>,
    pub value: Option<json::Value>,
}

pub struct Extractors {
    context: Context,
    runtime: JsObject,
}

impl Extractors {
    pub fn initialize(bundle_code: &str) -> Result<Self> {
        let mut context = ContextBuilder::default()
            .build()
            .map_err(|e| anyhow!("Boa build: {e}"))?;

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

    #[hotpath::measure]
    pub fn run_extractors(
        &mut self,
        state: Arc<TerminalState>,
    ) -> Result<Vec<Snapshot>> {
        let time = Time::from_system_time(state.timestamp);
        let state_value = terminal_state_to_js(state, &mut self.context);
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

        let snapshots = partials
            .iter()
            .map(|partial| Snapshot {
                index: partial.index,
                name: partial.name.clone(),
                value: partial.value.clone().unwrap_or(json::Value::Null),
                time,
            })
            .collect();

        Ok(snapshots)
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
