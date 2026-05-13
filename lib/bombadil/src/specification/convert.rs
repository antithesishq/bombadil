use serde::Serialize;

use crate::browser::actions::BrowserAction;
use crate::browser::state::Resources;
use crate::geometry::Point;
use crate::specification::domain::{BombadilDomain, Snapshot};
use crate::specification::js::RuntimeFunction;
use bombadil_ltl::formula::Formula;
use bombadil_ltl::violation::{EventuallyViolation, Violation};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PrettyFunction(pub String);

impl std::fmt::Display for PrettyFunction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

pub fn formula_with_pretty_functions(
    formula: &Formula<BombadilDomain<RuntimeFunction>>,
) -> Formula<BombadilDomain<PrettyFunction>> {
    formula.map_function(|f| PrettyFunction(f.pretty.clone()))
}

pub fn violation_with_pretty_functions(
    violation: &Violation<BombadilDomain<RuntimeFunction>>,
) -> Violation<BombadilDomain<PrettyFunction>> {
    violation.map_function(|f| PrettyFunction(f.pretty.clone()))
}

pub trait ToSchema<Output> {
    fn to_schema(&self) -> Output;
}

impl ToSchema<bombadil_schema::Formula>
    for Formula<BombadilDomain<PrettyFunction>>
{
    fn to_schema(&self) -> bombadil_schema::Formula {
        match self {
            Formula::Pure { value, pretty } => bombadil_schema::Formula::Pure {
                value: *value,
                pretty: pretty.clone(),
            },
            Formula::Thunk { function, negated } => {
                bombadil_schema::Formula::Thunk {
                    function: function.0.clone(),
                    negated: *negated,
                }
            }
            Formula::And(left, right) => bombadil_schema::Formula::And(
                Box::new(left.to_schema()),
                Box::new(right.to_schema()),
            ),
            Formula::Or(left, right) => bombadil_schema::Formula::Or(
                Box::new(left.to_schema()),
                Box::new(right.to_schema()),
            ),
            Formula::Implies(left, right) => bombadil_schema::Formula::Implies(
                Box::new(left.to_schema()),
                Box::new(right.to_schema()),
            ),
            Formula::Next(formula) => {
                bombadil_schema::Formula::Next(Box::new(formula.to_schema()))
            }
            Formula::Always(formula, bound) => {
                bombadil_schema::Formula::Always(
                    Box::new(formula.to_schema()),
                    *bound,
                )
            }
            Formula::Eventually(formula, bound) => {
                bombadil_schema::Formula::Eventually(
                    Box::new(formula.to_schema()),
                    *bound,
                )
            }
        }
    }
}

impl ToSchema<bombadil_schema::Violation>
    for Violation<BombadilDomain<PrettyFunction>>
{
    fn to_schema(&self) -> bombadil_schema::Violation {
        match self {
            Violation::False {
                time,
                condition,
                state,
            } => bombadil_schema::Violation::False {
                time: *time,
                condition: condition.clone(),
                snapshots: state.values().map(|s| s.to_schema()).collect(),
            },
            Violation::Eventually { subformula, reason } => {
                bombadil_schema::Violation::Eventually {
                    subformula: Box::new(subformula.to_schema()),
                    reason: reason.to_schema(),
                }
            }
            Violation::Always {
                violation,
                subformula,
                start,
                end,
                time,
            } => bombadil_schema::Violation::Always {
                violation: Box::new(violation.to_schema()),
                subformula: Box::new(subformula.to_schema()),
                start: *start,
                end: *end,
                time: *time,
            },
            Violation::And { left, right } => bombadil_schema::Violation::And {
                left: Box::new(left.to_schema()),
                right: Box::new(right.to_schema()),
            },
            Violation::Or { left, right } => bombadil_schema::Violation::Or {
                left: Box::new(left.to_schema()),
                right: Box::new(right.to_schema()),
            },
            Violation::Implies { left, right, state } => {
                bombadil_schema::Violation::Implies {
                    left: left.to_schema(),
                    right: Box::new(right.to_schema()),
                    antecedent_snapshots: state
                        .values()
                        .map(|s| s.to_schema())
                        .collect(),
                }
            }
        }
    }
}

impl ToSchema<bombadil_schema::EventuallyViolation>
    for EventuallyViolation<bombadil_schema::Time>
{
    fn to_schema(&self) -> bombadil_schema::EventuallyViolation {
        match self {
            EventuallyViolation::TimedOut(time) => {
                bombadil_schema::EventuallyViolation::TimedOut(*time)
            }
            EventuallyViolation::TestEnded => {
                bombadil_schema::EventuallyViolation::TestEnded
            }
        }
    }
}

impl ToSchema<bombadil_schema::Snapshot> for Snapshot {
    fn to_schema(&self) -> bombadil_schema::Snapshot {
        bombadil_schema::Snapshot {
            index: self.index,
            name: self.name.clone(),
            value: self.value.clone(),
            time: self.time,
        }
    }
}

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

pub trait ToInternal<Output> {
    fn to_internal(&self) -> Output;
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
