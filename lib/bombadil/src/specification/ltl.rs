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
    Pure { value: bool, pretty: String },
    Thunk { function: Function, negated: bool },
    And(Box<Formula<Function>>, Box<Formula<Function>>),
    Or(Box<Formula<Function>>, Box<Formula<Function>>),
    Implies(Box<Formula<Function>>, Box<Formula<Function>>),
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
}

#[derive(Clone, Debug, PartialEq)]
pub enum Derived<Function> {
    Once {
        start: Time,
        subformula: Box<Formula<Function>>,
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

impl<Function> Residual<Function> {
    pub fn operator_name(&self) -> &'static str {
        match self {
            Residual::True(_) => "true",
            Residual::False(_) => "false",
            Residual::Derived(derived, _) => match derived {
                Derived::Once { .. } => "once",
                Derived::Always { .. } => "always",
                Derived::Eventually { .. } => "eventually",
            },
            Residual::And { .. } => "and",
            Residual::Or { .. } => "or",
            Residual::Implies { .. } => "implies",
            Residual::OrEventually { .. } => "eventually",
            Residual::AndAlways { .. } => "always",
        }
    }
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
            Formula::Next(formula) => Ok(Value::Residual(Residual::Derived(
                Derived::Once {
                    start: time,
                    subformula: formula.clone(),
                },
                Leaning::AssumeTrue, // TODO: expose true/false leaning in TS layer?
            ))),
            Formula::Always(formula, bound) => {
                let end = if let Some(duration) = bound {
                    Some(time.checked_add(*duration).ok_or(
                        SpecificationError::OtherError(
                            "failed to add bound to time".to_string(),
                        ),
                    )?)
                } else {
                    None
                };
                self.evaluate_always(formula.clone(), time, end, time)
            }
            Formula::Eventually(formula, bound) => {
                let end = if let Some(duration) = bound {
                    Some(time.checked_add(*duration).ok_or(
                        SpecificationError::OtherError(
                            "failed to add bound to time".to_string(),
                        ),
                    )?)
                } else {
                    None
                };
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
