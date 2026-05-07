use crate::formula::{Domain, Formula};

#[derive(Clone, Debug, PartialEq)]
pub enum Violation<D: Domain> {
    False {
        time: D::Time,
        condition: String,
        state: D::State,
    },
    Eventually {
        subformula: Box<Formula<D>>,
        reason: EventuallyViolation<D::Time>,
    },
    Always {
        violation: Box<Violation<D>>,
        subformula: Box<Formula<D>>,
        start: D::Time,
        end: Option<D::Time>,
        time: D::Time,
    },
    And {
        left: Box<Violation<D>>,
        right: Box<Violation<D>>,
    },
    Or {
        left: Box<Violation<D>>,
        right: Box<Violation<D>>,
    },
    Implies {
        left: Formula<D>,
        right: Box<Violation<D>>,
        state: D::State,
    },
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum EventuallyViolation<Time> {
    TimedOut(Time),
    TestEnded,
}

impl<D: Domain> Violation<D> {
    pub fn map_function<
        U: Domain<Time = D::Time, Duration = D::Duration, State = D::State>,
    >(
        &self,
        f: impl Fn(&D::Function) -> U::Function,
    ) -> Violation<U> {
        self.map_function_ref(&f)
    }

    fn map_function_ref<
        U: Domain<Time = D::Time, Duration = D::Duration, State = D::State>,
    >(
        &self,
        f: &impl Fn(&D::Function) -> U::Function,
    ) -> Violation<U> {
        match self {
            Violation::False {
                time,
                condition,
                state,
            } => Violation::False {
                time: *time,
                condition: condition.clone(),
                state: state.clone(),
            },
            Violation::Eventually { subformula, reason } => {
                Violation::Eventually {
                    subformula: Box::new(subformula.map_function_ref(f)),
                    reason: *reason,
                }
            }
            Violation::Always {
                violation,
                subformula,
                start,
                end,
                time,
            } => Violation::Always {
                violation: Box::new(violation.map_function_ref(f)),
                subformula: Box::new(subformula.map_function_ref(f)),
                start: *start,
                end: *end,
                time: *time,
            },
            Violation::And { left, right } => Violation::And {
                left: Box::new(left.map_function_ref(f)),
                right: Box::new(right.map_function_ref(f)),
            },
            Violation::Or { left, right } => Violation::Or {
                left: Box::new(left.map_function_ref(f)),
                right: Box::new(right.map_function_ref(f)),
            },
            Violation::Implies { left, right, state } => Violation::Implies {
                left: left.map_function_ref(f),
                right: Box::new(right.map_function_ref(f)),
                state: state.clone(),
            },
        }
    }
}
