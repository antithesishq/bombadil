use std::cell::OnceCell;
use std::time::Duration;

use anyhow::{anyhow, ensure};
use num_traits::NumCast;
use rquickjs::{
    CatchResultExt, Ctx, Function, Object, Persistent, Value, function::This,
};
use serde::{
    Deserialize, Serialize,
    de::{self},
};
use std::ops::RangeInclusive;

use crate::specification::engine::{caught_to_string, display_value};
use crate::specification::{
    domain::{BombadilDomain, Snapshot},
    generators::{CharSetEntry, StringGenerator},
};
use crate::specification::{
    generators::Regexp,
    result::{JsResultExt, Result, SpecificationError},
};
use bombadil_ltl::syntax::Syntax;

#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeFunction {
    pub function: Persistent<Function<'static>>,
    pub pretty: String,
}

fn string_of(value: &Value<'_>) -> Option<String> {
    value.as_string()?.to_string().ok()
}

#[hotpath::measure]
pub fn syntax_from_value<'js>(
    value: &Value<'js>,
    bombadil: &BombadilExports,
    ctx: &Ctx<'js>,
) -> Result<Syntax<BombadilDomain<RuntimeFunction>>> {
    use Syntax::*;

    let object =
        value
            .as_object()
            .ok_or(SpecificationError::OtherError(format!(
                "formula is not an object: {}",
                display_value(value)
            )))?;

    // The formula classes are never subclassed, so we compare
    // the prototypes rather than doing more expensive instanceof
    // checks.
    let prototypes = bombadil.prototypes(ctx)?;
    let prototype = object.get_prototype();
    let is_instance = |class_prototype: &Persistent<Object<'static>>| {
        let class_prototype = class_prototype.clone().restore(ctx).js(ctx)?;
        Ok::<_, SpecificationError>(
            prototype.as_ref() == Some(&class_prototype),
        )
    };

    if is_instance(&prototypes.pure)? {
        let value = object.get::<_, Value>("value").js(ctx)?.as_bool().ok_or(
            SpecificationError::OtherError(
                "Pure.value is not a boolean".to_string(),
            ),
        )?;
        let pretty_value = object.get::<_, Value>("pretty").js(ctx)?;
        let pretty =
            string_of(&pretty_value).ok_or(SpecificationError::OtherError(
                "Pure.pretty is not a string".to_string(),
            ))?;
        return Ok(Pure { value, pretty });
    }

    if is_instance(&prototypes.thunk)? {
        let apply = object.get::<_, Value>("apply").js(ctx)?;
        let apply =
            apply.as_function().ok_or(SpecificationError::OtherError(
                "Thunk.apply is not callable".to_string(),
            ))?;
        let pretty_value = object.get::<_, Value>("pretty").js(ctx)?;
        let pretty = string_of(&pretty_value).ok_or(
            SpecificationError::OtherError(format!(
                "Thunk.pretty is not a string: {}",
                display_value(&pretty_value)
            )),
        )?;
        return Ok(Thunk(RuntimeFunction {
            function: Persistent::save(ctx, apply.clone()),
            pretty,
        }));
    }

    if is_instance(&prototypes.not)? {
        let value = object.get::<_, Value>("subformula").js(ctx)?;
        let subformula = syntax_from_value(&value, bombadil, ctx)?;
        return Ok(Not(Box::new(subformula)));
    }

    if is_instance(&prototypes.and)? {
        let left_value = object.get::<_, Value>("left").js(ctx)?;
        let right_value = object.get::<_, Value>("right").js(ctx)?;
        let left = syntax_from_value(&left_value, bombadil, ctx)?;
        let right = syntax_from_value(&right_value, bombadil, ctx)?;
        return Ok(And(Box::new(left), Box::new(right)));
    }

    if is_instance(&prototypes.or)? {
        let left_value = object.get::<_, Value>("left").js(ctx)?;
        let right_value = object.get::<_, Value>("right").js(ctx)?;
        let left = syntax_from_value(&left_value, bombadil, ctx)?;
        let right = syntax_from_value(&right_value, bombadil, ctx)?;
        return Ok(Or(Box::new(left), Box::new(right)));
    }

    if is_instance(&prototypes.implies)? {
        let left_value = object.get::<_, Value>("left").js(ctx)?;
        let right_value = object.get::<_, Value>("right").js(ctx)?;
        let left = syntax_from_value(&left_value, bombadil, ctx)?;
        let right = syntax_from_value(&right_value, bombadil, ctx)?;
        return Ok(Implies(Box::new(left), Box::new(right)));
    }

    if is_instance(&prototypes.next)? {
        let subformula_value = object.get::<_, Value>("subformula").js(ctx)?;
        let subformula = syntax_from_value(&subformula_value, bombadil, ctx)?;
        return Ok(Next(Box::new(subformula)));
    }

    if is_instance(&prototypes.always)? {
        let subformula_value = object.get::<_, Value>("subformula").js(ctx)?;
        let subformula = syntax_from_value(&subformula_value, bombadil, ctx)?;
        let bound = optional_duration_from_js(
            object.get::<_, Value>("boundMillis").js(ctx)?,
        )?;
        return Ok(Always(Box::new(subformula), bound));
    }

    if is_instance(&prototypes.eventually)? {
        let subformula_value = object.get::<_, Value>("subformula").js(ctx)?;
        let subformula = syntax_from_value(&subformula_value, bombadil, ctx)?;
        let bound = optional_duration_from_js(
            object.get::<_, Value>("boundMillis").js(ctx)?,
        )?;
        return Ok(Eventually(Box::new(subformula), bound));
    }

    Err(SpecificationError::OtherError(format!(
        "can't convert to formula: {}",
        display_value(value)
    )))
}

