use crate::specification::{
    bombadil_exports::BombadilExports,
    result::{Result, SpecificationError},
};
use boa_engine::{js_string, Context, JsObject, JsValue};

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum Formula {
    True,
    False,
    Always(Box<Formula>),
    Contextful(JsObject),
}

#[allow(dead_code)]
impl Formula {
    pub fn from_value(
        value: &JsValue,
        bombadil: &BombadilExports,
        context: &mut Context,
    ) -> Result<Self> {
        if let Some(value) = value.as_boolean() {
            return Ok(if value { Self::True } else { Self::False });
        }

        let object =
            value.as_object().ok_or(SpecificationError::ModuleError(
                "extractors is not an object".to_string(),
            ))?;

        if value.instance_of(&bombadil.always, context)? {
            let subformula_value =
                object.get(js_string!("subformula"), context)?;
            let subformula =
                Formula::from_value(&subformula_value, bombadil, context)?;
            return Ok(Formula::Always(Box::new(subformula)));
        }

        if value.instance_of(&bombadil.contextful, context)? {
            return Ok(Self::Contextful(object));
        }

        Err(SpecificationError::ModuleError(format!(
            "can't convert to formula: {}",
            value.display()
        )))
    }
}
