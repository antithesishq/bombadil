use serde::Serialize;

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
