use crate::specification::result::Result;
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

    pub fn update_from_results(
        &self,
        results: HashMap<u64, serde_json::Value>,
        context: &mut Context,
    ) -> Result<()> {
        for (id, json_result) in results {
            if let Some(obj) = self.get(id) {
                // Convert JSON to JsValue and update the object
                let js_value = json_to_jsvalue(&json_result, context)?;
                obj.set(js_string!("state"), js_value, true, context)?;
            }
        }
        Ok(())
    }
}

fn json_to_jsvalue(
    json: &serde_json::Value,
    context: &mut Context,
) -> Result<JsValue> {
    match json {
        serde_json::Value::Null => Ok(JsValue::null()),
        serde_json::Value::Bool(b) => Ok(JsValue::new(*b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(JsValue::new(i))
            } else if let Some(f) = n.as_f64() {
                Ok(JsValue::new(f))
            } else {
                Ok(JsValue::undefined())
            }
        }
        serde_json::Value::String(s) => {
            Ok(JsValue::new(js_string!(s.as_str())))
        }
        serde_json::Value::Array(arr) => {
            let js_arr = JsArray::new(context);
            for (i, item) in arr.iter().enumerate() {
                js_arr.set(
                    i,
                    json_to_jsvalue(item, context)?,
                    false,
                    context,
                )?;
            }
            Ok(js_arr.into())
        }
        serde_json::Value::Object(obj) => {
            let js_obj = JsObject::with_null_proto();
            for (key, value) in obj {
                js_obj.set(
                    js_string!(key.as_str()),
                    json_to_jsvalue(value, context)?,
                    true,
                    context,
                )?;
            }
            Ok(js_obj.into())
        }
    }
}
