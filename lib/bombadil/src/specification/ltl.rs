use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use crate::specification::result::{Result, SpecificationError};
use crate::specification::verifier::{Snapshot, merge_snapshots};
use serde::Serialize;

fn combine_options<T: Clone>(
    left: Option<T>,
    right: Option<T>,
    combine: fn(T, T) -> T,
) -> Option<T> {
    match (left, right) {
        (Some(left), Some(right)) => Some(combine(left, right)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

/// A formula in negation normal form (NNF), up to thunks. Note that `Implies` is preserved for
/// better error messages.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum Formula<Function> {
    Pure {
        value: bool,
        pretty: String,
    },
    Thunk {
        function: Function,
        negated: bool,
    },
    And(Box<Formula<Function>>, Box<Formula<Function>>),
    Or(Box<Formula<Function>>, Box<Formula<Function>>),
    Implies(Box<Formula<Function>>, Box<Formula<Function>>),
    Until(
        Box<Formula<Function>>,
        Box<Formula<Function>>,
        Option<Duration>,
    ),
    Release(
        Box<Formula<Function>>,
        Box<Formula<Function>>,
        Option<Duration>,
    ),
    Next(Box<Formula<Function>>),
    Always(Box<Formula<Function>>, Option<Duration>),
    Eventually(Box<Formula<Function>>, Option<Duration>),
}

impl<Function: Clone> Formula<Function> {
    pub fn map_function<Result>(
        &self,
        f: impl Fn(&Function) -> Result,
    ) -> Formula<Result> {
        self.map_function_ref(&f)
    }

    fn map_function_ref<Result>(
        &self,
        f: &impl Fn(&Function) -> Result,
    ) -> Formula<Result> {
        match self {
            Formula::Pure { value, pretty } => Formula::Pure {
                value: *value,
                pretty: pretty.clone(),
            },
            Formula::Thunk { function, negated } => Formula::Thunk {
                function: f(function),
                negated: *negated,
            },
            Formula::And(left, right) => Formula::And(
                Box::new(left.clone().map_function_ref(f)),
                Box::new(right.clone().map_function_ref(f)),
            ),
            Formula::Or(left, right) => Formula::Or(
                Box::new(left.clone().map_function_ref(f)),
                Box::new(right.clone().map_function_ref(f)),
            ),
            Formula::Implies(left, right) => Formula::Implies(
                Box::new(left.clone().map_function_ref(f)),
                Box::new(right.clone().map_function_ref(f)),
            ),
            Formula::Until(left, right, bound) => Formula::Until(
                Box::new(left.clone().map_function_ref(f)),
                Box::new(right.clone().map_function_ref(f)),
                *bound,
            ),
            Formula::Release(left, right, bound) => Formula::Release(
                Box::new(left.clone().map_function_ref(f)),
                Box::new(right.clone().map_function_ref(f)),
                *bound,
            ),
            Formula::Next(formula) => {
                Formula::Next(Box::new(formula.clone().map_function_ref(f)))
            }
            Formula::Always(formula, bound) => Formula::Always(
                Box::new(formula.clone().map_function_ref(f)),
                *bound,
            ),
            Formula::Eventually(formula, bound) => Formula::Eventually(
                Box::new(formula.clone().map_function_ref(f)),
                *bound,
            ),
        }
    }
}

pub type Time = SystemTime;

pub type UniqueSnapshots = BTreeMap<usize, Snapshot>;

#[derive(Clone, Debug, PartialEq)]
pub enum Value<Function> {
    True(UniqueSnapshots),
    False(Violation<Function>, Option<Residual<Function>>),
    Residual(Residual<Function>),
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum Violation<Function> {
    False {
        time: Time,
        condition: String,
        snapshots: Vec<Snapshot>,
    },
    Eventually {
        subformula: Box<Formula<Function>>,
        reason: EventuallyViolation,
    },
    Always {
        violation: Box<Violation<Function>>,
        subformula: Box<Formula<Function>>,
        start: Time,
        end: Option<Time>,
        time: Time,
    },
    Until {
        left: Box<Formula<Function>>,
        right: Box<Formula<Function>>,
        bound: Option<Duration>,
        reason: UntilViolation<Function>,
    },
    Release {
        left: Box<Formula<Function>>,
        right: Box<Formula<Function>>,
        bound: Option<Duration>,
        violation: Box<Violation<Function>>,
    },
    And {
        left: Box<Violation<Function>>,
        right: Box<Violation<Function>>,
    },
    Or {
        left: Box<Violation<Function>>,
        right: Box<Violation<Function>>,
    },
    Implies {
        left: Formula<Function>,
        right: Box<Violation<Function>>,
        antecedent_snapshots: Vec<Snapshot>,
    },
}

#[derive(Copy, Clone, Debug, PartialEq, Serialize)]
pub enum EventuallyViolation {
    TimedOut(Time),
    TestEnded,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum UntilViolation<Function> {
    Left(Box<Violation<Function>>),
    TimedOut(Time),
    TestEnded,
}

impl<Function: Clone> Violation<Function> {
    pub fn map_function<Result>(
        &self,
        f: impl Fn(&Function) -> Result,
    ) -> Violation<Result> {
        self.map_function_ref(&f)
    }

    fn map_function_ref<Result>(
        &self,
        f: &impl Fn(&Function) -> Result,
    ) -> Violation<Result> {
        match self {
            Violation::False {
                time,
                condition,
                snapshots,
            } => Violation::False {
                time: *time,
                condition: condition.clone(),
                snapshots: snapshots.clone(),
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
            Violation::Until {
                left,
                right,
                bound,
                reason,
            } => Violation::Until {
                left: Box::new(left.map_function_ref(f)),
                right: Box::new(right.map_function_ref(f)),
                bound: *bound,
                reason: reason.map_function_ref(f),
            },
            Violation::Release {
                left,
                right,
                bound,
                violation,
            } => Violation::Release {
                left: Box::new(left.map_function_ref(f)),
                right: Box::new(right.map_function_ref(f)),
                bound: *bound,
                violation: Box::new(violation.map_function_ref(f)),
            },
            Violation::And { left, right } => Violation::And {
                left: Box::new(left.map_function_ref(f)),
                right: Box::new(right.map_function_ref(f)),
            },
            Violation::Or { left, right } => Violation::Or {
                left: Box::new(left.map_function_ref(f)),
                right: Box::new(right.map_function_ref(f)),
            },
            Violation::Implies {
                left,
                right,
                antecedent_snapshots,
            } => Violation::Implies {
                left: left.map_function_ref(f),
                right: Box::new(right.map_function_ref(f)),
                antecedent_snapshots: antecedent_snapshots.clone(),
            },
        }
    }
}

impl<Function: Clone> UntilViolation<Function> {
    fn map_function_ref<Result>(
        &self,
        f: &impl Fn(&Function) -> Result,
    ) -> UntilViolation<Result> {
        match self {
            UntilViolation::Left(violation) => {
                UntilViolation::Left(Box::new(violation.map_function_ref(f)))
            }
            UntilViolation::TimedOut(time) => UntilViolation::TimedOut(*time),
            UntilViolation::TestEnded => UntilViolation::TestEnded,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Leaning<Function> {
    AssumeTrue,
    AssumeFalse(Violation<Function>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Residual<Function> {
    True(UniqueSnapshots),
    False(Violation<Function>),
    Derived(Derived<Function>, Leaning<Function>),
    And {
        left: Box<Residual<Function>>,
        right: Box<Residual<Function>>,
    },
    Or {
        left: Box<Residual<Function>>,
        right: Box<Residual<Function>>,
    },
    OrUntil {
        end: Option<Time>,
        left: Box<Residual<Function>>,
        right: Box<Residual<Function>>,
    },
    OrRelease {
        left: Box<Residual<Function>>,
        right: Box<Residual<Function>>,
    },
    Implies {
        left_formula: Formula<Function>,
        left: Box<Residual<Function>>,
        right: Box<Residual<Function>>,
    },
    OrEventually {
        subformula: Box<Formula<Function>>,
        start: Time,
        end: Option<Time>,
        left: Box<Residual<Function>>,
        right: Box<Residual<Function>>,
    },
    AndAlways {
        subformula: Box<Formula<Function>>,
        start: Time,
        end: Option<Time>,
        /// When the left-side residual was first created. Used as
        /// the violation time in the Always wrapper so that "but
        /// at T" reflects when the subformula first started
        /// failing, not when the failure was confirmed.
        onset: Time,
        left: Box<Residual<Function>>,
        right: Box<Residual<Function>>,
    },
    AndUntil {
        left_formula: Box<Formula<Function>>,
        right_formula: Box<Formula<Function>>,
        bound: Option<Duration>,
        end: Option<Time>,
        left: Box<Residual<Function>>,
        right: Box<Residual<Function>>,
    },
    AndRelease {
        left_formula: Box<Formula<Function>>,
        right_formula: Box<Formula<Function>>,
        bound: Option<Duration>,
        end: Option<Time>,
        left: Box<Residual<Function>>,
        right: Box<Residual<Function>>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Derived<Function> {
    Once {
        start: Time,
        subformula: Box<Formula<Function>>,
    },
    Until {
        left_formula: Box<Formula<Function>>,
        right_formula: Box<Formula<Function>>,
        bound: Option<Duration>,
        end: Option<Time>,
    },
    Release {
        left_formula: Box<Formula<Function>>,
        right_formula: Box<Formula<Function>>,
        bound: Option<Duration>,
        end: Option<Time>,
    },
    Always {
        start: Time,
        end: Option<Time>,
        subformula: Box<Formula<Function>>,
    },
    Eventually {
        start: Time,
        end: Option<Time>,
        subformula: Box<Formula<Function>>,
    },
}

pub type EvaluateThunk<'a, Function> =
    &'a mut dyn FnMut(
        &'_ Function,
        bool,
    ) -> Result<(Formula<Function>, UniqueSnapshots)>;

pub struct Evaluator<'a, Function> {
    evaluate_thunk: EvaluateThunk<'a, Function>,
}

impl<'a, Function: Clone> Evaluator<'a, Function> {
    pub fn new(evaluate_thunk: EvaluateThunk<'a, Function>) -> Self {
        Evaluator { evaluate_thunk }
    }

    fn end_from_bound(
        bound: Option<Duration>,
        time: Time,
    ) -> Result<Option<Time>> {
        if let Some(duration) = bound {
            Ok(Some(time.checked_add(duration).ok_or(
                SpecificationError::OtherError(
                    "failed to add bound to time".to_string(),
                ),
            )?))
        } else {
            Ok(None)
        }
    }

    fn until_test_ended_violation(
        left_formula: &Formula<Function>,
        right_formula: &Formula<Function>,
        bound: Option<Duration>,
    ) -> Violation<Function> {
        Violation::Until {
            left: Box::new(left_formula.clone()),
            right: Box::new(right_formula.clone()),
            bound,
            reason: UntilViolation::TestEnded,
        }
    }

    fn until_timed_out_violation(
        left_formula: &Formula<Function>,
        right_formula: &Formula<Function>,
        bound: Option<Duration>,
        time: Time,
    ) -> Violation<Function> {
        Violation::Until {
            left: Box::new(left_formula.clone()),
            right: Box::new(right_formula.clone()),
            bound,
            reason: UntilViolation::TimedOut(time),
        }
    }

    fn until_left_violation(
        left_formula: &Formula<Function>,
        right_formula: &Formula<Function>,
        bound: Option<Duration>,
        violation: Violation<Function>,
    ) -> Violation<Function> {
        Violation::Until {
            left: Box::new(left_formula.clone()),
            right: Box::new(right_formula.clone()),
            bound,
            reason: UntilViolation::Left(Box::new(violation)),
        }
    }

    fn release_right_violation(
        left_formula: &Formula<Function>,
        right_formula: &Formula<Function>,
        bound: Option<Duration>,
        violation: Violation<Function>,
    ) -> Violation<Function> {
        Violation::Release {
            left: Box::new(left_formula.clone()),
            right: Box::new(right_formula.clone()),
            bound,
            violation: Box::new(violation),
        }
    }

    pub fn evaluate(
        &mut self,
        formula: &Formula<Function>,
        time: Time,
    ) -> Result<Value<Function>> {
        match formula {
            Formula::Pure { value, pretty } => Ok(if *value {
                Value::True(UniqueSnapshots::new())
            } else {
                Value::False(
                    Violation::False {
                        time,
                        condition: pretty.clone(),
                        snapshots: vec![],
                    },
                    None,
                )
            }),
            Formula::Thunk { function, negated } => {
                let (formula, snapshots) =
                    (self.evaluate_thunk)(function, *negated)?;
                let mut value = self.evaluate(&formula, time)?;
                attach_snapshots(&mut value, snapshots);
                Ok(value)
            }
            Formula::And(left, right) => {
                let left = self.evaluate(left.as_ref(), time)?;
                let right = self.evaluate(right.as_ref(), time)?;
                Ok(self.evaluate_and(&left, &right))
            }
            Formula::Or(left, right) => {
                let left = self.evaluate(left.as_ref(), time)?;
                let right = self.evaluate(right.as_ref(), time)?;
                Ok(self.evaluate_or(&left, &right))
            }
            Formula::Implies(left_formula, right) => {
                let left = self.evaluate(left_formula.as_ref(), time)?;
                let right = self.evaluate(right.as_ref(), time)?;
                Ok(self.evaluate_implies(left_formula, &left, &right))
            }
            Formula::Until(left_formula, right_formula, bound) => {
                let end = Self::end_from_bound(*bound, time)?;
                self.evaluate_until(
                    left_formula.clone(),
                    right_formula.clone(),
                    *bound,
                    end,
                    time,
                )
            }
            Formula::Release(left_formula, right_formula, bound) => {
                let end = Self::end_from_bound(*bound, time)?;
                self.evaluate_release(
                    left_formula.clone(),
                    right_formula.clone(),
                    *bound,
                    end,
                    time,
                )
            }
            Formula::Next(formula) => Ok(Value::Residual(Residual::Derived(
                Derived::Once {
                    start: time,
                    subformula: formula.clone(),
                },
                Leaning::AssumeTrue, // TODO: expose true/false leaning in TS layer?
            ))),
            Formula::Always(formula, bound) => {
                let end = Self::end_from_bound(*bound, time)?;
                self.evaluate_always(formula.clone(), time, end, time)
            }
            Formula::Eventually(formula, bound) => {
                let end = Self::end_from_bound(*bound, time)?;
                self.evaluate_eventually(formula.clone(), time, end, time)
            }
        }
    }

    fn evaluate_and(
        &mut self,
        left: &Value<Function>,
        right: &Value<Function>,
    ) -> Value<Function> {
        fn combine_and<F: Clone>(
            left: Residual<F>,
            right: Residual<F>,
        ) -> Residual<F> {
            Residual::And {
                left: Box::new(left),
                right: Box::new(right),
            }
        }

        match (left, right) {
            (Value::True(snapshots_left), Value::True(snapshots_right)) => {
                Value::True(merge_snapshots(snapshots_left, snapshots_right))
            }
            (Value::True(snapshots), Value::Residual(residual)) => {
                Value::Residual(combine_and(
                    Residual::True(snapshots.clone()),
                    residual.clone(),
                ))
            }
            (Value::Residual(residual), Value::True(snapshots)) => {
                Value::Residual(combine_and(
                    residual.clone(),
                    Residual::True(snapshots.clone()),
                ))
            }
            (Value::True(_), right) => right.clone(),
            (left, Value::True(_)) => left.clone(),
            (
                Value::False(violation_left, residual_left),
                Value::False(violation_right, residual_right),
            ) => Value::False(
                Violation::And {
                    left: Box::new(violation_left.clone()),
                    right: Box::new(violation_right.clone()),
                },
                combine_options(
                    residual_left.clone(),
                    residual_right.clone(),
                    combine_and,
                ),
            ),
            (
                Value::Residual(residual),
                Value::False(violation, continuation),
            )
            | (
                Value::False(violation, continuation),
                Value::Residual(residual),
            ) => {
                let continuation = match continuation {
                    Some(continuation) => {
                        combine_and(residual.clone(), continuation.clone())
                    }
                    None => residual.clone(),
                };
                Value::False(violation.clone(), Some(continuation))
            }
            (Value::Residual(left), Value::Residual(right)) => {
                Value::Residual(combine_and(left.clone(), right.clone()))
            }
        }
    }

    fn evaluate_or(
        &mut self,
        left: &Value<Function>,
        right: &Value<Function>,
    ) -> Value<Function> {
        match (left, right) {
            (
                Value::False(violation_left, residual_left),
                Value::False(violation_right, residual_right),
            ) => Value::False(
                Violation::Or {
                    left: Box::new(violation_left.clone()),
                    right: Box::new(violation_right.clone()),
                },
                combine_options(
                    residual_left.clone(),
                    residual_right.clone(),
                    |left, right| Residual::Or {
                        left: Box::new(left),
                        right: Box::new(right),
                    },
                ),
            ),
            (Value::True(snapshots_left), Value::True(snapshots_right)) => {
                Value::True(merge_snapshots(snapshots_left, snapshots_right))
            }
            (Value::True(references), _) => Value::True(references.clone()),
            (_, Value::True(references)) => Value::True(references.clone()),
            (left, Value::False(_, _)) => left.clone(),
            (Value::False(_, _), right) => right.clone(),
            (Value::Residual(left), Value::Residual(right)) => {
                Value::Residual(Residual::Or {
                    left: Box::new(left.clone()),
                    right: Box::new(right.clone()),
                })
            }
        }
    }

    fn evaluate_implies(
        &mut self,
        left_formula: &Formula<Function>,
        left: &Value<Function>,
        right: &Value<Function>,
    ) -> Value<Function> {
        match (left, right) {
            (Value::False(_, _), _) => Value::True(UniqueSnapshots::new()),
            (
                Value::True(snapshots_left),
                Value::False(violation, continuation),
            ) => Value::False(
                Violation::Implies {
                    left: left_formula.clone(),
                    right: Box::new(violation.clone()),
                    antecedent_snapshots: snapshots_left
                        .values()
                        .cloned()
                        .collect(),
                },
                continuation.as_ref().map(|c| Residual::Implies {
                    left_formula: left_formula.clone(),
                    left: Box::new(Residual::True(snapshots_left.clone())),
                    right: Box::new(c.clone()),
                }),
            ),
            (Value::True(snapshots_left), Value::True(snapshots_right)) => {
                Value::True(merge_snapshots(snapshots_left, snapshots_right))
            }
            (Value::True(snapshots_left), Value::Residual(right)) => {
                Value::Residual(Residual::Implies {
                    left_formula: left_formula.clone(),
                    left: Box::new(Residual::True(snapshots_left.clone())),
                    right: Box::new(right.clone()),
                })
            }
            (Value::Residual(_), Value::True(references)) => {
                Value::True(references.clone())
            }
            (Value::Residual(left), Value::False(violation, _)) => {
                Value::Residual(Residual::Implies {
                    left_formula: left_formula.clone(),
                    left: Box::new(left.clone()),
                    right: Box::new(Residual::False(violation.clone())),
                })
            }
            (Value::Residual(left), Value::Residual(right)) => {
                Value::Residual(Residual::Implies {
                    left_formula: left_formula.clone(),
                    left: Box::new(left.clone()),
                    right: Box::new(right.clone()),
                })
            }
        }
    }

    fn evaluate_until(
        &mut self,
        left_formula: Box<Formula<Function>>,
        right_formula: Box<Formula<Function>>,
        bound: Option<Duration>,
        end: Option<Time>,
        time: Time,
    ) -> Result<Value<Function>> {
        if let Some(end) = end
            && end < time
        {
            return Ok(Value::False(
                Self::until_timed_out_violation(
                    left_formula.as_ref(),
                    right_formula.as_ref(),
                    bound,
                    time,
                ),
                None,
            ));
        }

        let right = self.evaluate(&right_formula, time)?;
        if let Value::True(references) = right {
            return Ok(Value::True(references));
        }

        let left = self.evaluate(&left_formula, time)?;
        let recursive_until = Residual::Derived(
            Derived::Until {
                left_formula: left_formula.clone(),
                right_formula: right_formula.clone(),
                bound,
                end,
            },
            Leaning::AssumeFalse(Self::until_test_ended_violation(
                left_formula.as_ref(),
                right_formula.as_ref(),
                bound,
            )),
        );

        Ok(match (left, right) {
            (Value::True(_), Value::False(_, _)) => {
                Value::Residual(recursive_until)
            }
            (Value::False(violation, _), Value::False(_, _)) => Value::False(
                Self::until_left_violation(
                    left_formula.as_ref(),
                    right_formula.as_ref(),
                    bound,
                    violation,
                ),
                None,
            ),
            (Value::Residual(left), Value::False(_, _)) => {
                Value::Residual(Residual::AndUntil {
                    left_formula,
                    right_formula,
                    bound,
                    end,
                    left: Box::new(left),
                    right: Box::new(recursive_until),
                })
            }
            (Value::True(_), Value::Residual(right)) => {
                Value::Residual(Residual::OrUntil {
                    end,
                    left: Box::new(right),
                    right: Box::new(recursive_until),
                })
            }
            (Value::False(violation, _), Value::Residual(right)) => {
                Value::Residual(Residual::OrUntil {
                    end,
                    left: Box::new(right),
                    right: Box::new(Residual::False(
                        Self::until_left_violation(
                            left_formula.as_ref(),
                            right_formula.as_ref(),
                            bound,
                            violation,
                        ),
                    )),
                })
            }
            (Value::Residual(left), Value::Residual(right)) => {
                let fallback = Residual::AndUntil {
                    left_formula: left_formula.clone(),
                    right_formula: right_formula.clone(),
                    bound,
                    end,
                    left: Box::new(left),
                    right: Box::new(recursive_until),
                };
                Value::Residual(Residual::OrUntil {
                    end,
                    left: Box::new(right),
                    right: Box::new(fallback),
                })
            }
            _ => unreachable!(),
        })
    }

    fn evaluate_or_until(
        &mut self,
        end: Option<Time>,
        left: Value<Function>,
        right: Value<Function>,
    ) -> Value<Function> {
        match (left, right) {
            (Value::True(snapshots_left), Value::True(snapshots_right)) => {
                Value::True(merge_snapshots(&snapshots_left, &snapshots_right))
            }
            (Value::True(references), _) => Value::True(references),
            (_, Value::True(references)) => Value::True(references),
            (Value::False(_, _), Value::False(right, _)) => {
                Value::False(right, None)
            }
            (Value::False(_, _), Value::Residual(residual)) => {
                Value::Residual(residual)
            }
            (Value::Residual(residual), Value::False(_, _)) => {
                Value::Residual(residual)
            }
            (Value::Residual(left), Value::Residual(right)) => {
                Value::Residual(Residual::OrUntil {
                    end,
                    left: Box::new(left),
                    right: Box::new(right),
                })
            }
        }
    }

    fn evaluate_and_until(
        &mut self,
        left_formula: Box<Formula<Function>>,
        right_formula: Box<Formula<Function>>,
        bound: Option<Duration>,
        end: Option<Time>,
        left: Value<Function>,
        right: Value<Function>,
    ) -> Value<Function> {
        match (left, right) {
            (Value::True(snapshots_left), Value::True(snapshots_right)) => {
                Value::True(merge_snapshots(&snapshots_left, &snapshots_right))
            }
            (Value::Residual(left), Value::True(snapshots)) => {
                Value::Residual(Residual::AndUntil {
                    left_formula,
                    right_formula,
                    bound,
                    end,
                    left: Box::new(left),
                    right: Box::new(Residual::True(snapshots)),
                })
            }
            (Value::True(snapshots), Value::Residual(right)) => {
                Value::Residual(Residual::AndUntil {
                    left_formula,
                    right_formula,
                    bound,
                    end,
                    left: Box::new(Residual::True(snapshots)),
                    right: Box::new(right),
                })
            }
            (Value::Residual(left), Value::Residual(right)) => {
                Value::Residual(Residual::AndUntil {
                    left_formula,
                    right_formula,
                    bound,
                    end,
                    left: Box::new(left),
                    right: Box::new(right),
                })
            }
            (Value::False(violation, _), _) => Value::False(
                Self::until_left_violation(
                    left_formula.as_ref(),
                    right_formula.as_ref(),
                    bound,
                    violation,
                ),
                None,
            ),
            (_, Value::False(violation, _)) => Value::False(violation, None),
        }
    }

    fn evaluate_release(
        &mut self,
        left_formula: Box<Formula<Function>>,
        right_formula: Box<Formula<Function>>,
        bound: Option<Duration>,
        end: Option<Time>,
        time: Time,
    ) -> Result<Value<Function>> {
        if let Some(end) = end
            && end < time
        {
            return Ok(Value::True(UniqueSnapshots::new()));
        }

        let right = self.evaluate(&right_formula, time)?;
        let recursive_release = Residual::Derived(
            Derived::Release {
                left_formula: left_formula.clone(),
                right_formula: right_formula.clone(),
                bound,
                end,
            },
            Leaning::AssumeTrue,
        );

        Ok(match right {
            Value::False(violation, _) => Value::False(
                Self::release_right_violation(
                    left_formula.as_ref(),
                    right_formula.as_ref(),
                    bound,
                    violation,
                ),
                None,
            ),
            Value::True(right_snapshots) => {
                match self.evaluate(&left_formula, time)? {
                    Value::True(left_snapshots) => Value::True(
                        merge_snapshots(&left_snapshots, &right_snapshots),
                    ),
                    Value::False(_, _) => Value::Residual(recursive_release),
                    Value::Residual(left) => {
                        Value::Residual(Residual::OrRelease {
                            left: Box::new(left),
                            right: Box::new(recursive_release),
                        })
                    }
                }
            }
            Value::Residual(right) => match self
                .evaluate(&left_formula, time)?
            {
                Value::True(left_snapshots) => {
                    Value::Residual(Residual::AndRelease {
                        left_formula,
                        right_formula,
                        bound,
                        end,
                        left: Box::new(right),
                        right: Box::new(Residual::True(left_snapshots)),
                    })
                }
                Value::False(_, _) => Value::Residual(Residual::AndRelease {
                    left_formula,
                    right_formula,
                    bound,
                    end,
                    left: Box::new(right),
                    right: Box::new(recursive_release),
                }),
                Value::Residual(left) => {
                    let fallback = Residual::OrRelease {
                        left: Box::new(left),
                        right: Box::new(recursive_release),
                    };
                    Value::Residual(Residual::AndRelease {
                        left_formula,
                        right_formula,
                        bound,
                        end,
                        left: Box::new(right),
                        right: Box::new(fallback),
                    })
                }
            },
        })
    }

    fn evaluate_or_release(
        &mut self,
        left: Value<Function>,
        right: Value<Function>,
    ) -> Value<Function> {
        match (left, right) {
            (Value::True(snapshots_left), Value::True(snapshots_right)) => {
                Value::True(merge_snapshots(&snapshots_left, &snapshots_right))
            }
            (Value::True(references), _) => Value::True(references),
            (_, Value::True(references)) => Value::True(references),
            (Value::False(_, _), Value::False(right, _)) => {
                Value::False(right, None)
            }
            (Value::False(_, _), Value::Residual(residual)) => {
                Value::Residual(residual)
            }
            (Value::Residual(residual), Value::False(_, _)) => {
                Value::Residual(residual)
            }
            (Value::Residual(left), Value::Residual(right)) => {
                Value::Residual(Residual::OrRelease {
                    left: Box::new(left),
                    right: Box::new(right),
                })
            }
        }
    }

    fn evaluate_and_release(
        &mut self,
        left_formula: Box<Formula<Function>>,
        right_formula: Box<Formula<Function>>,
        bound: Option<Duration>,
        end: Option<Time>,
        left: Value<Function>,
        right: Value<Function>,
    ) -> Value<Function> {
        match (left, right) {
            (Value::True(snapshots_left), Value::True(snapshots_right)) => {
                Value::True(merge_snapshots(&snapshots_left, &snapshots_right))
            }
            (Value::Residual(left), Value::True(snapshots)) => {
                Value::Residual(Residual::AndRelease {
                    left_formula,
                    right_formula,
                    bound,
                    end,
                    left: Box::new(left),
                    right: Box::new(Residual::True(snapshots)),
                })
            }
            (Value::True(snapshots), Value::Residual(right)) => {
                Value::Residual(Residual::AndRelease {
                    left_formula,
                    right_formula,
                    bound,
                    end,
                    left: Box::new(Residual::True(snapshots)),
                    right: Box::new(right),
                })
            }
            (Value::Residual(left), Value::Residual(right)) => {
                Value::Residual(Residual::AndRelease {
                    left_formula,
                    right_formula,
                    bound,
                    end,
                    left: Box::new(left),
                    right: Box::new(right),
                })
            }
            (Value::False(violation, _), _) => Value::False(
                Self::release_right_violation(
                    left_formula.as_ref(),
                    right_formula.as_ref(),
                    bound,
                    violation,
                ),
                None,
            ),
            (_, Value::False(violation, _)) => Value::False(violation, None),
        }
    }

    fn evaluate_always(
        &mut self,
        subformula: Box<Formula<Function>>,
        start: Time,
        end: Option<Time>,
        time: Time,
    ) -> Result<Value<Function>> {
        if let Some(end) = end
            && end < time
        {
            return Ok(Value::True(UniqueSnapshots::new()));
        }

        let residual = Residual::Derived(
            Derived::Always {
                subformula: subformula.clone(),
                start,
                end,
            },
            Leaning::AssumeTrue,
        );

        let wrap_and_always = |inner: Residual<Function>,
                               always: Residual<Function>|
         -> Residual<Function> {
            Residual::AndAlways {
                subformula: subformula.clone(),
                start,
                end,
                onset: time,
                left: Box::new(inner),
                right: Box::new(always),
            }
        };

        Ok(match self.evaluate(&subformula, time)? {
            Value::True(_) => Value::Residual(residual),
            Value::False(violation, continuation) => {
                let continuation = match continuation {
                    Some(inner) => wrap_and_always(inner, residual),
                    None => residual,
                };
                Value::False(
                    Violation::Always {
                        violation: Box::new(violation),
                        subformula: subformula.clone(),
                        start,
                        end,
                        time,
                    },
                    Some(continuation),
                )
            }
            Value::Residual(inner) => {
                Value::Residual(wrap_and_always(inner, residual))
            }
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn evaluate_and_always(
        &mut self,
        subformula: Box<Formula<Function>>,
        start: Time,
        end: Option<Time>,
        onset: Time,
        time: Time,
        left: Value<Function>,
        right: Value<Function>,
    ) -> Result<Value<Function>> {
        if let Some(end) = end
            && end < time
        {
            return Ok(Value::True(UniqueSnapshots::new()));
        }

        let wrap_and_always = |onset: Time,
                               inner: Residual<Function>,
                               always: Residual<Function>|
         -> Residual<Function> {
            Residual::AndAlways {
                subformula: subformula.clone(),
                start,
                end,
                onset,
                left: Box::new(inner),
                right: Box::new(always),
            }
        };

        fn pending_residual<F>(value: &Value<F>) -> Option<&Residual<F>> {
            match value {
                Value::Residual(residual) => Some(residual),
                Value::False(_, Some(continuation)) => Some(continuation),
                _ => None,
            }
        }

        Ok(match (left, right) {
            (Value::True(_), Value::True(_)) => {
                Value::True(UniqueSnapshots::new())
            }
            (Value::Residual(left), Value::True(_)) => {
                Value::Residual(Residual::AndAlways {
                    subformula,
                    start,
                    end,
                    onset,
                    left: Box::new(left),
                    right: Box::new(Residual::True(UniqueSnapshots::new())),
                })
            }
            (Value::True(_), Value::Residual(right)) => {
                Value::Residual(Residual::AndAlways {
                    subformula,
                    start,
                    end,
                    onset: time,
                    left: Box::new(Residual::True(UniqueSnapshots::new())),
                    right: Box::new(right),
                })
            }
            (Value::Residual(left), Value::Residual(right)) => {
                Value::Residual(Residual::AndAlways {
                    subformula,
                    start,
                    end,
                    onset,
                    left: Box::new(left),
                    right: Box::new(right),
                })
            }
            (left, right) => {
                let always_residual = Residual::Derived(
                    Derived::Always {
                        subformula: subformula.clone(),
                        start,
                        end,
                    },
                    Leaning::AssumeTrue,
                );
                let inner = combine_options(
                    pending_residual(&left).cloned(),
                    pending_residual(&right).cloned(),
                    |left, right| Residual::And {
                        left: Box::new(left),
                        right: Box::new(right),
                    },
                );
                let continuation = match inner {
                    Some(inner) => {
                        wrap_and_always(onset, inner, always_residual)
                    }
                    None => always_residual,
                };
                // Unwrap one layer of Always if present, since
                // we're about to re-wrap. The inner Always was
                // produced by either evaluate_always or a prior
                // evaluate_and_always call for the same formula.
                //
                // When the left side fails, use onset (when the
                // left residual was first created) so that "but
                // at T" reflects when the subformula first
                // started failing. When the right side fails,
                // use the time from the inner Always (set by
                // evaluate_always at the current step).
                let (violation, violation_time) = match (&left, &right) {
                    (Value::False(v, _), _) => match v {
                        Violation::Always { violation, .. } => {
                            (violation.as_ref(), onset)
                        }
                        other => (other, onset),
                    },
                    (_, Value::False(v, _)) => match v {
                        Violation::Always {
                            violation,
                            time: inner_time,
                            ..
                        } => (violation.as_ref(), *inner_time),
                        other => (other, time),
                    },
                    _ => unreachable!(),
                };
                Value::False(
                    Violation::Always {
                        violation: Box::new(violation.clone()),
                        subformula,
                        start,
                        end,
                        time: violation_time,
                    },
                    Some(continuation),
                )
            }
        })
    }

    fn evaluate_eventually(
        &mut self,
        subformula: Box<Formula<Function>>,
        start: Time,
        end: Option<Time>,
        time: Time,
    ) -> Result<Value<Function>> {
        if let Some(end) = end
            && end < time
        {
            return Ok(Value::False(
                Violation::Eventually {
                    subformula: subformula.clone(),
                    reason: EventuallyViolation::TimedOut(time),
                },
                None,
            ));
        }

        let residual = Residual::Derived(
            Derived::Eventually {
                subformula: subformula.clone(),
                start,
                end,
            },
            Leaning::AssumeFalse(Violation::Eventually {
                subformula: subformula.clone(),
                reason: EventuallyViolation::TestEnded,
            }),
        );

        Ok(match self.evaluate(&subformula, time)? {
            Value::True(references) => Value::True(references),
            Value::False(_violation, _) => Value::Residual(residual),
            Value::Residual(left) => Value::Residual(Residual::OrEventually {
                subformula,
                end,
                start,
                left: Box::new(left),
                right: Box::new(residual),
            }),
        })
    }

    fn evaluate_or_eventually(
        &mut self,
        subformula: Box<Formula<Function>>,
        start: Time,
        end: Option<Time>,
        time: Time,
        left: Value<Function>,
        right: Value<Function>,
    ) -> Result<Value<Function>> {
        if let Some(end) = end
            && end < time
        {
            return Ok(Value::False(
                Violation::Eventually {
                    subformula,
                    reason: EventuallyViolation::TimedOut(time),
                },
                None,
            ));
        }

        Ok(match (left, right) {
            (Value::True(references), _) => Value::True(references),
            (_, Value::True(references)) => Value::True(references),
            (Value::False(_, _), Value::False(right, _)) => {
                // NOTE: We ignore the left-side violation in `eventually` in
                // order to not build up a giant violation tree of all the
                // non-evidence we've seen (e.g. X was not true in state 1 and
                // X was not true in state 2 and ...).
                Value::False(right.clone(), None) // TODO: should this be wrapped in Violation::Eventually?
            }
            (Value::False(_, _), Value::Residual(residual)) => {
                Value::Residual(residual.clone())
            }
            (Value::Residual(residual), Value::False(_, _)) => {
                Value::Residual(residual.clone())
            }
            (Value::Residual(left), Value::Residual(right)) => {
                Value::Residual(Residual::OrEventually {
                    subformula,
                    start,
                    end,
                    left: Box::new(left.clone()),
                    right: Box::new(right.clone()),
                })
            }
        })
    }

    pub fn step(
        &mut self,
        residual: &Residual<Function>,
        time: Time,
    ) -> Result<Value<Function>> {
        Ok(match residual {
            Residual::True(references) => Value::True(references.clone()),
            Residual::False(violation) => Value::False(violation.clone(), None),
            Residual::And { left, right } => {
                let left = self.step(left, time)?;
                let right = self.step(right, time)?;
                self.evaluate_and(&left, &right)
            }
            Residual::Or { left, right } => {
                let left = self.step(left, time)?;
                let right = self.step(right, time)?;
                self.evaluate_or(&left, &right)
            }
            Residual::OrUntil { end, left, right } => {
                if let Some(end) = end
                    && *end < time
                {
                    self.step(right, time)?
                } else {
                    let left = self.step(left, time)?;
                    let right = self.step(right, time)?;
                    self.evaluate_or_until(*end, left, right)
                }
            }
            Residual::OrRelease { left, right } => {
                let left = self.step(left, time)?;
                let right = self.step(right, time)?;
                self.evaluate_or_release(left, right)
            }
            Residual::Implies {
                left_formula,
                left,
                right,
            } => {
                let left = self.step(left, time)?;
                let right = self.step(right, time)?;
                self.evaluate_implies(left_formula, &left, &right)
            }
            Residual::Derived(derived, _) => match derived {
                Derived::Once {
                    start: _,
                    subformula,
                } => {
                    // TODO: wrap potential violation in Next wrapper with start time
                    self.evaluate(subformula, time)?
                }
                Derived::Until {
                    left_formula,
                    right_formula,
                    bound,
                    end,
                } => self.evaluate_until(
                    left_formula.clone(),
                    right_formula.clone(),
                    *bound,
                    *end,
                    time,
                )?,
                Derived::Release {
                    left_formula,
                    right_formula,
                    bound,
                    end,
                } => self.evaluate_release(
                    left_formula.clone(),
                    right_formula.clone(),
                    *bound,
                    *end,
                    time,
                )?,
                Derived::Always {
                    start,
                    end,
                    subformula,
                } => self.evaluate_always(
                    subformula.clone(),
                    *start,
                    *end,
                    time,
                )?,
                Derived::Eventually {
                    start,
                    end: deadline,
                    subformula,
                } => self.evaluate_eventually(
                    subformula.clone(),
                    *start,
                    *deadline,
                    time,
                )?,
            },
            Residual::OrEventually {
                subformula,
                start,
                end,
                left,
                right,
            } => {
                let left = self.step(left, time)?;
                let right = self.step(right, time)?;

                self.evaluate_or_eventually(
                    subformula.clone(),
                    *start,
                    *end,
                    time,
                    left,
                    right,
                )?
            }
            Residual::AndAlways {
                subformula,
                start,
                end,
                onset,
                left,
                right,
            } => {
                let left = self.step(left, time)?;
                let right = self.step(right, time)?;
                self.evaluate_and_always(
                    subformula.clone(),
                    *start,
                    *end,
                    *onset,
                    time,
                    left,
                    right,
                )?
            }
            Residual::AndUntil {
                left_formula,
                right_formula,
                bound,
                end,
                left,
                right,
            } => {
                if let Some(end) = end
                    && *end < time
                {
                    Value::False(
                        Self::until_timed_out_violation(
                            left_formula,
                            right_formula,
                            *bound,
                            time,
                        ),
                        None,
                    )
                } else {
                    let left = self.step(left, time)?;
                    let right = self.step(right, time)?;
                    self.evaluate_and_until(
                        left_formula.clone(),
                        right_formula.clone(),
                        *bound,
                        *end,
                        left,
                        right,
                    )
                }
            }
            Residual::AndRelease {
                left_formula,
                right_formula,
                bound,
                end,
                left,
                right,
            } => {
                if let Some(end) = end
                    && *end < time
                {
                    Value::True(UniqueSnapshots::new())
                } else {
                    let left = self.step(left, time)?;
                    let right = self.step(right, time)?;
                    self.evaluate_and_release(
                        left_formula.clone(),
                        right_formula.clone(),
                        *bound,
                        *end,
                        left,
                        right,
                    )
                }
            }
        })
    }
}

fn attach_snapshots<F>(value: &mut Value<F>, resolved: UniqueSnapshots) {
    if resolved.is_empty() {
        return;
    }
    match value {
        Value::True(snapshots) => {
            snapshots.extend(resolved);
        }
        Value::False(violation, _) => {
            if let Violation::False { snapshots, .. } = violation {
                snapshots.extend(resolved.values().cloned());
            }
        }
        Value::Residual(_) => {}
    }
}
