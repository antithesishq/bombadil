use std::convert::TryInto;
use std::ops::RangeInclusive;

use anyhow::ensure;
use bombadil::specification::js::{JsRange, JsStringGenerator};
use bombadil_schema::{Rect, browser::Fingerprint};
use serde::{Deserialize, Serialize};

use crate::browser::actions::{BrowserAction, BrowserActionTemplate};
use crate::geometry::Point;

/// TypeScript-friendly action representation with camelCase and f64 for numbers.
/// This matches the JSON that comes from the JavaScript specification layer.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum JsAction {
    Back,
    Forward,
    #[serde(rename_all = "camelCase")]
    Click {
        fingerprint: JsFingerprint,
        point: JsPoint,
    },
    #[serde(rename_all = "camelCase")]
    DoubleClick {
        fingerprint: JsFingerprint,
        point: JsPoint,
        delay_millis: JsRange,
    },
    #[serde(rename_all = "camelCase")]
    TypeText {
        text: JsStringGenerator,
        delay_millis: JsRange,
    },
    #[serde(rename_all = "camelCase")]
    PressKey {
        code: f64,
    },
    #[serde(rename_all = "camelCase")]
    ScrollUp {
        origin: JsPoint,
        distance: JsRange,
    },
    #[serde(rename_all = "camelCase")]
    ScrollDown {
        origin: JsPoint,
        distance: JsRange,
    },
    Reload,
    Wait,
    #[serde(rename_all = "camelCase")]
    SetFileInputFiles {
        selector: String,
        files: Vec<String>,
    },
    #[serde(rename_all = "camelCase")]
    MouseDrag {
        from: JsPoint,
        to: JsPoint,
        steps: JsRange,
        delay_millis: JsRange,
    },
    #[serde(rename_all = "camelCase")]
    SetViewport {
        width: JsRange,
        height: JsRange,
    },
}

impl TryInto<BrowserActionTemplate> for JsAction {
    type Error = anyhow::Error;

