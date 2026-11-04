use std::collections::BTreeMap;
use std::fmt::Formatter;
use std::hash::{Hash, Hasher};
use std::ops::RangeInclusive;

use anyhow::{Result, ensure};
use bombadil::driver::{ActionTemplate, FromGeneratedAction};
use bombadil::render::{Format, Formatted};
use bombadil::specification::generators::StringGenerator;
use bombadil::specification::js::{JsRange, JsStringGenerator};
use rand::RngExt;
use serde::{Deserialize, Serialize};
use serde_json as json;

/// Key marking a generated leaf in templates from the specification.
const GENERATE_KEY: &str = "$generate";

/// A concrete action, sent to the driver as is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ExternalAction(pub json::Value);

impl Format for ExternalAction {
    fn format(&self, f: &mut Formatter) -> Result<(), std::fmt::Error> {
        write!(f, "{}", self.0)
    }
}

/// JSON in which some leaves are generators.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ExternalActionTemplate {
    Literal(json::Value),
    Integer(RangeInclusive<u64>),
    Float(RangeInclusive<f64>),
    String(StringGenerator),
    Array(Vec<ExternalActionTemplate>),
    Object(BTreeMap<String, ExternalActionTemplate>),
}

#[derive(Deserialize)]
enum JsGenerated {
    Integer(JsRange),
    Float(JsRange),
    String(JsStringGenerator),
}

impl FromGeneratedAction for ExternalActionTemplate {
    fn from_generated(value: json::Value) -> Result<Self> {
        Ok(match value {
            json::Value::Object(mut object)
                if object.contains_key(GENERATE_KEY) =>
            {
                ensure!(
                    object.len() == 1,
                    "a `{GENERATE_KEY}` leaf must have no other keys"
                );
                let generated = object.remove(GENERATE_KEY).expect("checked");
                match json::from_value(generated)? {
                    JsGenerated::Integer(range) => {
                        Self::Integer(range.try_into()?)
                    }
                    JsGenerated::Float(range) => Self::Float(range.try_into()?),
                    JsGenerated::String(generator) => {
                        Self::String(generator.try_into()?)
                    }
                }
            }
            json::Value::Object(object) => Self::Object(
                object
                    .into_iter()
                    .map(|(key, value)| Ok((key, Self::from_generated(value)?)))
                    .collect::<Result<_>>()?,
            ),
            json::Value::Array(items) => Self::Array(
                items
                    .into_iter()
                    .map(Self::from_generated)
                    .collect::<Result<_>>()?,
            ),
            literal => Self::Literal(literal),
        })
    }
}

impl ExternalActionTemplate {
    fn generate_value<Rng: rand::TryRng + RngExt>(
        &self,
        rng: &mut Rng,
    ) -> json::Value {
        match self {
            Self::Literal(value) => value.clone(),
            Self::Integer(range) => rng.random_range(range.clone()).into(),
            Self::Float(range) => rng.random_range(range.clone()).into(),
            Self::String(generator) => generator.generate(rng).into(),
            Self::Array(items) => {
                items.iter().map(|item| item.generate_value(rng)).collect()
            }
            Self::Object(fields) => fields
                .iter()
                .map(|(key, value)| (key.clone(), value.generate_value(rng)))
                .collect::<json::Map<_, _>>()
                .into(),
        }
    }

    fn accepts_value(&self, value: &json::Value) -> bool {
        match (self, value) {
            (Self::Literal(literal), value) => literal == value,
            (Self::Integer(range), value) => {
                value.as_u64().is_some_and(|x| range.contains(&x))
            }
            (Self::Float(range), value) => {
                value.as_f64().is_some_and(|x| range.contains(&x))
            }
            (Self::String(generator), json::Value::String(string)) => {
                generator.accepts(string)
            }
            (Self::Array(items), json::Value::Array(values)) => {
                items.len() == values.len()
                    && items
                        .iter()
                        .zip(values)
                        .all(|(item, value)| item.accepts_value(value))
            }
            (Self::Object(fields), json::Value::Object(values)) => {
                fields.len() == values.len()
                    && fields.iter().all(|(key, field)| {
                        values
                            .get(key)
                            .is_some_and(|value| field.accepts_value(value))
                    })
            }
            _ => false,
        }
    }
}

