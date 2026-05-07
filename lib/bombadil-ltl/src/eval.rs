use crate::formula::{Domain, Formula, State};
use crate::violation::{EventuallyViolation, Violation};

#[derive(Clone, Debug, PartialEq)]
pub enum Value<D: Domain> {
    True(D::State),
    False(Violation<D>, Option<Residual<D>>),
    Residual(Residual<D>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Leaning<D: Domain> {
    AssumeTrue,
    AssumeFalse(Violation<D>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Residual<D: Domain> {
    True(D::State),
    False(Violation<D>),
    Derived(Derived<D>, Leaning<D>),
    And {
        left: Box<Residual<D>>,
        right: Box<Residual<D>>,
    },
    Or {
        left: Box<Residual<D>>,
        right: Box<Residual<D>>,
    },
    Implies {
        left_formula: Formula<D>,
        left: Box<Residual<D>>,
        right: Box<Residual<D>>,
    },
    OrEventually {
        subformula: Box<Formula<D>>,
        start: D::Time,
        end: Option<D::Time>,
        left: Box<Residual<D>>,
        right: Box<Residual<D>>,
    },
    AndAlways {
        subformula: Box<Formula<D>>,
        start: D::Time,
        end: Option<D::Time>,
        /// When the left-side residual was first created. Used as
        /// the violation time in the Always wrapper so that "but
        /// at T" reflects when the subformula first started
        /// failing, not when the failure was confirmed.
        onset: D::Time,
        left: Box<Residual<D>>,
        right: Box<Residual<D>>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Derived<D: Domain> {
    Once {
        start: D::Time,
        subformula: Box<Formula<D>>,
    },
    Always {
        start: D::Time,
        end: Option<D::Time>,
        subformula: Box<Formula<D>>,
    },
    Eventually {
        start: D::Time,
        end: Option<D::Time>,
        subformula: Box<Formula<D>>,
    },
}

pub type EvaluateThunk<'a, D, Error> =
    &'a mut dyn FnMut(
        &'_ <D as Domain>::Function,
        bool,
    ) -> Result<(Formula<D>, <D as Domain>::State), Error>;

pub struct Evaluator<'a, D: Domain, Error> {
    evaluate_thunk: EvaluateThunk<'a, D, Error>,
}

impl<'a, D: Domain, Error> Evaluator<'a, D, Error> {
    pub fn new(evaluate_thunk: EvaluateThunk<'a, D, Error>) -> Self {
        Evaluator { evaluate_thunk }
    }

    pub fn evaluate(
        &mut self,
        formula: &Formula<D>,
        time: D::Time,
    ) -> Result<Value<D>, Error> {
        match formula {
            Formula::Pure { value, pretty } => Ok(if *value {
                Value::True(D::State::default())
            } else {
                Value::False(
                    Violation::False {
                        time,
                        condition: pretty.clone(),
                        state: D::State::default(),
                    },
                    None,
                )
            }),
            Formula::Thunk { function, negated } => {
                let (formula, state) =
                    (self.evaluate_thunk)(function, *negated)?;
                let mut value = self.evaluate(&formula, time)?;
                attach_state(&mut value, &state);
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
                Leaning::AssumeTrue,
            ))),
            Formula::Always(formula, bound) => {
                let end = bound.map(|duration| time + duration);
                self.evaluate_always(formula.clone(), time, end, time)
            }
            Formula::Eventually(formula, bound) => {
                let end = bound.map(|duration| time + duration);
                self.evaluate_eventually(formula.clone(), time, end, time)
            }
        }
    }

    fn evaluate_and(&mut self, left: &Value<D>, right: &Value<D>) -> Value<D> {
        fn combine_and<D: Domain>(
            left: Residual<D>,
            right: Residual<D>,
        ) -> Residual<D> {
            Residual::And {
                left: Box::new(left),
                right: Box::new(right),
            }
        }

        match (left, right) {
            (Value::True(left_state), Value::True(right_state)) => {
                Value::True(left_state.merge(right_state))
            }
            (Value::True(state), Value::Residual(residual)) => Value::Residual(
                combine_and(Residual::True(state.clone()), residual.clone()),
            ),
            (Value::Residual(residual), Value::True(state)) => Value::Residual(
                combine_and(residual.clone(), Residual::True(state.clone())),
            ),
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

    fn evaluate_or(&mut self, left: &Value<D>, right: &Value<D>) -> Value<D> {
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
            (Value::True(left_state), Value::True(right_state)) => {
                Value::True(left_state.merge(right_state))
            }
            (Value::True(state), _) => Value::True(state.clone()),
            (_, Value::True(state)) => Value::True(state.clone()),
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
        left_formula: &Formula<D>,
        left: &Value<D>,
        right: &Value<D>,
    ) -> Value<D> {
        match (left, right) {
            (Value::False(_, _), _) => Value::True(D::State::default()),
            (
                Value::True(left_state),
                Value::False(violation, continuation),
            ) => Value::False(
                Violation::Implies {
                    left: left_formula.clone(),
                    right: Box::new(violation.clone()),
                    state: left_state.clone(),
                },
                continuation.as_ref().map(|c| Residual::Implies {
                    left_formula: left_formula.clone(),
                    left: Box::new(Residual::True(left_state.clone())),
                    right: Box::new(c.clone()),
                }),
            ),
            (Value::True(left_state), Value::True(right_state)) => {
                Value::True(left_state.merge(right_state))
            }
            (Value::True(left_state), Value::Residual(right)) => {
                Value::Residual(Residual::Implies {
                    left_formula: left_formula.clone(),
                    left: Box::new(Residual::True(left_state.clone())),
                    right: Box::new(right.clone()),
                })
            }
            (Value::Residual(_), Value::True(state)) => {
                Value::True(state.clone())
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
        subformula: Box<Formula<D>>,
        start: D::Time,
        end: Option<D::Time>,
        time: D::Time,
    ) -> Result<Value<D>, Error> {
        if let Some(end) = end
            && end < time
        {
            return Ok(Value::True(D::State::default()));
        }

        let residual = Residual::Derived(
            Derived::Always {
                subformula: subformula.clone(),
                start,
                end,
            },
            Leaning::AssumeTrue,
        );

        let wrap_and_always =
            |inner: Residual<D>, always: Residual<D>| -> Residual<D> {
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
        subformula: Box<Formula<D>>,
        start: D::Time,
        end: Option<D::Time>,
        onset: D::Time,
        time: D::Time,
        left: Value<D>,
        right: Value<D>,
    ) -> Result<Value<D>, Error> {
        if let Some(end) = end
            && end < time
        {
            return Ok(Value::True(D::State::default()));
        }

        let wrap_and_always = |onset: D::Time,
                               inner: Residual<D>,
                               always: Residual<D>|
         -> Residual<D> {
            Residual::AndAlways {
                subformula: subformula.clone(),
                start,
                end,
                onset,
                left: Box::new(inner),
                right: Box::new(always),
            }
        };

        fn pending_residual<D: Domain>(
            value: &Value<D>,
        ) -> Option<&Residual<D>> {
            match value {
                Value::Residual(residual) => Some(residual),
                Value::False(_, Some(continuation)) => Some(continuation),
                _ => None,
            }
        }

        Ok(match (left, right) {
            (Value::True(_), Value::True(_)) => {
                Value::True(D::State::default())
            }
            (Value::Residual(left), Value::True(_)) => {
                Value::Residual(Residual::AndAlways {
                    subformula,
                    start,
                    end,
                    onset,
                    left: Box::new(left),
                    right: Box::new(Residual::True(D::State::default())),
                })
            }
            (Value::True(_), Value::Residual(right)) => {
                Value::Residual(Residual::AndAlways {
                    subformula,
                    start,
                    end,
                    onset: time,
                    left: Box::new(Residual::True(D::State::default())),
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
                let (violation, violation_time) = match (&left, &right) {
                    (Value::False(v, _), _) => {
                        let mut current = v;
                        while let Violation::Always {
                            violation: inner, ..
                        } = current
                        {
                            current = inner.as_ref();
                        }
                        (current, onset)
                    }
                    (_, Value::False(v, _)) => {
                        let mut current = v;
                        let mut last_time = time;
                        while let Violation::Always {
                            violation: inner,
                            time: inner_time,
                            ..
                        } = current
                        {
                            last_time = *inner_time;
                            current = inner.as_ref();
                        }
                        (current, last_time)
                    }
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
        subformula: Box<Formula<D>>,
        start: D::Time,
        end: Option<D::Time>,
        time: D::Time,
    ) -> Result<Value<D>, Error> {
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
            Value::True(state) => Value::True(state),
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
        subformula: Box<Formula<D>>,
        start: D::Time,
        end: Option<D::Time>,
        time: D::Time,
        left: Value<D>,
        right: Value<D>,
    ) -> Result<Value<D>, Error> {
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
            (Value::True(state), _) => Value::True(state),
            (_, Value::True(state)) => Value::True(state),
            (Value::False(_, _), Value::False(right, _)) => {
                Value::False(right.clone(), None)
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
        residual: &Residual<D>,
        time: D::Time,
    ) -> Result<Value<D>, Error> {
        Ok(match residual {
            Residual::True(state) => Value::True(state.clone()),
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
                } => self.evaluate(subformula, time)?,
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

fn attach_state<D: Domain>(value: &mut Value<D>, resolved: &D::State) {
    if resolved.is_empty() {
        return;
    }
    match value {
        Value::True(state) => {
            *state = state.merge(resolved);
        }
        Value::False(violation, _) => {
            attach_to_violation(violation, resolved);
        }
        Value::Residual(residual) => {
            attach_to_residual(residual, resolved);
        }
    }
}

fn attach_to_violation<D: Domain>(
    violation: &mut Violation<D>,
    resolved: &D::State,
) {
    let mut queue = vec![violation];

    while let Some(v) = queue.pop() {
        match v {
            Violation::False { state, .. } => {
                *state = state.merge(resolved);
            }
            Violation::Implies { state, right, .. } => {
                *state = state.merge(resolved);
                queue.push(right.as_mut());
            }
            Violation::And { left, right } => {
                queue.push(left.as_mut());
                queue.push(right.as_mut());
            }
            Violation::Or { left, right } => {
                queue.push(left.as_mut());
                queue.push(right.as_mut());
            }
            Violation::Always { violation, .. } => {
                queue.push(violation.as_mut());
            }
            Violation::Eventually { .. } => {}
        }
    }
}

fn attach_to_residual<D: Domain>(
    residual: &mut Residual<D>,
    resolved: &D::State,
) {
    let mut queue = vec![residual];

    while let Some(r) = queue.pop() {
        match r {
            Residual::True(state) => {
                *state = state.merge(resolved);
            }
            Residual::False(violation) => {
                attach_to_violation(violation, resolved);
            }
            Residual::And { left, right }
            | Residual::Or { left, right }
            | Residual::OrEventually { left, right, .. }
            | Residual::AndAlways { left, right, .. } => {
                queue.push(left.as_mut());
                queue.push(right.as_mut());
            }
            Residual::Implies { left, right, .. } => {
                queue.push(left.as_mut());
                queue.push(right.as_mut());
            }
            Residual::Derived(_, _) => {}
        }
    }
}

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
