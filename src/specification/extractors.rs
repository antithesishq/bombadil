use crate::specification::result::{Result, SpecificationError};
use boa_engine::{object::builtins::JsArray, *};
use std::collections::HashMap;

pub struct Extractors {
    next_id: u64,
    instances: HashMap<u64, JsObject>,
}

impl Extractors {
    pub fn new() -> Self {
        Self {
            next_id: 0,
            instances: HashMap::new(),
        }
    }

    pub fn register(&mut self, obj: JsObject) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.instances.insert(id, obj);
        id
    }

    pub fn get(&self, id: u64) -> Option<&JsObject> {
        self.instances.get(&id)
    }

    pub fn extract_functions(
        &self,
        context: &mut Context,
    ) -> Result<HashMap<u64, String>> {
        let mut functions = HashMap::new();

        for (&id, obj) in &self.instances {
            let func = obj.get(js_string!("extract"), context)?;
            functions
                .insert(id, func.to_string(context)?.to_std_string_lossy());
        }

        Ok(functions)
    }

    pub fn update_from_snapshots(
        &self,
        results: HashMap<u64, serde_json::Value>,
        context: &mut Context,
    ) -> Result<()> {
        for (id, json_result) in results {
            if let Some(obj) = self.get(id) {
                let js_value = JsValue::from_json(&json_result, context)?;
                let method = obj
                    .get(js_string!("update"), context)?
                    .as_callable()
                    .ok_or(SpecificationError::ModuleError(
                        "update is not callable".to_string(),
                    ))?;
                method.call(
                    &JsValue::from(obj.clone()),
                    &[js_value],
                    context,
                )?;
            }
        }
        Ok(())
    }
}
