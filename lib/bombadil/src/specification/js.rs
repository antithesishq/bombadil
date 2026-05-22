use std::collections::HashMap;
use std::time::Duration;

use boa_engine::{
    Context, JsObject, JsValue, Module, js_string, property::PropertyKey,
};

use crate::specification::domain::{BombadilDomain, Snapshot};
use crate::specification::result::{Result, SpecificationError};
use bombadil_ltl::syntax::Syntax;

#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeFunction {
    pub object: JsObject,
    pub pretty: String,
}

pub fn syntax_from_value(
    value: &JsValue,
    bombadil: &BombadilExports,
    context: &mut Context,
) -> Result<Syntax<BombadilDomain<RuntimeFunction>>> {
    use Syntax::*;

    let object =
        value
            .as_object()
            .ok_or(SpecificationError::OtherError(format!(
                "formula is not an object: {}",
                value.display()
            )))?;

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
        return Ok(Pure { value, pretty });
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
        return Ok(Thunk(RuntimeFunction {
            object: apply_object,
            pretty,
        }));
    }

    if value.instance_of(&bombadil.not, context)? {
        let value = object.get(js_string!("subformula"), context)?;
        let subformula = syntax_from_value(&value, bombadil, context)?;
        return Ok(Not(Box::new(subformula)));
    }

    if value.instance_of(&bombadil.and, context)? {
        let left_value = object.get(js_string!("left"), context)?;
        let right_value = object.get(js_string!("right"), context)?;
        let left = syntax_from_value(&left_value, bombadil, context)?;
        let right = syntax_from_value(&right_value, bombadil, context)?;
        return Ok(And(Box::new(left), Box::new(right)));
    }

    if value.instance_of(&bombadil.or, context)? {
        let left_value = object.get(js_string!("left"), context)?;
        let right_value = object.get(js_string!("right"), context)?;
        let left = syntax_from_value(&left_value, bombadil, context)?;
        let right = syntax_from_value(&right_value, bombadil, context)?;
        return Ok(Or(Box::new(left), Box::new(right)));
    }

    if value.instance_of(&bombadil.implies, context)? {
        let left_value = object.get(js_string!("left"), context)?;
        let right_value = object.get(js_string!("right"), context)?;
        let left = syntax_from_value(&left_value, bombadil, context)?;
        let right = syntax_from_value(&right_value, bombadil, context)?;
        return Ok(Implies(Box::new(left), Box::new(right)));
    }

    if value.instance_of(&bombadil.next, context)? {
        let subformula_value = object.get(js_string!("subformula"), context)?;
        let subformula =
            syntax_from_value(&subformula_value, bombadil, context)?;
        return Ok(Next(Box::new(subformula)));
    }

    if value.instance_of(&bombadil.always, context)? {
        let subformula_value = object.get(js_string!("subformula"), context)?;
        let subformula =
            syntax_from_value(&subformula_value, bombadil, context)?;
        let bound = optional_duration_from_js(
            object.get(js_string!("boundMillis"), context)?,
        )?;
        return Ok(Always(Box::new(subformula), bound));
    }

    if value.instance_of(&bombadil.eventually, context)? {
        let subformula_value = object.get(js_string!("subformula"), context)?;
        let subformula =
            syntax_from_value(&subformula_value, bombadil, context)?;
        let bound = optional_duration_from_js(
            object.get(js_string!("boundMillis"), context)?,
        )?;
        return Ok(Eventually(Box::new(subformula), bound));
    }

    Err(SpecificationError::OtherError(format!(
        "can't convert to formula: {}",
        value.display()
    )))
}

fn optional_duration_from_js(value: JsValue) -> Result<Option<Duration>> {
    if value.is_null_or_undefined() {
        return Ok(None);
    }
    let millis =
        value
            .as_number()
            .ok_or(SpecificationError::OtherError(format!(
                "milliseconds is not a number: {}",
                value.display()
            )))?;
    if millis < 0.0 {
        return Err(SpecificationError::OtherError(format!(
            "milliseconds is negative: {}",
            value.display()
        )));
    }
    if millis.is_nan() || millis.is_infinite() {
        return Err(SpecificationError::OtherError(format!(
            "milliseconds is {}",
            value.display()
        )));
    }
    Ok(Some(Duration::from_millis(millis as u64)))
}

#[derive(Debug)]
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
    pub runtime: JsObject,
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
            runtime: get_export("runtime")?.as_object().ok_or(
                SpecificationError::OtherError(
                    "runtime is not an object".to_string(),
                ),
            )?,
            action_generator: get_export("ActionGenerator")?,
        })
    }

    pub fn from_object(obj: &JsObject, context: &mut Context) -> Result<Self> {
        let mut get_export = |name: &str| -> Result<JsValue> {
            obj.get(js_string!(name), context).map_err(|e| {
                SpecificationError::OtherError(format!(
                    "Failed to get {}: {}",
                    name, e
                ))
            })
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
            runtime: get_export("runtime")?.as_object().ok_or(
                SpecificationError::OtherError(
                    "runtime is not an object".to_string(),
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
    instances: Vec<JsObject>,
}

impl Extractors {
    pub fn new() -> Self {
        Self { instances: vec![] }
    }

    pub fn register(&mut self, obj: JsObject) {
        self.instances.push(obj);
    }

    pub fn get(&self, index: usize) -> Option<&JsObject> {
        self.instances.get(index)
    }

    pub fn update_from_snapshots(
        &self,
        snapshots: &[Snapshot],
        context: &mut Context,
    ) -> Result<()> {
        let update = |extractor: &JsObject,
                      value: JsValue,
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
                &[value],
                context,
            )?;
            Ok(())
        };

        for (index, snapshot) in snapshots.iter().enumerate() {
            if let Some(obj) = self.get(index) {
                let js_value = JsValue::from_json(&snapshot.value, context)?;
                update(obj, js_value, context)?;
            }
        }
        Ok(())
    }
}

impl Default for Extractors {
    fn default() -> Self {
        Self::new()
    }
}
