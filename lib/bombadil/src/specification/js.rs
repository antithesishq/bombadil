use std::cell::OnceCell;
use std::collections::{HashMap, HashSet};
use std::time::Duration;

use anyhow::{anyhow, ensure};
use boa_engine::{
    Context, JsArgs, JsNativeError, JsObject, JsResult, JsString, JsValue,
    JsVariant, Module, NativeFunction, js_string,
    object::FunctionObjectBuilder, property::PropertyKey,
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

/// Make `JSON.parse` read lone surrogates as U+FFFD. boa's rejects them,
/// though `JSON.stringify` writes them.
// TODO: remove once boa is past 0.22, where `JSON.parse` accepts them.
pub fn json_parse_accept_lone_surrogates(
    context: &mut Context,
) -> JsResult<()> {
    let json = context
        .global_object()
        .get(js_string!("JSON"), context)?
        .as_object()
        .ok_or_else(|| JsNativeError::typ().with_message("no JSON object"))?;
    let parse = json
        .get(js_string!("parse"), context)?
        .as_object()
        .ok_or_else(|| JsNativeError::typ().with_message("no JSON.parse"))?;
    let lossy = NativeFunction::from_copy_closure_with_captures(
        |this, arguments, parse: &JsObject, context| {
            let text = arguments.get_or_undefined(0).to_string(context)?;
            let mut arguments =
                [text.clone().into(), arguments.get_or_undefined(1).clone()];
            let error = match parse.call(this, &arguments, context) {
                Ok(value) => return Ok(value),
                Err(error) => error,
            };
            // Text that changes was rejected before any reviver was called,
            // so parsing it again calls a reviver once.
            let (text, changed) = match text.to_std_string() {
                Ok(text) => (text, false),
                Err(_) => (text.to_std_string_lossy(), true),
            };
            let text = match stdx::surrogates::replace_lone(&text) {
                Some(replaced) => replaced,
                None if changed => text,
                None => return Err(error),
            };
            arguments[0] = JsString::from(text).into();
            parse.call(this, &arguments, context)
        },
        parse,
    );
    let lossy = FunctionObjectBuilder::new(context.realm(), lossy)
        .name(js_string!("parse"))
        .length(2)
        .build();
    json.set(js_string!("parse"), lossy, true, context)?;
    Ok(())
}

/// The string with lone surrogates as U+FFFD.
pub fn to_string_lossy(string: &JsString) -> String {
    string
        .to_std_string()
        .unwrap_or_else(|_| string.to_std_string_lossy())
}

/// [`JsValue::to_json`], but with lone surrogates in strings and keys as
/// U+FFFD rather than as the text of their escapes.
pub fn to_json_lossy(
    value: &JsValue,
    context: &mut Context,
) -> JsResult<Option<serde_json::Value>> {
    use serde_json::Value;

    enum Members {
        Array {
            values: Vec<Value>,
            indices: std::ops::Range<u32>,
        },
        Object {
            members: serde_json::Map<String, Value>,
            keys: std::vec::IntoIter<PropertyKey>,
            key: String,
        },
    }

    let mut stack: Vec<(JsObject, Members)> = Vec::new();
    let mut open: HashSet<JsObject> = HashSet::new();
    let mut current = value.clone();
    loop {
        // `None` after descending into an object.
        let mut converted = match current.variant() {
            JsVariant::Null => Some(Some(Value::Null)),
            JsVariant::Undefined => Some(None),
            JsVariant::Boolean(boolean) => Some(Some(Value::from(boolean))),
            JsVariant::String(string) => {
                Some(Some(Value::from(to_string_lossy(&string))))
            }
            JsVariant::Float64(number) => Some(Some(Value::from(number))),
            JsVariant::Integer32(number) => Some(Some(Value::from(number))),
            JsVariant::BigInt(_) => {
                return Err(JsNativeError::typ()
                    .with_message("cannot convert bigint to JSON")
                    .into());
            }
            JsVariant::Symbol(_) => {
                return Err(JsNativeError::typ()
                    .with_message("cannot convert Symbol to JSON")
                    .into());
            }
            JsVariant::Object(object) => {
                if !open.insert(object.clone()) {
                    return Err(JsNativeError::typ()
                        .with_message("cyclic object value")
                        .into());
                }
                let members = if object.is_array() {
                    let length = object
                        .get(js_string!("length"), context)?
                        .to_length(context)?
                        as u32;
                    Members::Array {
                        values: Vec::with_capacity(length as usize),
                        indices: 0..length,
                    }
                } else {
                    // The keys boa's own conversion walks, in its order.
                    let object = object.borrow();
                    let mut keys: Vec<PropertyKey> = object
                        .properties()
                        .index_property_keys()
                        .map(PropertyKey::from)
                        .collect();
                    keys.extend(object.shape().keys());
                    Members::Object {
                        members: serde_json::Map::new(),
                        keys: keys.into_iter(),
                        key: String::new(),
                    }
                };
                stack.push((object, members));
                None
            }
        };
        current = loop {
            let Some((object, members)) = stack.last_mut() else {
                return Ok(converted.flatten());
            };
            let next = match members {
                Members::Array { values, indices } => {
                    if let Some(value) = converted.take() {
                        values.push(value.unwrap_or(Value::Null));
                    }
                    indices.next().map(PropertyKey::from)
                }
                Members::Object { members, keys, key } => {
                    if let Some(Some(value)) = converted.take() {
                        members.insert(std::mem::take(key), value);
                    }
                    let next = keys.next();
                    match &next {
                        Some(PropertyKey::String(string)) => {
                            *key = to_string_lossy(string)
                        }
                        Some(PropertyKey::Index(index)) => {
                            *key = index.get().to_string()
                        }
                        Some(PropertyKey::Symbol(_)) => {
                            return Err(JsNativeError::typ()
                                .with_message("cannot convert Symbol to JSON")
                                .into());
                        }
                        None => {}
                    }
                    next
                }
            };
            let Some(key) = next else {
                converted = stack.pop().map(|(object, members)| {
                    open.remove(&object);
                    Some(match members {
                        Members::Array { values, .. } => Value::Array(values),
                        Members::Object { members, .. } => {
                            Value::Object(members)
                        }
                    })
                });
                continue;
            };
            // As in boa, a member without a value of its own is null.
            break object
                .borrow()
                .properties()
                .get(&key)
                .and_then(|property| property.value().cloned())
                .unwrap_or(JsValue::null());
        };
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeFunction {
    pub object: JsObject,
    pub pretty: String,
}

#[hotpath::measure]
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

    // The formula classes are never subclassed, so we compare
    // the prototypes rather than doing more expensive instanceof
    // checks.
    let prototypes = bombadil.prototypes(context)?;
    let prototype = object.prototype();
    let is_instance = |class_prototype: &JsObject| {
        prototype.as_ref().is_some_and(|prototype| {
            JsObject::equals(prototype, class_prototype)
        })
    };

    if is_instance(&prototypes.pure) {
        let value = object
            .get(js_string!("value"), context)?
            .as_boolean()
            .ok_or(SpecificationError::OtherError(
                "Pure.value is not a boolean".to_string(),
            ))?;
        let pretty = to_string_lossy(
            &object
                .get(js_string!("pretty"), context)?
                .as_string()
                .ok_or(SpecificationError::OtherError(
                    "Pure.pretty is not a string".to_string(),
                ))?,
        );
        return Ok(Pure { value, pretty });
    }

    if is_instance(&prototypes.thunk) {
        let apply_object = object
            .get(js_string!("apply"), context)?
            .as_callable()
            .ok_or(SpecificationError::OtherError(
                "Thunk.apply is not callable".to_string(),
            ))?;
        let pretty_value = object.get(js_string!("pretty"), context)?;
        let pretty = to_string_lossy(&pretty_value.as_string().ok_or(
            SpecificationError::OtherError(format!(
                "Thunk.pretty is not a string: {}",
                pretty_value.display()
            )),
        )?);
        return Ok(Thunk(RuntimeFunction {
            object: apply_object,
            pretty,
        }));
    }

    if is_instance(&prototypes.not) {
        let value = object.get(js_string!("subformula"), context)?;
        let subformula = syntax_from_value(&value, bombadil, context)?;
        return Ok(Not(Box::new(subformula)));
    }

    if is_instance(&prototypes.and) {
        let left_value = object.get(js_string!("left"), context)?;
        let right_value = object.get(js_string!("right"), context)?;
        let left = syntax_from_value(&left_value, bombadil, context)?;
        let right = syntax_from_value(&right_value, bombadil, context)?;
        return Ok(And(Box::new(left), Box::new(right)));
    }

    if is_instance(&prototypes.or) {
        let left_value = object.get(js_string!("left"), context)?;
        let right_value = object.get(js_string!("right"), context)?;
        let left = syntax_from_value(&left_value, bombadil, context)?;
        let right = syntax_from_value(&right_value, bombadil, context)?;
        return Ok(Or(Box::new(left), Box::new(right)));
    }

    if is_instance(&prototypes.implies) {
        let left_value = object.get(js_string!("left"), context)?;
        let right_value = object.get(js_string!("right"), context)?;
        let left = syntax_from_value(&left_value, bombadil, context)?;
        let right = syntax_from_value(&right_value, bombadil, context)?;
        return Ok(Implies(Box::new(left), Box::new(right)));
    }

    if is_instance(&prototypes.next) {
        let subformula_value = object.get(js_string!("subformula"), context)?;
        let subformula =
            syntax_from_value(&subformula_value, bombadil, context)?;
        return Ok(Next(Box::new(subformula)));
    }

    if is_instance(&prototypes.always) {
        let subformula_value = object.get(js_string!("subformula"), context)?;
        let subformula =
            syntax_from_value(&subformula_value, bombadil, context)?;
        let bound = optional_duration_from_js(
            object.get(js_string!("boundMillis"), context)?,
        )?;
        return Ok(Always(Box::new(subformula), bound));
    }

    if is_instance(&prototypes.eventually) {
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
    prototypes: OnceCell<FormulaPrototypes>,
}

/// The `prototype` objects of the formula classes.
#[derive(Debug)]
pub struct FormulaPrototypes {
    pub pure: JsObject,
    pub thunk: JsObject,
    pub not: JsObject,
    pub and: JsObject,
    pub or: JsObject,
    pub implies: JsObject,
    pub next: JsObject,
    pub always: JsObject,
    pub eventually: JsObject,
}

impl BombadilExports {
    pub fn prototypes(
        &self,
        context: &mut Context,
    ) -> Result<&FormulaPrototypes> {
        if let Some(prototypes) = self.prototypes.get() {
            return Ok(prototypes);
        }
        let mut prototype_of = |class: &JsValue| -> Result<JsObject> {
            class
                .as_object()
                .ok_or(SpecificationError::OtherError(format!(
                    "formula class is not an object: {}",
                    class.display()
                )))?
                .get(js_string!("prototype"), context)?
                .as_object()
                .ok_or(SpecificationError::OtherError(format!(
                    "formula class has no prototype object: {}",
                    class.display()
                )))
        };
        let prototypes = FormulaPrototypes {
            pure: prototype_of(&self.pure)?,
            thunk: prototype_of(&self.thunk)?,
            not: prototype_of(&self.not)?,
            and: prototype_of(&self.and)?,
            or: prototype_of(&self.or)?,
            implies: prototype_of(&self.implies)?,
            next: prototype_of(&self.next)?,
            always: prototype_of(&self.always)?,
            eventually: prototype_of(&self.eventually)?,
        };
        Ok(self.prototypes.get_or_init(|| prototypes))
    }
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
            prototypes: OnceCell::new(),
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
            prototypes: OnceCell::new(),
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

    #[hotpath::measure]
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

#[cfg(test)]
mod tests {
    use boa_engine::Source;
    use hegel::{
        TestCase,
        generators::{booleans, floats, integers, sampled_from, text, vecs},
    };
    use serde_json::{Value, json};

    use super::*;

    #[hegel::composite]
    fn expressions(tc: &TestCase, depth: u8) -> String {
        let inner = |tc: &TestCase| tc.draw_silent(expressions(depth - 1));
        let kinds = if depth == 0 { 3 } else { 10 };
        match tc.draw(integers::<u8>().max_value(kinds)) {
            0 => tc
                .draw(sampled_from(vec![
                    "null",
                    "undefined",
                    "true",
                    "NaN",
                    "-0",
                    "2 ** 40",
                    "1.5",
                    "() => 1",
                    "new Date(0)",
                    "new String('ab')",
                    "new Uint8Array([1, 2])",
                    "new Map([[1, 2]])",
                    "({ toJSON() { return 1 } })",
                ]))
                .to_string(),
            1 => tc.draw(integers::<i32>()).to_string(),
            2 => match tc.draw(floats::<f64>()) {
                number if number.is_finite() => format!("({number:?})"),
                _ => "-Infinity".to_string(),
            },
            3 => serde_json::to_string(&tc.draw(text())).unwrap(),
            4 => {
                let mut elements = String::from("[");
                for _ in 0..tc.draw(integers::<u8>().max_value(4)) {
                    if tc.draw(booleans()) {
                        elements.push_str(&inner(tc));
                    }
                    elements.push(',');
                }
                elements + "]"
            }
            5 | 6 => {
                let mut members = String::from("({");
                for _ in 0..tc.draw(integers::<u8>().max_value(5)) {
                    let key = match tc.draw(integers::<u8>().max_value(2)) {
                        0 => tc
                            .draw(sampled_from(vec![
                                "10",
                                "9",
                                "1",
                                "3000000000",
                                "70000",
                                "'01'",
                                "'length'",
                                "'b'",
                                "'a'",
                            ]))
                            .to_string(),
                        1 => serde_json::to_string(&tc.draw(text())).unwrap(),
                        _ => {
                            members.push_str("get g() { return 1 },");
                            continue;
                        }
                    };
                    members.push_str(&format!("[{key}]: {},", inner(tc)));
                }
                members + "})"
            }
            7 => format!("((shared) => [shared, shared])({})", inner(tc)),
            8 => format!(
                "((object) => {{ delete object.a; delete object[9]; \
                 object.a = 1; object[9] = 2; return object }})(Object({}))",
                inner(tc)
            ),
            9 => format!("new Proxy(Object({}), {{}})", inner(tc)),
            _ => format!(
                "((target, value) => {{ try {{ target[7] = value; \
                 target[2] = value }} catch {{}} return target }})\
                 (Object({}), {})",
                inner(tc),
                inner(tc)
            ),
        }
    }

    #[hegel::test(test_cases = 1000)]
    fn to_json_lossy_is_to_json_without_lone_surrogates(tc: TestCase) {
        let mut context = Context::default();
        let source = tc.draw(expressions(3));
        let value = context.eval(Source::from_bytes(&source)).unwrap();
        // As text, because equal JSON objects may differ in key order.
        let serialised = |json: JsResult<Option<Value>>| {
            json.map(|json| json.map(|json| json.to_string()))
                .map_err(|error| error.to_string())
        };
        assert_eq!(
            serialised(to_json_lossy(&value, &mut context)),
            serialised(value.to_json(&mut context)),
            "{source}"
        );
    }

    #[hegel::test(test_cases = 500)]
    fn to_json_lossy_replaces_lone_surrogates(tc: TestCase) {
        let mut context = Context::default();
        let units = tc.draw(vecs(hegel::one_of!(
            integers::<u16>(),
            integers::<u16>().min_value(0xD800).max_value(0xDFFF),
        )));
        let lossy = String::from_utf16_lossy(&units);
        let string = JsString::from(&units[..]);
        let object = JsObject::with_null_proto();
        object
            .set(string.clone(), string.clone(), true, &mut context)
            .unwrap();
        let array = boa_engine::object::builtins::JsArray::from_iter(
            [string.into(), object.into()],
            &mut context,
        );
        // An index key stays a number, so it is not compared as text.
        let keyed = lossy.parse::<u32>().is_err();
        let converted =
            to_json_lossy(&array.into(), &mut context).unwrap().unwrap();
        assert_eq!(converted[0], json!(lossy));
        if keyed {
            assert_eq!(converted[1], json!({ lossy.clone(): lossy }));
        }
    }

    #[test]
    fn to_json_lossy_follows_to_json_on_odd_values() {
        let mut context = Context::default();
        let mut eval =
            |source: &str| context.eval(Source::from_bytes(source)).unwrap();
        let odd = eval(
            "[undefined, , { a: undefined, get b() { return 1 }, 2: [] }, \
             () => 1, NaN, -0, 2 ** 40, { 10: 'x', 9: 'y', 1: 'z' }, \
             new String('ab'), new Uint8Array([1]), new Proxy([1], {}), \
             new Proxy({}, { ownKeys() { throw new RangeError() } })]",
        );
        let cyclic = eval("const a = { b: [] }; a.b.push(a); a");
        let shared = eval("const c = {}; [c, c]");
        assert_eq!(
            to_json_lossy(&odd, &mut context)
                .unwrap()
                .unwrap()
                .to_string(),
            odd.to_json(&mut context).unwrap().unwrap().to_string()
        );
        assert_eq!(
            to_json_lossy(&shared, &mut context).unwrap(),
            Some(json!([{}, {}]))
        );
        assert_eq!(
            to_json_lossy(&cyclic, &mut context)
                .unwrap_err()
                .to_string(),
            cyclic.to_json(&mut context).unwrap_err().to_string()
        );
    }

    #[test]
    fn json_parse_accepts_lone_surrogates() {
        let mut context = Context::default();
        json_parse_accept_lone_surrogates(&mut context).unwrap();
        let results = context
            .eval(Source::from_bytes(
                r#"
                const half = String.fromCharCode(0xd83d);
                const attempt = (run) => {
                  try { return run(); } catch (error) { return error.name; }
                };
                let revived = 0;
                let thrown = 0;
                const coerced = {
                  toString() { revived += 10; return JSON.stringify(half); },
                };
                JSON.stringify([
                  JSON.parse(JSON.stringify(half)),
                  JSON.parse(JSON.stringify({ [half]: [half + "x"] })),
                  JSON.parse('"' + half + '"'),
                  JSON.parse(JSON.stringify([half]), (_key, value) => {
                    revived += 1;
                    return value;
                  }),
                  revived,
                  JSON.parse('{"a": [1, "b"]}'),
                  attempt(() => JSON.parse('{"a": ')),
                  attempt(() => JSON.parse("[1]", () => {
                    thrown += 1;
                    throw new RangeError();
                  })),
                  thrown,
                  JSON.parse(new String(JSON.stringify(half))),
                  JSON.parse(coerced),
                  revived,
                  JSON.parse.name,
                  JSON.parse.length,
                ]).replaceAll(String.fromCharCode(0xfffd), "<U+FFFD>")
                "#,
            ))
            .unwrap();
        insta::assert_snapshot!(
            results.as_string().unwrap().to_std_string_lossy(),
            @r#"["<U+FFFD>",{"<U+FFFD>":["<U+FFFD>x"]},"<U+FFFD>",["<U+FFFD>"],2,{"a":[1,"b"]},"SyntaxError","RangeError",1,"<U+FFFD>","<U+FFFD>",12,"parse",2]"#
        );
    }
}
