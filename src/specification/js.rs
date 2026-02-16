use std::collections::HashMap;
use std::time::Duration;

use boa_engine::{
    Context, JsObject, JsValue, Module, js_string, property::PropertyKey,
};

use serde_json as json;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::specification::{
    result::{Result, SpecificationError},
    syntax::Syntax,
};

#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeFunction {
    pub object: JsObject,
    pub pretty: String,
}

impl Syntax<RuntimeFunction> {
    pub fn from_value(
        value: &JsValue,
        bombadil: &BombadilExports,
        context: &mut Context,
    ) -> Result<Self> {
        use Syntax::*;

        let object =
            value.as_object().ok_or(SpecificationError::OtherError(
                format!("formula is not an object: {}", value.display()),
            ))?;

        if value.instance_of(&bombadil.pure, context)? {
            let value = object
                .get(js_string!("value"), context)?
                .as_boolean()
                .ok_or(SpecificationError::OtherError(
                    "Pure.value is not a boolean".to_string(),
                ))?;
            let pretty = object
                .get(js_string!("pretty"), context)?
                .as_string()
                .ok_or(SpecificationError::OtherError(
                    "Pure.pretty is not a string".to_string(),
                ))?
                .to_std_string_escaped();
            return Ok(Self::Pure { value, pretty });
        }

        if value.instance_of(&bombadil.thunk, context)? {
            let apply_object = object
                .get(js_string!("apply"), context)?
                .as_callable()
                .ok_or(SpecificationError::OtherError(
                    "Thunk.apply is not callable".to_string(),
                ))?;
            let pretty_value = object.get(js_string!("pretty"), context)?;
            let pretty = pretty_value
                .as_string()
                .ok_or(SpecificationError::OtherError(format!(
                    "Thunk.pretty is not a string: {}",
                    pretty_value.display()
                )))?
                .to_std_string_escaped();
            return Ok(Self::Thunk(RuntimeFunction {
                object: apply_object,
                pretty,
            }));
        }

        if value.instance_of(&bombadil.not, context)? {
            let value = object.get(js_string!("subformula"), context)?;
            let subformula = Self::from_value(&value, bombadil, context)?;
            return Ok(Not(Box::new(subformula)));
        }

        if value.instance_of(&bombadil.and, context)? {
            let left_value = object.get(js_string!("left"), context)?;
            let right_value = object.get(js_string!("right"), context)?;
            let left = Self::from_value(&left_value, bombadil, context)?;
            let right = Self::from_value(&right_value, bombadil, context)?;
            return Ok(And(Box::new(left), Box::new(right)));
        }

        if value.instance_of(&bombadil.or, context)? {
            let left_value = object.get(js_string!("left"), context)?;
            let right_value = object.get(js_string!("right"), context)?;
            let left = Self::from_value(&left_value, bombadil, context)?;
            let right = Self::from_value(&right_value, bombadil, context)?;
            return Ok(Or(Box::new(left), Box::new(right)));
        }

        if value.instance_of(&bombadil.implies, context)? {
            let left_value = object.get(js_string!("left"), context)?;
            let right_value = object.get(js_string!("right"), context)?;
            let left = Self::from_value(&left_value, bombadil, context)?;
            let right = Self::from_value(&right_value, bombadil, context)?;
            return Ok(Implies(Box::new(left), Box::new(right)));
        }

        if value.instance_of(&bombadil.next, context)? {
            let subformula_value =
                object.get(js_string!("subformula"), context)?;
            let subformula =
                Self::from_value(&subformula_value, bombadil, context)?;
            return Ok(Next(Box::new(subformula)));
        }

        if value.instance_of(&bombadil.always, context)? {
            let subformula_value =
                object.get(js_string!("subformula"), context)?;
            let subformula =
                Self::from_value(&subformula_value, bombadil, context)?;
            let bound = optional_duration_from_js(
                object.get(js_string!("bound"), context)?,
                context,
            )?;
            return Ok(Always(Box::new(subformula), bound));
        }

        if value.instance_of(&bombadil.eventually, context)? {
            let subformula_value =
                object.get(js_string!("subformula"), context)?;
            let subformula =
                Self::from_value(&subformula_value, bombadil, context)?;
            let bound = optional_duration_from_js(
                object.get(js_string!("bound"), context)?,
                context,
            )?;
            return Ok(Eventually(Box::new(subformula), bound));
        }

        Err(SpecificationError::OtherError(format!(
            "can't convert to formula: {}",
            value.display()
        )))
    }
}

fn optional_duration_from_js(
    value: JsValue,
    context: &mut Context,
) -> Result<Option<Duration>> {
    if value.is_null_or_undefined() {
        return Ok(None);
    }

    let object =
        value
            .as_object()
            .ok_or(SpecificationError::OtherError(format!(
                "duration is not an object: {}",
                value.display()
            )))?;
    let milliseconds_value = object.get(js_string!("milliseconds"), context)?;

    let milliseconds = milliseconds_value.as_number().ok_or(
        SpecificationError::OtherError(format!(
            "milliseconds is not a number: {}",
            milliseconds_value.display()
        )),
    )?;
    if milliseconds < 0.0 {
        return Err(SpecificationError::OtherError(format!(
            "milliseconds is negative: {}",
            milliseconds_value.display()
        )));
    }
    if milliseconds.is_infinite() {
        return Err(SpecificationError::OtherError(format!(
            "milliseconds is {}",
            milliseconds_value.display()
        )));
    }
    Ok(Some(Duration::from_millis(milliseconds as u64)))
}

pub struct BombadilExports {
    pub formula: JsValue,
    pub pure: JsValue,
    pub thunk: JsValue,
    pub not: JsValue,
    pub and: JsValue,
    pub or: JsValue,
    pub implies: JsValue,
    pub next: JsValue,
    pub always: JsValue,
    pub eventually: JsValue,
    pub runtime_default: JsObject,
    pub time: JsObject,
    pub action_generator: JsValue,
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
            not: get_export("Not")?,
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
            action_generator: get_export("ActionGenerator")?,
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
