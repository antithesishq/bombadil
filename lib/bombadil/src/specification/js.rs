use std::collections::HashMap;
use std::time::Duration;

use anyhow::{anyhow, ensure};
use boa_engine::{
    Context, JsObject, JsValue, Module, js_string, property::PropertyKey,
};
use num_traits::NumCast;
use serde::{
    Deserialize, Serialize,
    de::{self},
};
use std::ops::RangeInclusive;

use crate::specification::{
    domain::{BombadilDomain, Snapshot},
    generators::{CharSetEntry, StringGenerator},
};
use crate::specification::{
    generators::Regexp,
    result::{Result, SpecificationError},
};
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

#[derive(Debug, Clone, PartialEq)]
pub enum JsRange {
    Fixed(f64),
    Range((f64, f64)),
}

macro_rules! impl_js_range_try_from {
    ($($t:ty),+) => {
        $(
            impl TryFrom<JsRange> for RangeInclusive<$t> {
                type Error = anyhow::Error;

                fn try_from(value: JsRange) -> anyhow::Result<RangeInclusive<$t>, Self::Error> {
                    fn cast(x: f64, label: &str) -> anyhow::Result<$t> {
                        ensure!(x.is_finite(), "{label} has to be a finite number");
                        ensure!(!x.is_sign_negative(), "{label} must not be negative");
                        ensure!(x.fract() == 0.0, "{label} must not have a fractional part");
                        <$t as NumCast>::from(x)
                            .ok_or(anyhow!("{label} out of range: {x}"))
                    }

                    match value {
                        JsRange::Fixed(x) => {
                            let x = cast(x, "value")?;
                            Ok(x..=x)
                        }
                        JsRange::Range((start, end)) => {
                            let start = cast(start, "start")?;
                            let end = cast(end, "end")?;
                            ensure!(start <= end, "start must be <= end");
                            Ok(start..=end)
                        }
                    }
                }
            }

            impl TryFrom<RangeInclusive<$t>> for JsRange {
                type Error = anyhow::Error;

                fn try_from(value: RangeInclusive<$t>) -> anyhow::Result<JsRange, Self::Error> {
                    Ok(JsRange::Range((*value.start() as f64, *value.end() as f64)))
                }
            }
        )+
    };
}

impl_js_range_try_from!(u8, u16, u32, u64);

impl TryInto<RangeInclusive<f64>> for JsRange {
    type Error = anyhow::Error;

    fn try_into(self) -> anyhow::Result<RangeInclusive<f64>, Self::Error> {
        fn cast(x: f64, label: &str) -> anyhow::Result<f64> {
            ensure!(x.is_finite(), "{label} has to be a finite number");
            ensure!(!x.is_sign_negative(), "{label} must not be negative");
            Ok(x)
        }

        match self {
            JsRange::Fixed(x) => {
                let x = cast(x, "value")?;
                Ok(x..=x)
            }
            JsRange::Range((start, end)) => {
                let start = cast(start, "low part of interval")?;
                let end = cast(end, "high part of interval")?;
                ensure!(
                    start <= end,
                    "low part must be less than or equal to high part of interval"
                );
                Ok(start..=end)
            }
        }
    }
}
impl Serialize for JsRange {
    fn serialize<S>(&self, serializer: S) -> anyhow::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            JsRange::Fixed(x) => x.serialize(serializer),
            JsRange::Range(range) => range.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for JsRange {
    fn deserialize<D>(deserializer: D) -> anyhow::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value: serde_json::Value = Deserialize::deserialize(deserializer)?;
        match value {
            serde_json::Value::Array(_) => {
                let range: (f64, f64) =
                    serde_json::from_value(value).map_err(de::Error::custom)?;
                Ok(JsRange::Range(range))
            }
            _ => {
                let value: f64 =
                    serde_json::from_value(value).map_err(de::Error::custom)?;
                Ok(JsRange::Fixed(value))
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum JsStringGenerator {
    Text(JsRange),
    Email,
    Regexp(String),
    CharSet(Vec<JsCharSetEntry>),
}

impl TryInto<StringGenerator> for JsStringGenerator {
    type Error = anyhow::Error;

    fn try_into(self) -> anyhow::Result<StringGenerator, Self::Error> {
        Ok(match self {
            JsStringGenerator::Text(length) => StringGenerator::Text {
                length: length.try_into()?,
            },
            JsStringGenerator::Email => StringGenerator::Email,
            JsStringGenerator::Regexp(regexp) => StringGenerator::Regexp {
                regexp: Regexp(regexp),
            },
            JsStringGenerator::CharSet(entries) => {
                let mut converted = Vec::with_capacity(entries.len());
                for entry in entries {
                    converted.push(entry.try_into()?);
                }
                StringGenerator::CharSet { entries: converted }
            }
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum JsCharSetEntry {
    Range(JsRange),
    Literal(String),
}

impl TryFrom<JsCharSetEntry> for CharSetEntry {
    type Error = anyhow::Error;

    fn try_from(
        value: JsCharSetEntry,
    ) -> std::result::Result<Self, Self::Error> {
        match value {
            JsCharSetEntry::Range(js_range) => {
                Ok(CharSetEntry::Range(js_range.try_into()?))
            }
            JsCharSetEntry::Literal(s) => Ok(CharSetEntry::Literal(s)),
        }
    }
}
