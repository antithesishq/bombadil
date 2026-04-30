use serde::Serialize;

use crate::specification::{
    js::RuntimeFunction,
    ltl::{EventuallyViolation, Formula, UntilViolation, Violation},
    verifier::Snapshot,
};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PrettyFunction(String);

impl std::fmt::Display for PrettyFunction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Formula<RuntimeFunction> {
    pub fn with_pretty_functions(&self) -> Formula<PrettyFunction> {
        self.map_function(|f| PrettyFunction(f.pretty.clone()))
    }
}

impl Violation<RuntimeFunction> {
    pub fn with_pretty_functions(&self) -> Violation<PrettyFunction> {
        self.map_function(|f| PrettyFunction(f.pretty.clone()))
    }
}

impl Formula<PrettyFunction> {
    pub fn to_api(&self) -> bombadil_schema::Formula {
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
                Box::new(left.to_api()),
                Box::new(right.to_api()),
            ),
            Formula::Or(left, right) => bombadil_schema::Formula::Or(
                Box::new(left.to_api()),
                Box::new(right.to_api()),
            ),
            Formula::Implies(left, right) => bombadil_schema::Formula::Implies(
                Box::new(left.to_api()),
                Box::new(right.to_api()),
            ),
            Formula::Until(left, right, bound) => {
                bombadil_schema::Formula::Until(
                    Box::new(left.to_api()),
                    Box::new(right.to_api()),
                    *bound,
                )
            }
            Formula::Release(left, right, bound) => {
                bombadil_schema::Formula::Release(
                    Box::new(left.to_api()),
                    Box::new(right.to_api()),
                    *bound,
                )
            }
            Formula::Next(formula) => {
                bombadil_schema::Formula::Next(Box::new(formula.to_api()))
            }
            Formula::Always(formula, bound) => {
                bombadil_schema::Formula::Always(
                    Box::new(formula.to_api()),
                    *bound,
                )
            }
            Formula::Eventually(formula, bound) => {
                bombadil_schema::Formula::Eventually(
                    Box::new(formula.to_api()),
                    *bound,
                )
            }
        }
    }
}

impl Violation<PrettyFunction> {
    pub fn to_api(&self) -> bombadil_schema::Violation {
        match self {
            Violation::False {
                time,
                condition,
                snapshots,
            } => bombadil_schema::Violation::False {
                time: *time,
                condition: condition.clone(),
                snapshots: snapshots.iter().map(|s| s.to_api()).collect(),
            },
            Violation::Eventually { subformula, reason } => {
                bombadil_schema::Violation::Eventually {
                    subformula: Box::new(subformula.to_api()),
                    reason: reason.to_api(),
                }
            }
            Violation::Always {
                violation,
                subformula,
                start,
                end,
                time,
            } => bombadil_schema::Violation::Always {
                violation: Box::new(violation.to_api()),
                subformula: Box::new(subformula.to_api()),
                start: *start,
                end: *end,
                time: *time,
            },
            Violation::Until {
                left,
                right,
                start,
                end,
                reason,
            } => bombadil_schema::Violation::Until {
                left: Box::new(left.to_api()),
                right: Box::new(right.to_api()),
                start: *start,
                end: *end,
                reason: reason.to_api(),
            },
            Violation::Release {
                left,
                right,
                start,
                end,
                violation,
            } => bombadil_schema::Violation::Release {
                left: Box::new(left.to_api()),
                right: Box::new(right.to_api()),
                start: *start,
                end: *end,
                violation: Box::new(violation.to_api()),
            },
            Violation::And { left, right } => bombadil_schema::Violation::And {
                left: Box::new(left.to_api()),
                right: Box::new(right.to_api()),
            },
            Violation::Or { left, right } => bombadil_schema::Violation::Or {
                left: Box::new(left.to_api()),
                right: Box::new(right.to_api()),
            },
            Violation::Implies {
                left,
                right,
                antecedent_snapshots,
            } => bombadil_schema::Violation::Implies {
                left: left.to_api(),
                right: Box::new(right.to_api()),
                antecedent_snapshots: antecedent_snapshots
                    .iter()
                    .map(|s| s.to_api())
                    .collect(),
            },
        }
    }
}

impl EventuallyViolation {
    pub fn to_api(&self) -> bombadil_schema::EventuallyViolation {
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

impl UntilViolation<PrettyFunction> {
    pub fn to_api(&self) -> bombadil_schema::UntilViolation {
        match self {
            UntilViolation::Left(violation) => {
                bombadil_schema::UntilViolation::Left(Box::new(
                    violation.to_api(),
                ))
            }
            UntilViolation::TimedOut { time, snapshots } => {
                bombadil_schema::UntilViolation::TimedOut {
                    time: *time,
                    snapshots: snapshots.iter().map(|s| s.to_api()).collect(),
                }
            }
            UntilViolation::TestEnded { snapshots } => {
                bombadil_schema::UntilViolation::TestEnded {
                    snapshots: snapshots.iter().map(|s| s.to_api()).collect(),
                }
            }
        }
    }
}

impl Snapshot {
    pub fn to_api(&self) -> bombadil_schema::Snapshot {
        bombadil_schema::Snapshot {
            index: self.index,
            name: self.name.clone(),
            value: self.value.clone(),
            time: self.time,
        }
    }
}
