use crate::specification::{
    bombadil_exports::BombadilExports,
    result::{Result, SpecificationError},
};
use boa_engine::*;
use serde_json as json;
use std::{
    collections::HashMap,
    time::{SystemTime, UNIX_EPOCH},
};

pub struct Extractors {
    next_id: u64,
    instances: HashMap<u64, JsObject>,
    time: JsObject,
}

impl Extractors {
    pub fn new(bombadil_exports: &BombadilExports) -> Self {
        Self {
            next_id: 0,
            instances: HashMap::new(),
            time: bombadil_exports.time.clone(),
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
        results: Vec<(u64, json::Value)>,
        time: SystemTime,
        context: &mut Context,
    ) -> Result<()> {
        let update = |extractor: &JsObject,
                      value: JsValue,
                      time: JsValue,
                      context: &mut Context|
         -> Result<()> {
            let method = extractor
                .get(js_string!("update"), context)?
                .as_callable()
                .ok_or(SpecificationError::OtherError(
                    "update is not callable".to_string(),
                ))?;
            method.call(
                &JsValue::from(extractor.clone()),
                &[value, time],
                context,
            )?;
            Ok(())
        };

        let time = JsValue::from_json(
            &json::Value::Number(
                json::Number::from_u128(
                    time.duration_since(UNIX_EPOCH)?.as_millis(),
                )
                .ok_or(SpecificationError::OtherError(
                    "conversion from SystemTime to number failed".to_string(),
                ))?,
            ),
            context,
        )?;

        update(&self.time, JsValue::null(), time.clone(), context)?;

        for (id, json_result) in results {
            if let Some(obj) = self.get(id) {
                let js_value = JsValue::from_json(&json_result, context)?;
                update(obj, js_value, time.clone(), context)?;
            }
        }
        Ok(())
    }
}
