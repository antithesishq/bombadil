use std::collections::HashMap;

use crate::specification::result::{Result, SpecificationError};
use boa_engine::{
    js_string, property::PropertyKey, Context, JsObject, JsValue, Module,
};

pub struct BombadilExports {
    pub formula: JsValue,
    pub pure: JsValue,
    pub thunk: JsValue,
    pub and: JsValue,
    pub or: JsValue,
    pub implies: JsValue,
    pub next: JsValue,
    pub always: JsValue,
    pub eventually: JsValue,
    pub runtime_default: JsObject,
    pub time: JsObject,
}

impl BombadilExports {
    pub fn from_module(module: &Module, context: &mut Context) -> Result<Self> {
        let exports = module_exports(module, context)?;

        let get_export = |name: &str| -> Result<JsValue> {
            exports
                .get(&PropertyKey::String(js_string!(name)))
                .cloned()
                .ok_or(SpecificationError::OtherError(format!(
                    "{name} is missing in exports"
                )))
        };
        Ok(Self {
            formula: get_export("Formula")?,
            pure: get_export("Pure")?,
            thunk: get_export("Thunk")?,
            and: get_export("And")?,
            or: get_export("Or")?,
            implies: get_export("Implies")?,
            next: get_export("Next")?,
            always: get_export("Always")?,
            eventually: get_export("Eventually")?,
            runtime_default: get_export("runtime_default")?.as_object().ok_or(
                SpecificationError::OtherError(
                    "runtime_default is not an object".to_string(),
                ),
            )?,
            time: get_export("time")?.as_object().ok_or(
                SpecificationError::OtherError(
                    "time is not an object".to_string(),
                ),
            )?,
        })
    }
}

pub fn module_exports(
    module: &Module,
    context: &mut Context,
) -> Result<HashMap<PropertyKey, JsValue>> {
    let mut exports = HashMap::new();
    for key in module.namespace(context).own_property_keys(context)? {
        let value = module.namespace(context).get(key.clone(), context)?;
        exports.insert(key, value);
    }
    Ok(exports)
}
