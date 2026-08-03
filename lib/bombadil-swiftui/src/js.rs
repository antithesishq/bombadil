//! Conversion of action templates produced by the JS specification into
//! the driver's [`SwiftUIActionTemplate`].

use std::ops::RangeInclusive;

use anyhow::{Result, ensure};
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
                x: screen_coordinate_range(x)?,
                y: screen_coordinate_range(y)?,
            }),
            JsSwiftUIAction::TypeText(text) => Ok(SwiftUIAction::TypeText {
                text: text.try_into()?,
            }),
            JsSwiftUIAction::PressKey(key) => {
                Ok(SwiftUIAction::PressKey { key })
            }
            JsSwiftUIAction::ScrollUp { x, y, distance } => {
                Ok(SwiftUIAction::ScrollUp {
                    x: screen_coordinate_range(x)?,
                    y: screen_coordinate_range(y)?,
                    distance: scroll_distance_range(distance)?,
                })
            }
            JsSwiftUIAction::ScrollDown { x, y, distance } => {
                Ok(SwiftUIAction::ScrollDown {
                    x: screen_coordinate_range(x)?,
                    y: screen_coordinate_range(y)?,
                    distance: scroll_distance_range(distance)?,
                })
            }
        }
    }
}

/// Global screen coordinates can be negative when a display is arranged
/// above or to the left of the primary display.
fn screen_coordinate_range(value: JsRange) -> Result<RangeInclusive<f64>> {
    fn finite(value: f64, label: &str) -> Result<f64> {
        ensure!(value.is_finite(), "{label} must be finite");
        Ok(value)
    }

    match value {
        JsRange::Fixed(value) => {
            let value = finite(value, "coordinate")?;
            Ok(value..=value)
        }
        JsRange::Range((start, end)) => {
            let start = finite(start, "coordinate range start")?;
            let end = finite(end, "coordinate range end")?;
            ensure!(start <= end, "coordinate range start must be <= end");
            Ok(start..=end)
        }
    }
}

fn scroll_distance_range(value: JsRange) -> Result<RangeInclusive<f64>> {
    let range: RangeInclusive<f64> = value.try_into()?;
    ensure!(
        *range.end() <= f64::from(i32::MAX),
        "scroll distance exceeds the agent's supported range"
    );
    Ok(range)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permits_negative_global_screen_coordinates() {
        let action = SwiftUIActionTemplate::try_from(JsSwiftUIAction::Tap {
            x: JsRange::Range((-200.0, -100.0)),
            y: JsRange::Fixed(-20.0),
        })
        .unwrap();

        let SwiftUIAction::Tap { x, y } = action else {
            panic!("expected tap");
        };
        assert_eq!(x, -200.0..=-100.0);
        assert_eq!(y, -20.0..=-20.0);
    }

    #[test]
    fn rejects_scroll_distances_the_agent_cannot_represent() {
        let result =
            scroll_distance_range(JsRange::Fixed(f64::from(i32::MAX) + 1.0));
        assert!(result.is_err());
    }
}
