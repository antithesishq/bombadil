use crate::browser::actions::BrowserAction;
use crate::browser::state::Resources;
use crate::geometry::Point;
pub use bombadil::specification::convert::{
    PrettyFunction, ToInternal, ToSchema, formula_with_pretty_functions,
    violation_with_pretty_functions,
};
use bombadil_schema::browser;

impl ToSchema<bombadil_schema::Point> for Point {
    fn to_schema(&self) -> bombadil_schema::Point {
        bombadil_schema::Point {
            x: self.x,
            y: self.y,
        }
    }
}

impl ToSchema<browser::Resources> for Resources {
    fn to_schema(&self) -> browser::Resources {
        browser::Resources {
            js_heap_used: self.js_heap_used,
            js_heap_total: self.js_heap_total,
            dom_nodes: self.dom_nodes,
            documents: self.documents,
            js_event_listeners: self.js_event_listeners,
            layout_objects: self.layout_objects,
            timestamp: self.timestamp,
            thread_time: self.thread_time,
            task_duration: self.task_duration,
            script_duration: self.script_duration,
        }
    }
}

impl ToSchema<browser::BrowserAction> for BrowserAction {
    fn to_schema(&self) -> browser::BrowserAction {
        match self {
            BrowserAction::Back => browser::BrowserAction::Back,
            BrowserAction::Forward => browser::BrowserAction::Forward,
            BrowserAction::Click {
                fingerprint,
                point: position,
            } => browser::BrowserAction::Click {
                fingerprint: fingerprint.clone(),
                point: position.to_schema(),
            },
            BrowserAction::DoubleClick {
                fingerprint,
                point: position,
            } => browser::BrowserAction::DoubleClick {
                fingerprint: fingerprint.clone(),
                point: position.to_schema(),
                delay_millis: None,
            },
            BrowserAction::TypeText { text } => {
                browser::BrowserAction::TypeText {
                    text: text.clone(),
                    delay_millis: None,
                }
            }
            BrowserAction::PressKey { code, modifiers } => {
                browser::BrowserAction::PressKey {
                    code: *code,
                    modifiers: *modifiers,
                }
            }
            BrowserAction::ScrollUp { origin, distance } => {
                browser::BrowserAction::ScrollUp {
                    origin: origin.to_schema(),
                    distance: *distance,
                }
            }
            BrowserAction::ScrollDown { origin, distance } => {
                browser::BrowserAction::ScrollDown {
                    origin: origin.to_schema(),
                    distance: *distance,
                }
            }
            BrowserAction::Reload => browser::BrowserAction::Reload,
            BrowserAction::Wait => browser::BrowserAction::Wait,
            BrowserAction::SetFileInputFiles { selector, files } => {
                browser::BrowserAction::SetFileInputFiles {
                    selector: selector.clone(),
                    files: files.clone(),
                }
            }
            BrowserAction::MouseDrag {
                from,
                to,
                steps,
                delay_millis,
            } => browser::BrowserAction::MouseDrag {
                from: from.to_schema(),
                to: to.to_schema(),
                steps: *steps,
                delay_millis: *delay_millis,
            },
            BrowserAction::SetViewport { width, height } => {
                browser::BrowserAction::SetViewport {
                    width: *width,
                    height: *height,
                }
            }
            BrowserAction::Custom { name, arguments } => {
                browser::BrowserAction::Custom {
                    name: name.clone(),
                    arguments: arguments.clone(),
                }
            }
        }
    }
}

impl ToInternal<BrowserAction> for browser::BrowserAction {
    fn to_internal(&self) -> BrowserAction {
        match self {
            browser::BrowserAction::Back => BrowserAction::Back,
            browser::BrowserAction::Forward => BrowserAction::Forward,
            browser::BrowserAction::Click { fingerprint, point } => {
                BrowserAction::Click {
                    fingerprint: fingerprint.clone(),
                    point: point.to_internal(),
                }
            }
            browser::BrowserAction::DoubleClick {
                fingerprint, point, ..
            } => BrowserAction::DoubleClick {
                fingerprint: fingerprint.clone(),
                point: point.to_internal(),
            },
            browser::BrowserAction::TypeText { text, .. } => {
                BrowserAction::TypeText { text: text.clone() }
            }
            browser::BrowserAction::PressKey { code, modifiers } => {
                BrowserAction::PressKey {
                    code: *code,
                    modifiers: *modifiers,
                }
            }
            browser::BrowserAction::ScrollUp { origin, distance } => {
                BrowserAction::ScrollUp {
                    origin: origin.to_internal(),
                    distance: *distance,
                }
            }
            browser::BrowserAction::ScrollDown { origin, distance } => {
                BrowserAction::ScrollDown {
                    origin: origin.to_internal(),
                    distance: *distance,
                }
            }
            browser::BrowserAction::Reload => BrowserAction::Reload,
            browser::BrowserAction::Wait => BrowserAction::Wait,
            browser::BrowserAction::SetFileInputFiles { selector, files } => {
                BrowserAction::SetFileInputFiles {
                    selector: selector.clone(),
                    files: files.clone(),
                }
            }
            browser::BrowserAction::MouseDrag {
                from,
                to,
                steps,
                delay_millis,
            } => BrowserAction::MouseDrag {
                from: from.to_internal(),
                to: to.to_internal(),
                steps: *steps,
                delay_millis: *delay_millis,
            },
            browser::BrowserAction::SetViewport { width, height } => {
                BrowserAction::SetViewport {
                    width: *width,
                    height: *height,
                }
            }
            browser::BrowserAction::Custom { name, arguments } => {
                BrowserAction::Custom {
                    name: name.clone(),
                    arguments: arguments.clone(),
                }
            }
        }
    }
}

impl ToInternal<Point> for bombadil_schema::Point {
    fn to_internal(&self) -> Point {
        Point {
            x: self.x,
            y: self.y,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bombadil_schema::browser::KeyModifiers;

    #[test]
    fn press_key_modifiers_survive_schema_conversion() {
        let modifiers = KeyModifiers {
            ctrl: true,
            shift: true,
            ..KeyModifiers::default()
        };
        let action = BrowserAction::PressKey {
            code: 77,
            modifiers,
        };
        let schema = action.to_schema();
        assert_eq!(
            schema,
            browser::BrowserAction::PressKey {
                code: 77,
                modifiers
            }
        );
        assert_eq!(schema.to_internal(), action);
    }
}