    fn try_into(self) -> Result<BrowserActionTemplate, Self::Error> {
        use anyhow::bail;

        Ok(match self {
            JsAction::Back => BrowserAction::Back,
            JsAction::Forward => BrowserAction::Forward,
            JsAction::Reload => BrowserAction::Reload,
            JsAction::Click { fingerprint, point } => BrowserAction::Click {
                fingerprint: fingerprint.try_into()?,
                point: point.try_into()?,
            },
            JsAction::DoubleClick {
                fingerprint,
                point,
                delay_millis,
            } => {
                let delay_millis: RangeInclusive<u64> =
                    delay_millis.try_into()?;
                if *delay_millis.end() > 1000 {
                    bail!(
                        "delayMillis end must be at most 1000, got {:?}",
                        delay_millis
                    );
                }
                BrowserAction::DoubleClick {
                    fingerprint: fingerprint.try_into()?,
                    point: point.try_into()?,
                    delay_millis,
                }
            }
            JsAction::TypeText { text, delay_millis } => {
                let delay_millis: RangeInclusive<u64> =
                    delay_millis.try_into()?;
                BrowserAction::TypeText {
                    text: text.try_into()?,
                    delay_millis,
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
                BrowserAction::ScrollUp {
                    origin: origin.try_into()?,
                    distance: distance.try_into()?,
                }
            }
            JsAction::ScrollDown { origin, distance } => {
                BrowserAction::ScrollDown {
                    origin: origin.try_into()?,
                    distance: distance.try_into()?,
                }
            }
            JsAction::Wait => BrowserAction::Wait,
            JsAction::SetFileInputFiles { selector, files } => {
                BrowserAction::SetFileInputFiles { selector, files }
            }
            JsAction::MouseDrag {
                from,
                to,
                steps,
                delay_millis,
            } => {
                let steps: RangeInclusive<u8> = steps.try_into()?;
                ensure!(
                    *steps.start() > 0,
                    "steps start must be greater than 0, got {:?}",
                    steps
                );
                let delay_millis: RangeInclusive<u64> =
                    delay_millis.try_into()?;
                ensure!(
                    *delay_millis.end() <= 1000,
                    "delayMillis end must be at most 1000, got {:?}",
                    delay_millis
                );
                BrowserAction::MouseDrag {
                    from: from.try_into()?,
                    to: to.try_into()?,
                    steps,
                    delay_millis,
                }
            }
            JsAction::SetViewport { width, height } => {
                let width: RangeInclusive<u16> = width.try_into()?;
                let height: RangeInclusive<u16> = height.try_into()?;
                for (name, value) in [("width", &width), ("height", &height)] {
                    if *value.start() == 0 || *value.end() > 10000 {
                        bail!(
                            "{} must be an integer range within 1..=10000, got {:?}",
                            name,
                            value
                        );
                    }
                }
                BrowserAction::SetViewport { width, height }
            }
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JsFingerprint {
    // Universal strong identifiers
    test_id: Option<String>,
    id: Option<String>,
    role: Option<String>,
    accessible_name: Option<String>,
    tag: String,

    // Type-specific weak identifiers
    href: Option<String>,
    name_attr: Option<String>,
    placeholder: Option<String>,
    input_type: Option<String>,

    // Fallbacks
    text_content: Option<String>,
    structural_path: Option<String>,
}

impl TryInto<Fingerprint> for JsFingerprint {
    type Error = anyhow::Error;

    fn try_into(self) -> Result<Fingerprint, Self::Error> {
        use anyhow::ensure;
        if self.structural_path.is_some() {
            let error_message = "structural_path must not be included when other fingerprint values are present";
            ensure!(self.test_id.is_none(), error_message);
            ensure!(self.id.is_none(), error_message);
            ensure!(self.role.is_none(), error_message);
            ensure!(self.accessible_name.is_none(), error_message);
            ensure!(self.href.is_none(), error_message);
            ensure!(self.name_attr.is_none(), error_message);
            ensure!(self.placeholder.is_none(), error_message);
            ensure!(self.input_type.is_none(), error_message);
            ensure!(self.text_content.is_none(), error_message);
        }
        Ok(Fingerprint {
            test_id: self.test_id.clone(),
            id: self.id.clone(),
            role: self.role.clone(),
            accessible_name: self.accessible_name.clone(),
            tag: self.tag.clone(),
            href: self.href.clone(),
            name_attr: self.name_attr.clone(),
            placeholder: self.placeholder.clone(),
            input_type: self.input_type.clone(),
            text_content: self.text_content.clone(),
            structural_path: self.structural_path.clone(),
        })
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
struct JsRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl TryFrom<JsRect> for Rect {
    type Error = anyhow::Error;

    fn try_from(value: JsRect) -> Result<Self, Self::Error> {
        use anyhow::ensure;

        ensure!(value.x.is_normal());
        ensure!(value.y.is_normal());
        ensure!(value.width.is_normal());
        ensure!(value.height.is_normal());
        ensure!(
            !value.width.is_sign_negative(),
            "width must be non-negative"
        );
        ensure!(
            !value.height.is_sign_negative(),
            "height must be non-negative"
        );
        ensure!(value.width.fract() != 0.0, "width must be an integer");
        ensure!(value.height.fract() != 0.0, "height must be an integer");

        Ok(Rect {
            x: value.x,
            y: value.y,
            width: value.width,
            height: value.height,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JsPoint {
    pub x: JsRange,
    pub y: JsRange,
}

impl TryFrom<JsPoint> for Point<RangeInclusive<f64>> {
    type Error = anyhow::Error;

    fn try_from(value: JsPoint) -> Result<Self, Self::Error> {
        Ok(Point {
            x: value.x.try_into()?,
            y: value.y.try_into()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use anyhow::Result;

    use crate::browser::actions::BrowserAction;

    use super::*;

    #[test]
    fn test_deserialize_js_action_with_float_integers() {
        let json = r#"{"TypeText": {"text": "Email", "delayMillis": 43.0}}"#;
        let action: JsAction = serde_json::from_str(json).unwrap();
        match action {
            JsAction::TypeText {
                delay_millis,
                text: JsStringGenerator::Email,
            } => {
                assert_eq!(delay_millis, JsRange::Fixed(43.0));
            }
            _ => panic!("expected TypeText with Email"),
        }

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
    fn test_to_browser_action_does_not_silently_truncate_floats() {
        let js_action = JsAction::TypeText {
            text: JsStringGenerator::Text(JsRange::Range((0.0, 10.0))),
            delay_millis: JsRange::Fixed(43.9),
        };
        let result: Result<BrowserActionTemplate> = js_action.try_into();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("fractional"));
    }

    #[test]
    fn test_to_browser_action_validates_code_range() {
        let js_action = JsAction::PressKey { code: 256.0 };
        let result: Result<BrowserActionTemplate> = js_action.try_into();
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("between 0 and 255")
        );

        let js_action = JsAction::PressKey { code: 13.5 };
        let result: Result<BrowserActionTemplate> = js_action.try_into();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("integer"));
    }

    #[test]
    fn test_to_browser_action_validates_delay_millis() {
        let js_action = JsAction::TypeText {
            text: JsStringGenerator::Text(JsRange::Range((0.0, 10.0))),
            delay_millis: JsRange::Fixed(-10.0),
        };
        let result: Result<BrowserActionTemplate> = js_action.try_into();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("negative"));

        let js_action = JsAction::TypeText {
            text: JsStringGenerator::Text(JsRange::Range((0.0, 10.0))),
            delay_millis: JsRange::Fixed(f64::NAN),
        };
        let result: Result<BrowserActionTemplate> = js_action.try_into();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("finite"));
    }

    #[test]
    fn test_mouse_drag_round_trip() {
        let json = r#"{"MouseDrag": {"from": {"x": 1.0, "y": [1.0, 2.0]}, "to": {"x": 100.0, "y": [200.0, 300.0]}, "steps": 10.0, "delayMillis": 5.0}}"#;
        let action: JsAction = serde_json::from_str(json).unwrap();
        match action.try_into().unwrap() {
            BrowserAction::MouseDrag {
                from,
                to,
                steps,
                delay_millis,
            } => {
                assert_eq!(from.x.into_inner(), (1.0, 1.0));
                assert_eq!(from.y.into_inner(), (1.0, 2.0));
                assert_eq!(to.x.into_inner(), (100.0, 100.0));
                assert_eq!(to.y.into_inner(), (200.0, 300.0));
                assert_eq!(steps, 10..=10);
                assert_eq!(delay_millis, 5..=5);
            }
            _ => panic!("expected MouseDrag"),
        }
    }

    #[test]
    fn test_mouse_drag_validates_steps() {
        let make = |steps: JsRange| {
            TryInto::<BrowserActionTemplate>::try_into(JsAction::MouseDrag {
                from: JsPoint {
                    x: JsRange::Fixed(0.0),
                    y: JsRange::Fixed(0.0),
                },
                to: JsPoint {
                    x: JsRange::Fixed(1.0),
                    y: JsRange::Fixed(1.0),
                },
                steps,
                delay_millis: JsRange::Fixed(0.0),
            })
        };

        assert!(make(JsRange::Fixed(0.0)).is_err());
        assert!(make(JsRange::Fixed(256.0)).is_err());
        assert!(make(JsRange::Fixed(1.5)).is_err());
        assert!(make(JsRange::Fixed(f64::NAN)).is_err());
    }

    #[test]
    fn test_set_viewport_round_trip() {
        let json = r#"{"SetViewport": {"width": 1024.0, "height": 768.0}}"#;
        let action: JsAction = serde_json::from_str(json).unwrap();
        let browser_action = action.try_into().unwrap();
        match browser_action {
            BrowserAction::SetViewport { width, height } => {
                assert_eq!(width, 1024..=1024);
                assert_eq!(height, 768..=768);
            }
            _ => panic!("expected SetViewport"),
        }
    }

    #[test]
    fn test_set_viewport_validates_dimensions() {
        let make = |width: JsRange, height: JsRange| {
            TryInto::<BrowserActionTemplate>::try_into(JsAction::SetViewport {
                width,
                height,
            })
        };

        assert!(make(JsRange::Fixed(0.0), JsRange::Fixed(600.0)).is_err());
        assert!(make(JsRange::Fixed(800.0), JsRange::Fixed(0.0)).is_err());
        assert!(make(JsRange::Fixed(-1.0), JsRange::Fixed(600.0)).is_err());
        assert!(make(JsRange::Fixed(10_001.0), JsRange::Fixed(600.0)).is_err());
        assert!(make(JsRange::Fixed(800.5), JsRange::Fixed(600.0)).is_err());
        assert!(make(JsRange::Fixed(f64::NAN), JsRange::Fixed(600.0)).is_err());
    }
}
