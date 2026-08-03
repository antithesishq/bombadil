use std::sync::Arc;

use anyhow::Result;
use boa_engine::JsValue;
use bombadil::specification::domain::Snapshot;
use bombadil::specification::extractor_harness::ExtractorHarness;
use bombadil_schema::Time;
use serde_json as json;

use crate::driver::SwiftUIAction;
use crate::state::SwiftUIState;

pub struct Extractors {
    harness: ExtractorHarness,
}

impl Extractors {
    pub fn initialize(bundle_code: &str) -> Result<Self> {
        Ok(Extractors {
            harness: ExtractorHarness::new(bundle_code)?,
        })
    }

    pub fn run_extractors(
        &mut self,
        state: Arc<SwiftUIState>,
        last_action: Option<&SwiftUIAction>,
    ) -> Result<Vec<Snapshot>> {
        let time = Time::from_system_time(state.timestamp);
        let state_json = json::json!({
            "root": state.root,
            "exitStatus": state.exit_status,
            "lastAction": last_action.map(action_to_json),
        });
        let state_value =
            JsValue::from_json(&state_json, self.harness.context_mut())
                .map_err(|e| anyhow::anyhow!("{e}"))?;
        self.harness.run(state_value, time)
    }
}

/// Convert an action to the JSON shape of the specification's `Action`
/// type: payloads of `TypeText`/`PressKey` are plain strings.
fn action_to_json(action: &crate::driver::SwiftUIAction) -> json::Value {
    use crate::driver::SwiftUIAction::*;
    match action {
        Tap { x, y } => json::json!({"Tap": {"x": x, "y": y}}),
        TypeText { text } => json::json!({"TypeText": text}),
        PressKey { key } => json::json!({"PressKey": key}),
        ScrollUp { x, y, distance } => {
            json::json!({"ScrollUp": {"x": x, "y": y, "distance": distance}})
        }
        ScrollDown { x, y, distance } => {
            json::json!({"ScrollDown": {"x": x, "y": y, "distance": distance}})
        }
    }
}
