use serde::{Deserialize, Serialize};

use crate::browser::actions::BrowserAction;
use crate::geometry::Point;

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
    DoubleClick {
        name: String,
        content: Option<String>,
        point: Point,
        delay_millis: f64,
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
    Wait,
    #[serde(rename_all = "camelCase")]
    SetFileInputFiles {
        selector: String,
        files: Vec<String>,
    },
}

impl JsAction {
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
            JsAction::DoubleClick {
                name,
                content,
                point,
                delay_millis,
            } => {
                if !delay_millis.is_finite() || delay_millis < 0.0 {
                    bail!(
                        "delayMillis must be a non-negative finite number, got {}",
                        delay_millis
                    );
                }
                if delay_millis > 1000.0 {
                    bail!(
                        "delayMillis must be at most 1000, got {}",
                        delay_millis
                    );
                }
                BrowserAction::DoubleClick {
                    name,
                    content,
                    point,
                    delay_millis: delay_millis as u64,
                }
            }
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
            JsAction::Wait => BrowserAction::Wait,
            JsAction::SetFileInputFiles { selector, files } => {
                BrowserAction::SetFileInputFiles { selector, files }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize_js_action_with_float_integers() {
        let json = r#"{"TypeText": {"text": "hello", "delayMillis": 43.0}}"#;
        let action: JsAction = serde_json::from_str(json).unwrap();
        match action {
            JsAction::TypeText { delay_millis, .. } => {
                assert_eq!(delay_millis, 43.0);
            }
            _ => panic!("expected TypeText"),
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
