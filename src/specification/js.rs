use std::collections::HashMap;
use std::time::Duration;

use boa_engine::{
    Context, JsObject, JsValue, Module, js_string, property::PropertyKey,
};

use serde::{Deserialize, Serialize};
use serde_json as json;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::browser::actions::BrowserAction;
use crate::geometry::Point;
use crate::specification::{
    result::{Result, SpecificationError},
    syntax::Syntax,
};

/// TypeScript-friendly action representation with camelCase and f64 for numbers.
/// This matches the JSON that comes from the JavaScript specification layer.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum JsAction {
    Back,
    Forward,
    #[serde(rename_all = "camelCase")]
    Click {
        name: String,
        content: Option<String>,
        point: Point,
    },
    #[serde(rename_all = "camelCase")]
    TypeText {
        text: String,
        delay_millis: f64,
    },
    #[serde(rename_all = "camelCase")]
    PressKey {
        code: f64,
    },
    #[serde(rename_all = "camelCase")]
    ScrollUp {
        origin: Point,
        distance: f64,
    },
    #[serde(rename_all = "camelCase")]
    ScrollDown {
        origin: Point,
        distance: f64,
    },
    Reload,
}

impl JsAction {
    /// Convert from JS-friendly representation to internal browser action type.
    pub fn to_browser_action(self) -> anyhow::Result<BrowserAction> {
        use anyhow::bail;

        Ok(match self {
            JsAction::Back => BrowserAction::Back,
            JsAction::Forward => BrowserAction::Forward,
            JsAction::Reload => BrowserAction::Reload,
            JsAction::Click {
                name,
                content,
                point,
            } => BrowserAction::Click {
                name,
                content,
                point,
            },
            JsAction::TypeText { text, delay_millis } => {
                if !delay_millis.is_finite() || delay_millis < 0.0 {
                    bail!(
                        "delayMillis must be a non-negative finite number, got {}",
                        delay_millis
                    );
                }
                BrowserAction::TypeText {
                    text,
                    delay_millis: delay_millis as u64,
                }
            }
            JsAction::PressKey { code } => {
                if !code.is_finite()
                    || !(0.0..=255.0).contains(&code)
                    || code.fract() != 0.0
                {
                    bail!(
                        "code must be an integer between 0 and 255, got {}",
                        code
                    );
                }
                BrowserAction::PressKey { code: code as u8 }
            }
            JsAction::ScrollUp { origin, distance } => {
                BrowserAction::ScrollUp { origin, distance }
            }
            JsAction::ScrollDown { origin, distance } => {
                BrowserAction::ScrollDown { origin, distance }
            }
        })
    }
}

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
                object.get(js_string!("boundMillis"), context)?,
            )?;
            return Ok(Always(Box::new(subformula), bound));
        }

        if value.instance_of(&bombadil.eventually, context)? {
            let subformula_value =
                object.get(js_string!("subformula"), context)?;
            let subformula =
                Self::from_value(&subformula_value, bombadil, context)?;
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
            runtime_default: get_export("runtimeDefault")?.as_object().ok_or(
                SpecificationError::OtherError(
                    "runtimeDefault is not an object".to_string(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize_js_action_with_float_integers() {
        // TypeText with delayMillis as float (PascalCase variant, camelCase fields)
        let json = r#"{"TypeText": {"text": "hello", "delayMillis": 43.0}}"#;
        let action: JsAction = serde_json::from_str(json).unwrap();
        match action {
            JsAction::TypeText { delay_millis, .. } => {
                assert_eq!(delay_millis, 43.0);
            }
            _ => panic!("expected TypeText"),
        }

        // PressKey with code as float (PascalCase variant, camelCase fields)
        let json = r#"{"PressKey": {"code": 13.0}}"#;
        let action: JsAction = serde_json::from_str(json).unwrap();
        match action {
            JsAction::PressKey { code } => {
                assert_eq!(code, 13.0);
            }
            _ => panic!("expected PressKey"),
        }
    }

    #[test]
    fn test_to_browser_action_truncates_floats() {
        let js_action = JsAction::TypeText {
            text: "hello".to_string(),
            delay_millis: 43.9,
        };
        let browser_action = js_action.to_browser_action().unwrap();
        match browser_action {
            BrowserAction::TypeText { delay_millis, .. } => {
                assert_eq!(delay_millis, 43);
            }
            _ => panic!("expected TypeText"),
        }
    }

    #[test]
    fn test_to_browser_action_validates_code_range() {
        let js_action = JsAction::PressKey { code: 256.0 };
        let result = js_action.to_browser_action();
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("between 0 and 255")
        );

        let js_action = JsAction::PressKey { code: 13.5 };
        let result = js_action.to_browser_action();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("integer"));
    }

    #[test]
    fn test_to_browser_action_validates_delay_millis() {
        let js_action = JsAction::TypeText {
            text: "hello".to_string(),
            delay_millis: -10.0,
        };
        let result = js_action.to_browser_action();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("non-negative"));

        let js_action = JsAction::TypeText {
            text: "hello".to_string(),
            delay_millis: f64::NAN,
        };
        let result = js_action.to_browser_action();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("finite"));
    }
}
