use std::time::{Duration, SystemTime};

use crate::specification::result::{Result, SpecificationError};
use serde::Serialize;

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

#[derive(Clone, Debug, PartialEq)]
pub enum Value<Function> {
    True,
    False(Violation<Function>),
    Residual(Residual<Function>),
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum Violation<Function> {
    False {
        time: Time,
        condition: String,
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
            Violation::False { time, condition } => Violation::False {
                time: *time,
                condition: condition.clone(),
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
            Violation::Implies { left, right } => Violation::Implies {
                left: left.map_function_ref(f),
                right: Box::new(right.map_function_ref(f)),
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
    True,
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

pub type EvaluateThunk<'a, Function> =
    &'a mut dyn FnMut(&'_ Function, bool) -> Result<Formula<Function>>;

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
                Value::True
            } else {
                Value::False(Violation::False {
                    time,
                    condition: pretty.clone(),
                })
            }),
            Formula::Thunk { function, negated } => {
                let formula = (self.evaluate_thunk)(function, *negated)?;
                Ok(self.evaluate(&formula, time)?)
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
        match (left, right) {
            (Value::True, right) => right.clone(),
            (left, Value::True) => left.clone(),
            (Value::False(left), Value::False(right)) => {
                Value::False(Violation::And {
                    left: Box::new(left.clone()),
                    right: Box::new(right.clone()),
                })
            }
            (_, Value::False(violation)) => Value::False(violation.clone()),
            (Value::False(violation), _) => Value::False(violation.clone()),
            (Value::Residual(left), Value::Residual(right)) => {
                Value::Residual(Residual::And {
                    left: Box::new(left.clone()),
                    right: Box::new(right.clone()),
                })
            }
        }
    }

    fn evaluate_or(
        &mut self,
        left: &Value<Function>,
        right: &Value<Function>,
    ) -> Value<Function> {
        match (left, right) {
            (Value::False(left), Value::False(right)) => {
                Value::False(Violation::Or {
                    left: Box::new(left.clone()),
                    right: Box::new(right.clone()),
                })
            }
            (Value::True, _) => Value::True,
            (_, Value::True) => Value::True,
            (left, Value::False(_)) => left.clone(),
            (Value::False(_), right) => right.clone(),
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
            (Value::False(_), _) => Value::True,
            (Value::True, Value::False(violation)) => {
                Value::False(Violation::Implies {
                    left: left_formula.clone(),
                    right: Box::new(violation.clone()),
                })
            }
            (Value::True, Value::True) => Value::True,
            (Value::True, Value::Residual(right)) => {
                Value::Residual(Residual::Implies {
                    left_formula: left_formula.clone(),
                    left: Box::new(Residual::True),
                    right: Box::new(right.clone()),
                })
            }
            (Value::Residual(_), Value::True) => Value::True,
            (Value::Residual(left), Value::False(violation)) => {
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
            return Ok(Value::True);
        }

        let residual = Residual::Derived(
            Derived::Always {
                subformula: subformula.clone(),
                start,
                end,
            },
            Leaning::AssumeTrue,
        );

        Ok(match self.evaluate(&subformula, time)? {
            Value::True => Value::Residual(residual),
            Value::False(violation) => Value::False(Violation::Always {
                violation: Box::new(violation),
                subformula: subformula.clone(),
                start,
                end,
                time,
            }),
            Value::Residual(left) => Value::Residual(Residual::AndAlways {
                subformula: subformula.clone(),
                start,
                end,
                left: Box::new(left),
                right: Box::new(residual),
            }),
        })
    }

    fn evaluate_and_always(
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
            return Ok(Value::True);
        }

        Ok(match (left, right) {
            (Value::True, Value::True) => Value::True,
            (Value::False(violation), _) => Value::False(Violation::Always {
                violation: Box::new(violation.clone()),
                subformula,
                start,
                end,
                time,
            }),
            (_, Value::False(violation)) => Value::False(Violation::Always {
                violation: Box::new(violation.clone()),
                subformula,
                start,
                end,
                time,
            }),
            (Value::Residual(left), Value::True) => {
                Value::Residual(Residual::AndAlways {
                    subformula,
                    start,
                    end,
                    left: Box::new(left),
                    right: Box::new(Residual::True),
                })
            }
            (Value::True, Value::Residual(right)) => {
                Value::Residual(Residual::AndAlways {
                    subformula,
                    start,
                    end,
                    left: Box::new(Residual::True),
                    right: Box::new(right),
                })
            }
            (Value::Residual(left), Value::Residual(right)) => {
                Value::Residual(Residual::AndAlways {
                    subformula,
                    start,
                    end,
                    left: Box::new(left),
                    right: Box::new(right),
                })
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
            return Ok(Value::False(Violation::Eventually {
                subformula: subformula.clone(),
                reason: EventuallyViolation::TimedOut(time),
            }));
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
            Value::True => Value::True,
            Value::False(_violation) => Value::Residual(residual),
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
            return Ok(Value::False(Violation::Eventually {
                subformula,
                reason: EventuallyViolation::TimedOut(time),
            }));
        }

        Ok(match (left, right) {
            (Value::True, _) => Value::True,
            (_, Value::True) => Value::True,
            (Value::False(_), Value::False(right)) => {
                // NOTE: We ignore the left-side violation in `eventually` in
                // order to not build up a giant violation tree of all the
                // non-evidence we've seen (e.g. X was not true in state 1 and
                // X was not true in state 2 and ...).
                Value::False(right.clone()) // TODO: should this be wrapped in Violation::Eventually?
            }
            (Value::False(_), Value::Residual(residual)) => {
                Value::Residual(residual.clone())
            }
            (Value::Residual(residual), Value::False(_)) => {
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
            Residual::True => Value::True,
            Residual::False(violation) => Value::False(violation.clone()),
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
                left,
                right,
            } => {
                let left = self.step(left, time)?;
                let right = self.step(right, time)?;
                self.evaluate_and_always(
                    subformula.clone(),
                    *start,
                    *end,
                    time,
                    left,
                    right,
                )?
            }
        })
    }
}
