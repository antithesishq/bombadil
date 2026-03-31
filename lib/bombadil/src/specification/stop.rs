use crate::specification::{
    ltl::{Formula, Leaning, Residual, Time, UniqueSnapshots, Violation},
    verifier::merge_snapshots,
};

#[derive(Clone, Debug, PartialEq)]
pub enum StopDefault<Function> {
    True(UniqueSnapshots),
    False(Violation<Function>),
}

pub fn stop_default<Function: Clone>(
    residual: &Residual<Function>,
    time: Time,
) -> Option<StopDefault<Function>> {
    use Residual::*;
    match residual {
        True(snapshots) => Some(StopDefault::True(snapshots.clone())),
        False(violation) => Some(StopDefault::False(violation.clone())),
        Derived(_, leaning) => match leaning {
            Leaning::AssumeFalse(violation) => {
                Some(StopDefault::False(violation.clone()))
            }
            Leaning::AssumeTrue => {
                Some(StopDefault::True(UniqueSnapshots::new()))
            }
        },
        And { left, right } => stop_default(left, time).and_then(|s1| {
            stop_default(right, time).map(|s2| stop_and_default(&s1, &s2))
        }),
        Or { left, right } => stop_default(left, time).and_then(|s1| {
            stop_default(right, time).map(|s2| stop_or_default(&s1, &s2))
        }),
        Implies {
            left_formula,
            left,
            right,
        } => stop_default(left, time).and_then(|s1| {
            stop_default(right, time)
                .map(|s2| stop_implies_default(left_formula, &s1, &s2))
        }),
        AndAlways {
            subformula,
            start,
            end,
            left,
            right,
        } => stop_default(left, time).and_then(|s1| {
            stop_default(right, time).map(|s2| {
                stop_and_always_default(
                    subformula, *start, *end, time, &s1, &s2,
                )
            })
        }),
        OrEventually { left, right, .. } => {
            stop_default(left, time).and_then(|s1| {
                stop_default(right, time)
                    .map(|s2| stop_or_eventually_default(&s1, &s2))
            })
        }
    }
}

fn stop_and_default<Function: Clone>(
    left: &StopDefault<Function>,
    right: &StopDefault<Function>,
) -> StopDefault<Function> {
    use StopDefault::*;
    match (left, right) {
        (True(left_snapshots), True(right_snapshots)) => {
            True(merge_snapshots(left_snapshots, right_snapshots))
        }
        (True(_), right) => right.clone(),
        (left, True(_)) => left.clone(),
        (False(left), False(right)) => False(Violation::And {
            left: Box::new(left.clone()),
            right: Box::new(right.clone()),
        }),
    }
}

fn stop_or_default<Function: Clone>(
    left: &StopDefault<Function>,
    right: &StopDefault<Function>,
) -> StopDefault<Function> {
    use StopDefault::*;
    match (left, right) {
        (True(left_snapshots), True(right_snapshots)) => {
            True(merge_snapshots(left_snapshots, right_snapshots))
        }
        (True(snapshots), _) => True(snapshots.clone()),
        (_, True(snapshots)) => True(snapshots.clone()),
        (False(left), False(right)) => False(Violation::Or {
            left: Box::new(left.clone()),
            right: Box::new(right.clone()),
        }),
    }
}

fn stop_implies_default<Function: Clone>(
    left_formula: &Formula<Function>,
    left: &StopDefault<Function>,
    right: &StopDefault<Function>,
) -> StopDefault<Function> {
    use StopDefault::*;
    match (left, right) {
        (False(_), _) => True(UniqueSnapshots::new()),
        (True(snapshots), False(violation)) => False(Violation::Implies {
            left: left_formula.clone(),
            right: Box::new(violation.clone()),
            antecedent_snapshots: snapshots.values().cloned().collect(),
        }),
        (True(left_snapshots), True(right_snapshots)) => {
            True(merge_snapshots(left_snapshots, right_snapshots))
        }
    }
}

fn stop_and_always_default<Function: Clone>(
    subformula: &Formula<Function>,
    start: Time,
    end: Option<Time>,
    time: Time,
    left: &StopDefault<Function>,
    right: &StopDefault<Function>,
) -> StopDefault<Function> {
    use StopDefault::*;
    match (left, right) {
        (True(_), right) => right.clone(),
        (False(violation), _) => StopDefault::False(Violation::Always {
            violation: Box::new(violation.clone()),
            subformula: Box::new(subformula.clone()),
            start,
            end,
            time,
        }),
    }
}

fn stop_or_eventually_default<Function: Clone>(
    left: &StopDefault<Function>,
    right: &StopDefault<Function>,
) -> StopDefault<Function> {
    use StopDefault::*;
    match (left, right) {
        (True(snapshots), _) => True(snapshots.clone()),
        (_, True(snapshots)) => True(snapshots.clone()),
        (_, False(right)) => False(right.clone()),
    }
}
