use crate::browser::actions::BrowserAction;
use crate::browser::state::Resources;
use crate::geometry::Point;
pub use bombadil::specification::convert::{
    PrettyFunction, ToInternal, ToSchema, formula_with_pretty_functions,
    violation_with_pretty_functions,
};

impl ToSchema<bombadil_schema::Point> for Point {
    fn to_schema(&self) -> bombadil_schema::Point {
        bombadil_schema::Point {
            x: self.x,
            y: self.y,
        }
    }
}

impl ToSchema<bombadil_schema::Resources> for Resources {
    fn to_schema(&self) -> bombadil_schema::Resources {
        bombadil_schema::Resources {
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

impl ToSchema<bombadil_schema::BrowserAction> for BrowserAction {
    fn to_schema(&self) -> bombadil_schema::BrowserAction {
        match self {
            BrowserAction::Back => bombadil_schema::BrowserAction::Back,
            BrowserAction::Forward => bombadil_schema::BrowserAction::Forward,
            BrowserAction::Click {
                name,
                content,
                point,
            } => bombadil_schema::BrowserAction::Click {
                name: name.clone(),
                content: content.clone(),
                point: point.to_schema(),
            },
            BrowserAction::DoubleClick {
                name,
                content,
                point,
                delay_millis,
            } => bombadil_schema::BrowserAction::DoubleClick {
                name: name.clone(),
                content: content.clone(),
                point: point.to_schema(),
                delay_millis: *delay_millis,
            },
            BrowserAction::TypeText { text, delay_millis } => {
                bombadil_schema::BrowserAction::TypeText {
                    text: text.clone(),
                    delay_millis: *delay_millis,
                }
            }
            BrowserAction::PressKey { code } => {
                bombadil_schema::BrowserAction::PressKey { code: *code }
            }
            BrowserAction::ScrollUp { origin, distance } => {
                bombadil_schema::BrowserAction::ScrollUp {
                    origin: origin.to_schema(),
                    distance: *distance,
                }
            }
            BrowserAction::ScrollDown { origin, distance } => {
                bombadil_schema::BrowserAction::ScrollDown {
                    origin: origin.to_schema(),
                    distance: *distance,
                }
            }
            BrowserAction::Reload => bombadil_schema::BrowserAction::Reload,
            BrowserAction::Wait => bombadil_schema::BrowserAction::Wait,
            BrowserAction::SetFileInputFiles { selector, files } => {
                bombadil_schema::BrowserAction::SetFileInputFiles {
                    selector: selector.clone(),
                    files: files.clone(),
                }
            }
        }
    }
}

impl ToInternal<BrowserAction> for bombadil_schema::BrowserAction {
    fn to_internal(&self) -> BrowserAction {
        match self {
            bombadil_schema::BrowserAction::Back => BrowserAction::Back,
            bombadil_schema::BrowserAction::Forward => BrowserAction::Forward,
            bombadil_schema::BrowserAction::Click {
                name,
                content,
                point,
            } => BrowserAction::Click {
                name: name.clone(),
                content: content.clone(),
                point: point.to_internal(),
            },
            bombadil_schema::BrowserAction::DoubleClick {
                name,
                content,
                point,
                delay_millis,
            } => BrowserAction::DoubleClick {
                name: name.clone(),
                content: content.clone(),
                point: point.to_internal(),
                delay_millis: *delay_millis,
            },
            bombadil_schema::BrowserAction::TypeText { text, delay_millis } => {
                BrowserAction::TypeText {
                    text: text.clone(),
                    delay_millis: *delay_millis,
                }
            }
            bombadil_schema::BrowserAction::PressKey { code } => {
                BrowserAction::PressKey { code: *code }
            }
            bombadil_schema::BrowserAction::ScrollUp { origin, distance } => {
                BrowserAction::ScrollUp {
                    origin: origin.to_internal(),
                    distance: *distance,
                }
            }
            bombadil_schema::BrowserAction::ScrollDown { origin, distance } => {
                BrowserAction::ScrollDown {
                    origin: origin.to_internal(),
                    distance: *distance,
                }
            }
            bombadil_schema::BrowserAction::Reload => BrowserAction::Reload,
            bombadil_schema::BrowserAction::Wait => BrowserAction::Wait,
            bombadil_schema::BrowserAction::SetFileInputFiles {
                selector,
                files,
            } => BrowserAction::SetFileInputFiles {
                selector: selector.clone(),
                files: files.clone(),
            },
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
