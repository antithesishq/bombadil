//! Conversion of action templates produced by the JS specification into
//! the driver's [`SwiftUIActionTemplate`].

use anyhow::Result;
use bombadil::driver::FromGeneratedAction;
use bombadil::specification::generators::{CharSetEntry, StringGenerator};
use bombadil::specification::js::{JsRange, JsStringGenerator};
use serde::{Deserialize, Serialize};
use serde_json as json;

use crate::driver::{SwiftUIAction, SwiftUIActionTemplate};

impl FromGeneratedAction for SwiftUIActionTemplate {
    fn from_generated(value: json::Value) -> Result<Self> {
        let js_action: JsSwiftUIAction = json::from_value(value)?;
        js_action.try_into()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum JsSwiftUIAction {
    #[serde(rename_all = "camelCase")]
    Tap {
        x: JsRange,
        y: JsRange,
    },
    TypeText(JsText),
    PressKey(String),
    #[serde(rename_all = "camelCase")]
    ScrollUp {
        x: JsRange,
        y: JsRange,
        distance: JsRange,
    },
    #[serde(rename_all = "camelCase")]
    ScrollDown {
        x: JsRange,
        y: JsRange,
        distance: JsRange,
    },
}

/// The payload of `TypeText`: either one of the standard string
/// generators or a literal string. The generator is tried first so
/// that `"Email"` means the email generator, not the literal text.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsText {
    Generator(JsStringGenerator),
    Literal(String),
}

impl TryFrom<JsText> for StringGenerator {
    type Error = anyhow::Error;
    fn try_from(value: JsText) -> Result<Self> {
        match value {
            JsText::Literal(text) => Ok(StringGenerator::CharSet {
                entries: vec![CharSetEntry::Literal(text)],
            }),
            JsText::Generator(generator) => generator.try_into(),
        }
    }
}

impl TryFrom<JsSwiftUIAction> for SwiftUIActionTemplate {
    type Error = anyhow::Error;
    fn try_from(value: JsSwiftUIAction) -> Result<Self> {
        match value {
            JsSwiftUIAction::Tap { x, y } => Ok(SwiftUIAction::Tap {
                x: x.try_into()?,
                y: y.try_into()?,
            }),
            JsSwiftUIAction::TypeText(text) => Ok(SwiftUIAction::TypeText {
                text: text.try_into()?,
            }),
            JsSwiftUIAction::PressKey(key) => {
                Ok(SwiftUIAction::PressKey { key })
            }
            JsSwiftUIAction::ScrollUp { x, y, distance } => {
                Ok(SwiftUIAction::ScrollUp {
                    x: x.try_into()?,
                    y: y.try_into()?,
                    distance: distance.try_into()?,
                })
            }
            JsSwiftUIAction::ScrollDown { x, y, distance } => {
                Ok(SwiftUIAction::ScrollDown {
                    x: x.try_into()?,
                    y: y.try_into()?,
                    distance: distance.try_into()?,
                })
            }
        }
    }
}