impl ActionTemplate<ExternalAction> for ExternalActionTemplate {
    fn generate<Rng: rand::TryRng + RngExt>(
        &self,
        rng: &mut Rng,
    ) -> ExternalAction {
        ExternalAction(self.generate_value(rng))
    }

    fn accepts(&self, original: &ExternalAction) -> bool {
        self.accepts_value(&original.0)
    }

    fn category_hash<H: Hasher>(&self, hasher: &mut H) {
        match self {
            Self::Literal(value) => ("Literal", value.to_string()).hash(hasher),
            Self::Integer(range) => ("Integer", range).hash(hasher),
            Self::Float(range) => {
                ("Float", range.start().to_bits(), range.end().to_bits())
                    .hash(hasher)
            }
            Self::String(generator) => ("String", generator).hash(hasher),
            Self::Array(items) => {
                ("Array", items.len()).hash(hasher);
                for item in items {
                    item.category_hash(hasher);
                }
            }
            Self::Object(fields) => {
                ("Object", fields.len()).hash(hasher);
                for (key, field) in fields {
                    key.hash(hasher);
                    field.category_hash(hasher);
                }
            }
        }
    }
}

impl Format for ExternalActionTemplate {
    fn format(&self, f: &mut Formatter) -> Result<(), std::fmt::Error> {
        match self {
            Self::Literal(value) => write!(f, "{value}"),
            Self::Integer(range) => write!(f, "<integer {}>", Formatted(range)),
            Self::Float(range) => write!(f, "<float {}>", Formatted(range)),
            Self::String(generator) => generator.format(f),
            Self::Array(items) => {
                write!(f, "[")?;
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        write!(f, ",")?;
                    }
                    item.format(f)?;
                }
                write!(f, "]")
            }
            Self::Object(fields) => {
                write!(f, "{{")?;
                for (index, (key, field)) in fields.iter().enumerate() {
                    if index > 0 {
                        write!(f, ",")?;
                    }
                    write!(f, "{}:", json::Value::from(key.as_str()))?;
                    field.format(f)?;
                }
                write!(f, "}}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use rand::{SeedableRng, rngs::StdRng};
    use serde_json::json;

    use super::*;

    fn template(value: json::Value) -> ExternalActionTemplate {
        ExternalActionTemplate::from_generated(value).unwrap()
    }

    #[test]
    fn generates_values_for_generator_leaves_only() {
        let template = template(json!({
            "Tap": {
                "id": "submit",
                "x": { "$generate": { "Integer": [3, 5] } },
                "text": { "$generate": { "String": { "Text": [2, 2] } } },
                "tags": [1, { "$generate": { "Float": [0.5, 0.75] } }],
            }
        }));
        let mut rng = StdRng::seed_from_u64(0);
        for _ in 0..50 {
            let action = template.generate(&mut rng);
            let tap = &action.0["Tap"];
            assert_eq!(tap["id"], json!("submit"));
            assert!((3..=5).contains(&tap["x"].as_u64().unwrap()));
            assert_eq!(tap["text"].as_str().unwrap().chars().count(), 2);
            assert_eq!(tap["tags"][0], json!(1));
            assert!((0.5..=0.75).contains(&tap["tags"][1].as_f64().unwrap()));
            assert!(template.accepts(&action));
        }
    }

    #[test]
    fn accepts_only_actions_the_template_can_produce() {
        let template = template(json!({
            "Scroll": { "by": { "$generate": { "Integer": [0, 10] } } }
        }));
        let accepts = |value| template.accepts(&ExternalAction(value));
        assert!(accepts(json!({ "Scroll": { "by": 10 } })));
        assert!(!accepts(json!({ "Scroll": { "by": 11 } })));
        assert!(!accepts(json!({ "Scroll": { "by": "1" } })));
        assert!(!accepts(json!({ "Scroll": { "by": 1, "extra": 0 } })));
        assert!(!accepts(json!({ "Scroll": {} })));
        assert!(!accepts(json!({ "Back": { "by": 1 } })));
    }

    #[test]
    fn rejects_generate_leaf_with_other_keys() {
        let result = ExternalActionTemplate::from_generated(json!({
            "$generate": { "Integer": 1 },
            "other": 2,
        }));
        assert!(result.is_err());
    }
}
