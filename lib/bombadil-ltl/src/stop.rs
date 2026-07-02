use crate::eval::{Leaning, Residual};
use crate::formula::{Domain, Formula, State};
use crate::violation::Violation;

#[derive(Clone, Debug, PartialEq)]
pub enum StopDefault<D: Domain> {
    True(D::State),
    False(Violation<D>),
}

pub fn stop_default<D: Domain>(
    residual: &Residual<D>,
    time: D::Time,
) -> Option<StopDefault<D>> {
    use Residual::*;
    match residual {
        True(state) => Some(StopDefault::True(state.clone())),
        False(violation) => Some(StopDefault::False(violation.clone())),
        Derived(_, leaning) => match leaning {
            Leaning::AssumeFalse(violation) => {
                Some(StopDefault::False(violation.clone()))
            }
            Leaning::AssumeTrue => Some(StopDefault::True(D::State::default())),
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
            pending,
            ..
        } => {
            let pending = pending
                .iter()
                .filter_map(|residual| stop_default(residual, time))
                .collect::<Vec<_>>();
            stop_and_always_default(subformula, *start, *end, time, &pending)
        }
        OrEventually { pending, .. } => {
            let pending = pending
                .iter()
                .filter_map(|residual| stop_default(residual, time))
                .collect::<Vec<_>>();
            stop_or_eventually_default(&pending)
        }
    }
}

fn stop_and_default<D: Domain>(
    left: &StopDefault<D>,
    right: &StopDefault<D>,
) -> StopDefault<D> {
    use StopDefault::*;
    match (left, right) {
        (True(left_state), True(right_state)) => {
            True(left_state.merge(right_state))
        }
        (True(_), right) => right.clone(),
        (left, True(_)) => left.clone(),
        (False(left), False(right)) => False(Violation::And {
            left: Box::new(left.clone()),
            right: Box::new(right.clone()),
        }),
    }
}

fn stop_or_default<D: Domain>(
    left: &StopDefault<D>,
    right: &StopDefault<D>,
) -> StopDefault<D> {
    use StopDefault::*;
    match (left, right) {
        (True(left_state), True(right_state)) => {
            True(left_state.merge(right_state))
        }
        (True(state), _) => True(state.clone()),
        (_, True(state)) => True(state.clone()),
        (False(left), False(right)) => False(Violation::Or {
            left: Box::new(left.clone()),
            right: Box::new(right.clone()),
        }),
    }
}

fn stop_implies_default<D: Domain>(
    left_formula: &Formula<D>,
    left: &StopDefault<D>,
    right: &StopDefault<D>,
) -> StopDefault<D> {
    use StopDefault::*;
    match (left, right) {
        (False(_), _) => True(D::State::default()),
        (True(state), False(violation)) => False(Violation::Implies {
            left: left_formula.clone(),
            right: Box::new(violation.clone()),
            state: state.clone(),
        }),
        (True(left_state), True(right_state)) => {
            True(left_state.merge(right_state))
        }
    }
}

fn stop_and_always_default<D: Domain>(
    subformula: &Formula<D>,
    start: D::Time,
    end: Option<D::Time>,
    time: D::Time,
    children: &[StopDefault<D>],
) -> Option<StopDefault<D>> {
    use StopDefault::*;
    let mut states_true = Vec::with_capacity(children.len());
    let mut violation_first: Option<Violation<D>> = None;

    for child in children {
        match child {
            True(state) => states_true.push(state),
            False(violation) => {
                violation_first = Some(violation.clone());
                break;
            }
        }
    }

    if let Some(violation) = violation_first {
        Some(StopDefault::False(Violation::Always {
            violation: Box::new(violation),
            subformula: Box::new(subformula.clone()),
            start,
            end,
            time,
        }))
    } else if !children.is_empty() {
        Some(StopDefault::True(
            states_true
                .iter()
                .fold(D::State::default(), |a, b| a.merge(b)),
        ))
    } else {
        None
    }
}

fn stop_or_eventually_default<D: Domain>(
    children: &[StopDefault<D>],
) -> Option<StopDefault<D>> {
    use StopDefault::*;

    let mut last_violation = None;
    for child in children {
        match child {
            True(state) => return Some(True(state.clone())),
            False(violation) => {
                last_violation = Some(violation);
            }
        }
    }
    last_violation.cloned().map(StopDefault::False)
}
