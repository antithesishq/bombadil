use std::sync::Arc;

use anyhow::{Result, anyhow};
use bombadil::specification::{
    domain::Snapshot,
    engine::{Engine, caught_to_string, to_json},
};
use bombadil_schema::Time;
use rquickjs::{
    CatchResultExt, Ctx, Object, Persistent, Value, function::This,
};
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
    runtime_object: Persistent<Object<'static>>,
    engine: Engine,
}

impl Extractors {
    pub fn initialize(bundle_code: &str) -> Result<Self> {
        let engine =
            Engine::new().map_err(|e| anyhow!("engine init failed: {e}"))?;

        let runtime_object = engine.context.with(
            |ctx| -> Result<Persistent<Object<'static>>> {
                js_result(&ctx, ctx.eval::<Value, _>(bundle_code))
                    .map_err(|e| anyhow!("bundle eval failed: {e}"))?;

                let require_value = js_result(
                    &ctx,
                    ctx.globals().get::<_, Value>("__bombadilRequire"),
                )?;
                let require_fn = require_value
                    .as_function()
                    .ok_or(anyhow!("__bombadilRequire is not callable"))?;

                let module_value = js_result(
                    &ctx,
                    require_fn.call::<_, Value>(("@antithesishq/bombadil",)),
                )?;
                let module_obj = module_value
                    .as_object()
                    .ok_or(anyhow!("runtime module is not an object"))?;

                let runtime_value =
                    js_result(&ctx, module_obj.get::<_, Value>("runtime"))?;
                let runtime = runtime_value
                    .as_object()
                    .ok_or(anyhow!("runtime is not an object"))?
                    .clone();

                Ok(Persistent::save(&ctx, runtime))
            },
        )?;

        Ok(Extractors {
            runtime_object,
            engine,
        })
    }

    #[hotpath::measure]
    pub fn run_extractors(
        &mut self,
        state: Arc<TerminalState>,
    ) -> Result<Vec<Snapshot>> {
        let time = Time::from_system_time(state.timestamp);
        let result_json =
            self.engine
                .context
                .with(|ctx| -> Result<Option<json::Value>> {
                    let runtime = js_result(
                        &ctx,
                        self.runtime_object.clone().restore(&ctx),
                    )?;
                    let state_value =
                        js_result(&ctx, terminal_state_to_js(state, &ctx))?;
                    let run_extractors_value = js_result(
                        &ctx,
                        runtime.get::<_, Value>("runExtractors"),
                    )?;
                    let run_extractors_fn = run_extractors_value
                        .as_function()
                        .ok_or(anyhow!("runExtractors is not callable"))?;
                    let result = js_result(
                        &ctx,
                        run_extractors_fn.call::<_, Value>((
                            This(runtime.clone()),
                            state_value,
                        )),
                    )?;
                    js_result(&ctx, to_json(&ctx, result))
                })?;
        let result_json =
            result_json.ok_or(anyhow!("runExtractors returned undefined"))?;

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

fn js_result<'js, T>(ctx: &Ctx<'js>, result: rquickjs::Result<T>) -> Result<T> {
    result
        .catch(ctx)
        .map_err(|error| anyhow!("{}", caught_to_string(error)))
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::time::SystemTime;

    use bombadil::specification::bundler::bundle;
    use bombadil_schema::terminal::{
        TerminalColor, TerminalCursor, TerminalCursorPosition,
        TerminalCursorVisualStyle, TerminalGrid, TerminalSize,
    };
    use tempfile::NamedTempFile;

    use super::*;

    fn terminal_state() -> Arc<TerminalState> {
        let size = TerminalSize {
            columns: 80,
            rows: 24,
        };
        Arc::new(TerminalState {
            timestamp: SystemTime::UNIX_EPOCH,
            grid: TerminalGrid::with_size(size),
            scrollback: TerminalGrid::with_size(size),
            scroll_offset: 0,
            cursor: TerminalCursor {
                position: TerminalCursorPosition { column: 0, row: 0 },
                visible: true,
                blinking: false,
                visual_style: TerminalCursorVisualStyle::Block,
                color: TerminalColor::None,
            },
            exit_status: None,
            last_action: None,
            terminal_snapshot: None,
        })
    }

    #[test]
    fn extracted_lone_surrogates_become_replacement_characters() {
        let mut file = NamedTempFile::with_suffix(".ts").unwrap();
        file.write_all(
            r#"
            import { actions, extract } from "@antithesishq/bombadil";
            export const _actions = actions(() => []);
            export const lone = extract(() => "😀".slice(0, 1));
            export const full = extract(() => "😀");
            "#
            .as_bytes(),
        )
        .unwrap();
        let bundle_code =
            bundle(".", &file.path().display().to_string()).unwrap();
        let mut extractors = Extractors::initialize(&bundle_code).unwrap();
        let values: Vec<json::Value> = extractors
            .run_extractors(terminal_state())
            .unwrap()
            .into_iter()
            .map(|snapshot| snapshot.value)
            .collect();
        assert_eq!(values, vec![json::json!("\u{FFFD}"), json::json!("😀")]);
    }
}