fn optional_duration_from_js(value: Value<'_>) -> Result<Option<Duration>> {
    if value.is_null() || value.is_undefined() {
        return Ok(None);
    }
    let millis =
        value
            .as_number()
            .ok_or(SpecificationError::OtherError(format!(
                "milliseconds is not a number: {}",
                display_value(&value)
            )))?;
    if millis < 0.0 {
        return Err(SpecificationError::OtherError(format!(
            "milliseconds is negative: {}",
            display_value(&value)
        )));
    }
    if millis.is_nan() || millis.is_infinite() {
        return Err(SpecificationError::OtherError(format!(
            "milliseconds is {}",
            display_value(&value)
        )));
    }
    Ok(Some(Duration::from_millis(millis as u64)))
}

#[derive(Debug)]
pub struct BombadilExports {
    pub formula: Persistent<Value<'static>>,
    pub pure: Persistent<Value<'static>>,
    pub thunk: Persistent<Value<'static>>,
    pub not: Persistent<Value<'static>>,
    pub and: Persistent<Value<'static>>,
    pub or: Persistent<Value<'static>>,
    pub implies: Persistent<Value<'static>>,
    pub next: Persistent<Value<'static>>,
    pub always: Persistent<Value<'static>>,
    pub eventually: Persistent<Value<'static>>,
    pub runtime: Persistent<Object<'static>>,
    pub action_generator: Persistent<Value<'static>>,
    prototypes: OnceCell<FormulaPrototypes>,
}

/// The `prototype` objects of the formula classes.
#[derive(Debug)]
pub struct FormulaPrototypes {
    pub pure: Persistent<Object<'static>>,
    pub thunk: Persistent<Object<'static>>,
    pub not: Persistent<Object<'static>>,
    pub and: Persistent<Object<'static>>,
    pub or: Persistent<Object<'static>>,
    pub implies: Persistent<Object<'static>>,
    pub next: Persistent<Object<'static>>,
    pub always: Persistent<Object<'static>>,
    pub eventually: Persistent<Object<'static>>,
}

