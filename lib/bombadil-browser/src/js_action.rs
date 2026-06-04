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
    #[serde(rename_all = "camelCase")]
    MouseDrag {
        from: Point,
        to: Point,
        steps: f64,
        delay_millis: f64,
    },
    #[serde(rename_all = "camelCase")]
    SetViewport {
        width: f64,
        height: f64,
    },
}

impl JsAction {
    pub fn into_browser_action(self) -> anyhow::Result<BrowserAction> {
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
            JsAction::MouseDrag {
                from,
                to,
                steps,
                delay_millis,
            } => {
                if !steps.is_finite()
                    || !(1.0..=255.0).contains(&steps)
                    || steps.fract() != 0.0
                {
                    bail!(
                        "steps must be an integer between 1 and 255, got {}",
                        steps
                    );
                }
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
                BrowserAction::MouseDrag {
                    from,
                    to,
                    steps: steps as u8,
                    delay_millis: delay_millis as u64,
                }
            }
            JsAction::SetViewport { width, height } => {
                for (name, value) in [("width", width), ("height", height)] {
                    if !value.is_finite()
                        || !(1.0..=10_000.0).contains(&value)
                        || value.fract() != 0.0
                    {
                        bail!(
                            "{} must be an integer between 1 and 10000, got {}",
                            name,
                            value
                        );
                    }
                }
                BrowserAction::SetViewport {
                    width: width as u16,
                    height: height as u16,
                }
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
        let browser_action = js_action.into_browser_action().unwrap();
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
        let result = js_action.into_browser_action();
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("between 0 and 255")
        );

        let js_action = JsAction::PressKey { code: 13.5 };
        let result = js_action.into_browser_action();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("integer"));
    }

    #[test]
    fn test_to_browser_action_validates_delay_millis() {
        let js_action = JsAction::TypeText {
            text: "hello".to_string(),
            delay_millis: -10.0,
        };
        let result = js_action.into_browser_action();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("non-negative"));

        let js_action = JsAction::TypeText {
            text: "hello".to_string(),
            delay_millis: f64::NAN,
        };
        let result = js_action.into_browser_action();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("finite"));
    }

    #[test]
    fn test_mouse_drag_round_trip() {
        let json = r#"{"MouseDrag": {"from": {"x": 1.0, "y": 2.0}, "to": {"x": 100.0, "y": 200.0}, "steps": 10.0, "delayMillis": 5.0}}"#;
        let action: JsAction = serde_json::from_str(json).unwrap();
        let browser_action = action.into_browser_action().unwrap();
        match browser_action {
            BrowserAction::MouseDrag {
                from,
                to,
                steps,
                delay_millis,
            } => {
                assert_eq!((from.x, from.y), (1.0, 2.0));
                assert_eq!((to.x, to.y), (100.0, 200.0));
                assert_eq!(steps, 10);
                assert_eq!(delay_millis, 5);
            }
            _ => panic!("expected MouseDrag"),
        }
    }

    #[test]
    fn test_mouse_drag_validates_steps() {
        let make = |steps: f64| JsAction::MouseDrag {
            from: Point { x: 0.0, y: 0.0 },
            to: Point { x: 1.0, y: 1.0 },
            steps,
            delay_millis: 0.0,
        };

        assert!(make(0.0).into_browser_action().is_err());
        assert!(make(256.0).into_browser_action().is_err());
        assert!(make(1.5).into_browser_action().is_err());
        assert!(make(f64::NAN).into_browser_action().is_err());
    }

    #[test]
    fn test_set_viewport_round_trip() {
        let json = r#"{"SetViewport": {"width": 1024.0, "height": 768.0}}"#;
        let action: JsAction = serde_json::from_str(json).unwrap();
        let browser_action = action.into_browser_action().unwrap();
        match browser_action {
            BrowserAction::SetViewport { width, height } => {
                assert_eq!(width, 1024);
                assert_eq!(height, 768);
            }
            _ => panic!("expected SetViewport"),
        }
    }

    #[test]
    fn test_set_viewport_validates_dimensions() {
        let make =
            |width: f64, height: f64| JsAction::SetViewport { width, height };

        assert!(make(0.0, 600.0).into_browser_action().is_err());
        assert!(make(800.0, 0.0).into_browser_action().is_err());
        assert!(make(-1.0, 600.0).into_browser_action().is_err());
        assert!(make(10_001.0, 600.0).into_browser_action().is_err());
        assert!(make(800.5, 600.0).into_browser_action().is_err());
        assert!(make(f64::NAN, 600.0).into_browser_action().is_err());
    }
}
