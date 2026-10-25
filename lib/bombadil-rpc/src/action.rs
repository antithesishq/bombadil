use std::collections::BTreeMap;
use std::fmt::Formatter;
use std::hash::{Hash, Hasher};
use std::ops::RangeInclusive;

use anyhow::{Result, ensure};
use bombadil::driver::{ActionTemplate, FromGeneratedAction};
use bombadil::render::{Format, Formatted};
use bombadil::specification::generators::{StringGenerator, range_contains};
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

    fn contains(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Literal(a), Self::Literal(b)) => a == b,
            (Self::Integer(range), Self::Literal(value)) => {
                value.as_u64().is_some_and(|x| range.contains(&x))
            }
            (Self::Integer(a), Self::Integer(b)) => range_contains(a, b),
            (Self::Float(a), Self::Float(b)) => range_contains(a, b),
            (Self::String(a), Self::String(b)) => a.contains(b),
            (Self::Array(a), Self::Array(b)) => {
                a.len() == b.len()
                    && a.iter().zip(b).all(|(a, b)| a.contains(b))
            }
            (Self::Object(a), Self::Object(b)) => {
                a.len() == b.len()
                    && a.iter().all(|(key, a)| {
                        b.get(key).is_some_and(|b| a.contains(b))
                    })
            }
            _ => false,
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
    use hegel::{
        TestCase,
        generators::{booleans, integers, text, vecs},
    };
    use rand::{SeedableRng, rngs::StdRng};
    use serde_json::json;

    use super::*;

    #[hegel::test]
    fn nested_templates_generate_and_accept_only_matching_values(tc: TestCase) {
        let min = tc.draw(integers::<u64>().min_value(1).max_value(1000));
        let max = min + tc.draw(integers::<u64>().max_value(1000));
        let length = tc.draw(integers::<usize>().max_value(8));
        let literal = tc.draw(text().max_size(16));
        let wrappers = tc.draw(vecs(booleans()).max_size(6));
        let wrap = |mut value: json::Value| {
            for &array in &wrappers {
                value = if array {
                    json!([null, value])
                } else {
                    json!({"nested": value})
                };
            }
            value
        };
        let template = ExternalActionTemplate::from_generated(wrap(json!({
            "id": literal,
            "x": { "$generate": { "Integer": [min, max] } },
            "text": { "$generate": { "String": { "Text": [length, length] } } },
            "tags": [1, { "$generate": { "Float": [min as f64 / 4.0, max as f64 / 4.0] } }],
        }))).unwrap();
        let mut rng = StdRng::seed_from_u64(tc.draw(integers()));
        let generated = template.generate(&mut rng);
        let mut leaf = &generated.0;
        for &array in wrappers.iter().rev() {
            leaf = if array { &leaf[1] } else { &leaf["nested"] };
        }
        assert_eq!(leaf["id"], json!(literal));
        assert!((min..=max).contains(&leaf["x"].as_u64().unwrap()));
        assert_eq!(leaf["text"].as_str().unwrap().chars().count(), length);
        assert_eq!(leaf["tags"][0], json!(1));
        assert!(
            (min as f64 / 4.0..=max as f64 / 4.0)
                .contains(&leaf["tags"][1].as_f64().unwrap())
        );
        for x in [min - 1, min, max, max + 1] {
            let value = json!({"id": literal, "x": x, "text": "a".repeat(length), "tags": [1, min as f64 / 4.0]});
            assert_eq!(
                template.accepts(&ExternalAction(wrap(value.clone()))),
                (min..=max).contains(&x)
            );
            for wrong in [json!(null), json!({"x": x}), {
                let mut extra = value.clone();
                extra["extra"] = json!(0);
                extra
            }] {
                assert!(!template.accepts(&ExternalAction(wrap(wrong))));
            }
        }
    }

    #[test]
    fn rejects_generate_leaf_with_other_keys() {
        let result = ExternalActionTemplate::from_generated(json!({
            "$generate": { "Integer": [1, 2] },
            "other": 2,
        }));
        assert!(result.is_err());
    }
}
