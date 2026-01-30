use std::collections::HashMap;

use crate::specification::result::{Result, SpecificationError};
use boa_engine::{Context, JsObject, JsValue, Module};

#[allow(dead_code)]
pub struct BombadilExports {
    pub formula: JsValue,
    pub always: JsValue,
    pub contextful: JsValue,
    pub runtime_default: JsObject,
}

impl BombadilExports {
    pub fn from_module(module: &Module, context: &mut Context) -> Result<Self> {
        let exports = module_exports(module, context)?;

        let get_export = |name: &str| -> Result<JsValue> {
            exports
                .get(name)
                .cloned()
                .ok_or(SpecificationError::ModuleError(format!(
                    "{name} is missing in exports"
                )))
        };
        Ok(Self {
            formula: get_export("Formula")?,
            always: get_export("Always")?,
            contextful: get_export("Contextful")?,
            runtime_default: get_export("runtime_default")?.as_object().ok_or(
                SpecificationError::ModuleError(
                    "runtime_default is not an object".to_string(),
                ),
            )?,
        })
    }
}

pub fn module_exports(
    module: &Module,
    context: &mut Context,
) -> Result<HashMap<String, JsValue>> {
    let mut exports = HashMap::new();
    for key in module.namespace(context).own_property_keys(context)? {
        let value = module.namespace(context).get(key.clone(), context)?;
        exports.insert(key.to_string(), value);
    }
    Ok(exports)
}