impl BombadilExports {
    pub fn prototypes<'js>(
        &self,
        ctx: &Ctx<'js>,
    ) -> Result<&FormulaPrototypes> {
        if let Some(prototypes) = self.prototypes.get() {
            return Ok(prototypes);
        }
        let prototype_of = |class: &Persistent<Value<'static>>| {
            let class = class.clone().restore(ctx).js(ctx)?;
            let class_object = class.as_object().ok_or(
                SpecificationError::OtherError(format!(
                    "formula class is not an object: {}",
                    display_value(&class)
                )),
            )?;
            let prototype =
                class_object.get::<_, Value>("prototype").js(ctx)?;
            let prototype_object = prototype.as_object().ok_or(
                SpecificationError::OtherError(format!(
                    "formula class has no prototype object: {}",
                    display_value(&class)
                )),
            )?;
            Ok::<_, SpecificationError>(Persistent::save(
                ctx,
                prototype_object.clone(),
            ))
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

    pub fn from_object<'js>(obj: &Object<'js>, ctx: &Ctx<'js>) -> Result<Self> {
        let get_export = |name: &str| -> Result<Value<'js>> {
            obj.get::<_, Value>(name).catch(ctx).map_err(|error| {
                SpecificationError::OtherError(format!(
                    "Failed to get {}: {}",
                    name,
                    caught_to_string(error)
                ))
            })
        };
        let formula = get_export("Formula")?;
        let pure = get_export("Pure")?;
        let thunk = get_export("Thunk")?;
        let not = get_export("Not")?;
        let and = get_export("And")?;
        let or = get_export("Or")?;
        let implies = get_export("Implies")?;
        let next = get_export("Next")?;
        let always = get_export("Always")?;
        let eventually = get_export("Eventually")?;
        let runtime = get_export("runtime")?;
        let runtime =
            runtime.as_object().ok_or(SpecificationError::OtherError(
                "runtime is not an object".to_string(),
            ))?;
        let action_generator = get_export("ActionGenerator")?;
        Ok(Self {
            formula: Persistent::save(ctx, formula),
            pure: Persistent::save(ctx, pure),
            thunk: Persistent::save(ctx, thunk),
            not: Persistent::save(ctx, not),
            and: Persistent::save(ctx, and),
            or: Persistent::save(ctx, or),
            implies: Persistent::save(ctx, implies),
            next: Persistent::save(ctx, next),
            always: Persistent::save(ctx, always),
            eventually: Persistent::save(ctx, eventually),
            runtime: Persistent::save(ctx, runtime.clone()),
            action_generator: Persistent::save(ctx, action_generator),
            prototypes: OnceCell::new(),
        })
    }
}

pub struct Extractors {
    instances: Vec<Persistent<Object<'static>>>,
}

impl Extractors {
    pub fn new() -> Self {
        Self { instances: vec![] }
    }

    pub fn register(&mut self, obj: Persistent<Object<'static>>) {
        self.instances.push(obj);
    }

    pub fn get(&self, index: usize) -> Option<&Persistent<Object<'static>>> {
        self.instances.get(index)
    }

    #[hotpath::measure]
    pub fn update_from_snapshots<'js>(
        &self,
        snapshots: &[Snapshot],
        ctx: &Ctx<'js>,
    ) -> Result<()> {
        for (index, snapshot) in snapshots.iter().enumerate() {
            if let Some(instance) = self.get(index) {
                let extractor = instance.clone().restore(ctx).js(ctx)?;
                let json_text = serde_json::to_string(&snapshot.value)
                    .map_err(|error| {
                        SpecificationError::OtherError(error.to_string())
                    })?;
                let value = ctx.json_parse(json_text).js(ctx)?;
                let update = extractor.get::<_, Value>("update").js(ctx)?;
                let update = update.as_function().ok_or(
                    SpecificationError::OtherError(
                        "update is not callable".to_string(),
                    ),
                )?;
                update
                    .call::<_, Value>((This(extractor.clone()), value))
                    .js(ctx)?;
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
