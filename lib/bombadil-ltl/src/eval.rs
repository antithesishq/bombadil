/// This module implements the evaluator for Bombadil's LTL. The main component
/// is the [Evaluator], with two key methods:
///
/// * [Evaluator::evaluate]: this takes a [Formula] and returns a [Value]. A
///   value is either [Value::True], [Value::False], or [Value::Residual].
///   The false values are violations, along with a residual to optionally
///   continue evaluating to possibly collect multiple violations. Residuals
///   are for when we don't yet have a definitive answer and need to either
///   decide how to stop (based on [Leaning]), or continue stepping the formula
///   with more states.
/// * [Evaluator::step]: given a residual, we step it one step ahead, getting
///   back another value to inspect.
///
/// Those two primitives constitute the main flow of evaluating formulas.
///
/// [State]s are not global states, but partial ones. The evaluator tracks the
/// state values used when evaluating [Thunk]s in order to provide a [Violation]
/// structure with relevant information. This can be used to render detailed
/// error messages with only relevant information. It is up to the caller to
/// return only the relevant state when returning a result from [EvaluateThunk].
///
/// [Formula::Thunk] are embedded domain-specific computations in the host
/// language that return formulas. These are used to implement custom logic
/// and state usage interleaved with the pure LTL evaluation.
use crate::formula::{Domain, Formula, State};
use crate::violation::{EventuallyViolation, UntilViolation, Violation};

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
        pending: Vec<Residual<D>>,
    },
    AndAlways {
        subformula: Box<Formula<D>>,
        start: D::Time,
        end: Option<D::Time>,
        /// When the first pending residual was created. Used as
        /// the violation time in the Always wrapper so that "but
        /// at T" reflects when the subformula first started
        /// failing, not when the failure was confirmed.
        onset: D::Time,
        pending: Vec<Residual<D>>,
    },
    Until {
        left: Box<Formula<D>>,
        right: Box<Formula<D>>,
        start: D::Time,
        pending: Vec<UntilPending<D>>,
        future: bool,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct UntilPending<D: Domain> {
    pub left: Residual<D>,
    pub right: Residual<D>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResidualSize {
    pub nodes: usize,
    pub depth: usize,
}

impl<D: Domain> Residual<D> {
    pub fn size(&self) -> ResidualSize {
        let mut size = ResidualSize::default();
        let mut queue = vec![(self, 1usize)];

        while let Some((residual, depth)) = queue.pop() {
            size.nodes += 1;
            size.depth = size.depth.max(depth);
            match residual {
                Residual::True(_)
                | Residual::False(_)
                | Residual::Derived(_, _) => {}
                Residual::And { left, right }
                | Residual::Or { left, right }
                | Residual::Implies { left, right, .. } => {
                    queue.push((left, depth + 1));
                    queue.push((right, depth + 1));
                }
                Residual::OrEventually { pending, .. }
                | Residual::AndAlways { pending, .. } => {
                    for residual in pending {
                        queue.push((residual, depth + 1));
                    }
                }
                Residual::Until { pending, .. } => {
                    for pending in pending {
                        queue.push((&pending.left, depth + 1));
                        queue.push((&pending.right, depth + 1));
                    }
                }
            }
        }

        size
    }
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

    #[hotpath::measure]
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
                attach_state(&mut value, state);
                Ok(value)
            }
            Formula::And(left, right) => {
                let left = self.evaluate(left.as_ref(), time)?;
                let right = self.evaluate(right.as_ref(), time)?;
                Ok(self.evaluate_and(left, right))
            }
            Formula::Or(left, right) => {
                let left = self.evaluate(left.as_ref(), time)?;
                let right = self.evaluate(right.as_ref(), time)?;
                Ok(self.evaluate_or(left, right))
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
            Formula::Until(left, right) => self.evaluate_until(
                left.clone(),
                right.clone(),
                time,
                &[],
                true,
                time,
            ),
        }
    }

    fn evaluate_until(
        &mut self,
        left: Box<Formula<D>>,
        right: Box<Formula<D>>,
        start: D::Time,
        pending: &[UntilPending<D>],
        mut future: bool,
        time: D::Time,
    ) -> Result<Value<D>, Error> {
        let mut updated = Vec::with_capacity(pending.len() + 1);
        for pending in pending {
            let right_value = self.step(&pending.right, time)?;
            if let Value::True(state) = right_value {
                updated.push(UntilPending {
                    left: Residual::True(D::State::default()),
                    right: Residual::True(state),
                });
                future = false;
                break;
            }
            let left_value = self.step(&pending.left, time)?;
            let left_failed = matches!(left_value, Value::False(_, _));
            updated.push(UntilPending {
                left: until_operand(left_value),
                right: until_operand(right_value),
            });
            if left_failed {
                future = false;
                break;
            }
        }
        if future {
            let right_value = self.evaluate(&right, time)?;
            let left_value = if matches!(right_value, Value::True(_)) {
                Value::True(D::State::default())
            } else {
                self.evaluate(&left, time)?
            };
            updated.push(UntilPending {
                left: until_operand(left_value),
                right: until_operand(right_value),
            });
        }
        Ok(reduce_until(left, right, start, updated, future))
    }

    fn evaluate_and(&mut self, left: Value<D>, right: Value<D>) -> Value<D> {
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
                Value::True(left_state.merge(&right_state))
            }
            (Value::True(state), Value::Residual(residual)) => {
                Value::Residual(combine_and(Residual::True(state), residual))
            }
            (Value::Residual(residual), Value::True(state)) => {
                Value::Residual(combine_and(residual, Residual::True(state)))
            }
            (Value::True(_), right) => right,
            (left, Value::True(_)) => left,
            (
                Value::False(violation_left, residual_left),
                Value::False(violation_right, residual_right),
            ) => Value::False(
                Violation::And {
                    left: Box::new(violation_left),
                    right: Box::new(violation_right),
                },
                combine_options(residual_left, residual_right, combine_and),
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
                    Some(continuation) => combine_and(residual, continuation),
                    None => residual,
                };
                Value::False(violation, Some(continuation))
            }
            (Value::Residual(left), Value::Residual(right)) => {
                Value::Residual(combine_and(left, right))
            }
        }
    }

    fn evaluate_or(&mut self, left: Value<D>, right: Value<D>) -> Value<D> {
        match (left, right) {
            (
                Value::False(violation_left, residual_left),
                Value::False(violation_right, residual_right),
            ) => Value::False(
                Violation::Or {
                    left: Box::new(violation_left),
                    right: Box::new(violation_right),
                },
                combine_options(
                    residual_left,
                    residual_right,
                    |left, right| Residual::Or {
                        left: Box::new(left),
                        right: Box::new(right),
                    },
                ),
            ),
            (Value::True(left_state), Value::True(right_state)) => {
                Value::True(left_state.merge(&right_state))
            }
            (Value::True(state), _) => Value::True(state),
            (_, Value::True(state)) => Value::True(state),
            (left, Value::False(_, _)) => left,
            (Value::False(_, _), right) => right,
            (Value::Residual(left), Value::Residual(right)) => {
                Value::Residual(Residual::Or {
                    left: Box::new(left),
                    right: Box::new(right),
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

        Ok(match self.evaluate(&subformula, time)? {
            Value::True(_) => Value::Residual(residual),
            Value::False(violation, continuation) => {
                let continuation = match continuation {
                    Some(inner) => Residual::AndAlways {
                        subformula: subformula.clone(),
                        start,
                        end,
                        onset: time,
                        pending: vec![inner, residual],
                    },
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
            Value::Residual(inner) => Value::Residual(Residual::AndAlways {
                subformula: subformula.clone(),
                start,
                end,
                onset: time,
                pending: vec![inner, residual],
            }),
        })
    }

    fn evaluate_and_always(
        &mut self,
        subformula: Box<Formula<D>>,
        start: D::Time,
        end: Option<D::Time>,
        onset: D::Time,
        time: D::Time,
        pending: Vec<Value<D>>,
    ) -> Result<Value<D>, Error> {
        if let Some(end) = end
            && end < time
        {
            return Ok(Value::True(D::State::default()));
        }

        let mut pending_updated = Vec::with_capacity(pending.len());
        // We track if the derived residual (constructed below but only conditionally added)
        // is already in the set of pending residuals.
        let mut has_own_derived_residual = false;

        fn is_own_derived<D: Domain>(
            residual: &Residual<D>,
            subformula: &Formula<D>,
            start: D::Time,
            end: Option<D::Time>,
        ) -> bool {
            matches!(
                residual,
                Residual::Derived(Derived::Always { subformula: subformula_other, start: start_other, end: end_other }, _)
                    if **subformula_other == *subformula && *start_other == start && *end_other == end
            )
        }
        let mut first_violation: Option<(Violation<D>, D::Time)> = None;

        for value in pending {
            match value {
                Value::True(_) => {}
                // Special case: if one of the pending values is an residual instance of this
                // very Always formula we're in the process of stepping through, we splice it
                // into our pending list to keep it from growing deeper at each step.
                Value::Residual(Residual::AndAlways {
                    subformula: subformula_inner,
                    start: start_inner,
                    end: end_inner,
                    pending: pending_inner,
                    ..
                }) if subformula_inner == subformula
                    && start_inner == start
                    && end_inner == end =>
                {
                    for pending in pending_inner {
                        if is_own_derived(&pending, &subformula, start, end) {
                            has_own_derived_residual = true;
                        }
                        pending_updated.push(pending);
                    }
                }
                Value::Residual(residual) => {
                    if is_own_derived(&residual, &subformula, start, end) {
                        has_own_derived_residual = true;
                    }
                    pending_updated.push(residual);
                }
                Value::False(violation, residual) => {
                    if let Some(residual) = residual {
                        pending_updated.push(residual);
                    }
                    if first_violation.is_none() {
                        // Unwrap nested Violation::Always to find the real onset time.
                        let mut current = &violation;
                        let mut violation_time = time;
                        while let Violation::Always {
                            violation: violation_inner,
                            time: time_inner,
                            ..
                        } = current
                        {
                            violation_time = *time_inner;
                            current = violation_inner.as_ref();
                        }
                        first_violation =
                            Some((current.clone(), violation_time));
                    }
                }
            }
        }

        let residual = Residual::Derived(
            Derived::Always {
                subformula: subformula.clone(),
                start,
                end,
            },
            Leaning::AssumeTrue,
        );
        // See note above about dedup of pending Always-derived residuals.
        if !has_own_derived_residual {
            pending_updated.push(residual);
        }

        Ok(match first_violation {
            Some((violation, violation_time)) => Value::False(
                Violation::Always {
                    violation: Box::new(violation),
                    subformula: subformula.clone(),
                    start,
                    end,
                    time: violation_time,
                },
                Some(Residual::AndAlways {
                    subformula: subformula.clone(),
                    start,
                    end,
                    onset,
                    pending: pending_updated,
                }),
            ),
            None if pending_updated.is_empty() => {
                Value::True(D::State::default())
            }
            None => Value::Residual(Residual::AndAlways {
                subformula,
                start,
                end,
                onset,
                pending: pending_updated,
            }),
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
                pending: vec![left, residual],
            }),
        })
    }

    fn evaluate_or_eventually(
        &mut self,
        subformula: Box<Formula<D>>,
        start: D::Time,
        end: Option<D::Time>,
        time: D::Time,
        pending: Vec<Value<D>>,
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

        let mut pending_updated = Vec::with_capacity(pending.len());
        let mut witness_found: Option<D::State> = None;
        let mut has_own_derived_residual = false;

        fn is_own_derived_residual<D: Domain>(
            r: &Residual<D>,
            subformula: &Formula<D>,
            start: D::Time,
            end: Option<D::Time>,
        ) -> bool {
            matches!(
                r,
                Residual::Derived(Derived::Eventually { subformula: subformula_other, start: start_other, end: end_other }, _)
                    if **subformula_other == *subformula && *start_other == start && *end_other == end
            )
        }

        for value in pending {
            match value {
                Value::True(state) => {
                    witness_found = Some(match witness_found {
                        Some(s) => s.merge(&state),
                        None => state,
                    });
                }
                Value::Residual(Residual::OrEventually {
                    subformula: si,
                    start: sti,
                    end: ei,
                    pending: pi,
                    ..
                }) if si == subformula && sti == start && ei == end => {
                    for r in pi {
                        if is_own_derived_residual(&r, &subformula, start, end)
                        {
                            has_own_derived_residual = true;
                        }
                        pending_updated.push(r);
                    }
                }
                Value::Residual(r) => {
                    if is_own_derived_residual(&r, &subformula, start, end) {
                        has_own_derived_residual = true;
                    }
                    pending_updated.push(r);
                }
                // Not a witness, and its continuation (which is only used
                // to keep checking Always properties for multiple violations)
                // cannot become a witness at a later state, hence we ignore
                // it.
                Value::False(_v, _continuation) => {}
            }
        }

        if let Some(state) = witness_found {
            return Ok(Value::True(state));
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
        if !has_own_derived_residual {
            pending_updated.push(residual);
        }

        Ok(if pending_updated.is_empty() {
            Value::False(
                Violation::Eventually {
                    subformula,
                    reason: EventuallyViolation::TestEnded,
                },
                None,
            )
        } else {
            Value::Residual(Residual::OrEventually {
                subformula,
                start,
                end,
                pending: pending_updated,
            })
        })
    }

    #[hotpath::measure]
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
                self.evaluate_and(left, right)
            }
            Residual::Or { left, right } => {
                let left = self.step(left, time)?;
                let right = self.step(right, time)?;
                self.evaluate_or(left, right)
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
            Residual::Until {
                left,
                right,
                start,
                pending,
                future,
            } => self.evaluate_until(
                left.clone(),
                right.clone(),
                *start,
                pending,
                *future,
                time,
            )?,
            Residual::OrEventually {
                subformula,
                start,
                end,
                pending,
            } => {
                let pending: Vec<Value<D>> = pending
                    .iter()
                    .map(|r| self.step(r, time))
                    .collect::<Result<_, _>>()?;
                self.evaluate_or_eventually(
                    subformula.clone(),
                    *start,
                    *end,
                    time,
                    pending,
                )?
            }
            Residual::AndAlways {
                subformula,
                start,
                end,
                onset,
                pending,
            } => {
                let pending: Vec<Value<D>> = pending
                    .iter()
                    .map(|r| self.step(r, time))
                    .collect::<Result<_, _>>()?;
                self.evaluate_and_always(
                    subformula.clone(),
                    *start,
                    *end,
                    *onset,
                    time,
                    pending,
                )?
            }
        })
    }
}

fn until_operand<D: Domain>(value: Value<D>) -> Residual<D> {
    match value {
        Value::True(state) => Residual::True(state),
        Value::False(violation, _) => Residual::False(violation),
        Value::Residual(residual) => residual,
    }
}

pub(crate) fn reduce_until<D: Domain>(
    left: Box<Formula<D>>,
    right: Box<Formula<D>>,
    start: D::Time,
    pending: Vec<UntilPending<D>>,
    mut future: bool,
) -> Value<D> {
    let mut result = if future {
        None
    } else {
        Some(Err(UntilViolation::TestEnded))
    };
    let mut retained = Vec::with_capacity(pending.len());
    for pending in pending.into_iter().rev() {
        match (&pending.right, &pending.left) {
            (Residual::True(state), _) => {
                retained.clear();
                future = false;
                result = Some(Ok(state.clone()));
            }
            (Residual::False(right), Residual::False(left)) => {
                retained.clear();
                future = false;
                result = Some(Err(UntilViolation::BeforeRight {
                    left: Box::new(left.clone()),
                    right: Box::new(right.clone()),
                }));
            }
            (Residual::False(_), Residual::True(_)) => continue,
            (_, Residual::False(_)) => {
                retained.clear();
                future = false;
                result = None;
            }
            (Residual::False(_), _) if matches!(result, Some(Err(_))) => {
                continue;
            }
            (_, Residual::True(_)) if matches!(result, Some(Ok(_))) => continue,
            _ => result = None,
        }
        retained.push(pending);
    }
    match result {
        Some(Ok(state)) => Value::True(state),
        Some(Err(reason)) => Value::False(
            Violation::Until {
                left,
                right,
                start,
                reason,
            },
            None,
        ),
        None => {
            retained.reverse();
            retained.dedup_by(|newer, older| {
                same_until_obligation(&older.right, &newer.right)
                    && (matches!(newer.left, Residual::True(_))
                        || same_until_obligation(&older.left, &newer.left))
            });
            Value::Residual(Residual::Until {
                left,
                right,
                start,
                pending: retained,
                future,
            })
        }
    }
}

fn same_until_obligation<D: Domain>(
    left: &Residual<D>,
    right: &Residual<D>,
) -> bool {
    use crate::eval::Derived::{Always, Eventually, Once};
    use Residual::*;

    match (left, right) {
        (True(_), True(_)) | (False(_), False(_)) => true,
        (Derived(left, left_leaning), Derived(right, right_leaning)) => {
            if std::mem::discriminant(left_leaning)
                != std::mem::discriminant(right_leaning)
            {
                return false;
            }
            match (left, right) {
                (
                    Once {
                        subformula: left, ..
                    },
                    Once {
                        subformula: right, ..
                    },
                ) => left == right,
                (
                    Always {
                        subformula: left,
                        end: left_end,
                        ..
                    },
                    Always {
                        subformula: right,
                        end: right_end,
                        ..
                    },
                )
                | (
                    Eventually {
                        subformula: left,
                        end: left_end,
                        ..
                    },
                    Eventually {
                        subformula: right,
                        end: right_end,
                        ..
                    },
                ) => left == right && left_end == right_end,
                _ => false,
            }
        }
        (
            And {
                left: ll,
                right: lr,
            },
            And {
                left: rl,
                right: rr,
            },
        )
        | (
            Or {
                left: ll,
                right: lr,
            },
            Or {
                left: rl,
                right: rr,
            },
        )
        | (
            Implies {
                left: ll,
                right: lr,
                ..
            },
            Implies {
                left: rl,
                right: rr,
                ..
            },
        ) => same_until_obligation(ll, rl) && same_until_obligation(lr, rr),
        (
            OrEventually {
                subformula: left,
                end: left_end,
                pending: left_pending,
                ..
            },
            OrEventually {
                subformula: right,
                end: right_end,
                pending: right_pending,
                ..
            },
        )
        | (
            AndAlways {
                subformula: left,
                end: left_end,
                pending: left_pending,
                ..
            },
            AndAlways {
                subformula: right,
                end: right_end,
                pending: right_pending,
                ..
            },
        ) => {
            left == right
                && left_end == right_end
                && left_pending.len() == right_pending.len()
                && left_pending
                    .iter()
                    .zip(right_pending)
                    .all(|(l, r)| same_until_obligation(l, r))
        }
        (
            Until {
                left: ll,
                right: lr,
                pending: left_pending,
                future: left_future,
                ..
            },
            Until {
                left: rl,
                right: rr,
                pending: right_pending,
                future: right_future,
                ..
            },
        ) => {
            ll == rl
                && lr == rr
                && left_future == right_future
                && left_pending.len() == right_pending.len()
                && left_pending.iter().zip(right_pending).all(|(l, r)| {
                    same_until_obligation(&l.left, &r.left)
                        && same_until_obligation(&l.right, &r.right)
                })
        }
        _ => false,
    }
}

fn attach_state<D: Domain>(value: &mut Value<D>, resolved: D::State) {
    if resolved.is_empty() {
        return;
    }
    match value {
        Value::True(state) => {
            *state = if state.is_empty() {
                resolved
            } else {
                state.merge(&resolved)
            };
        }
        Value::False(violation, _) => {
            attach_to_violation(violation, &resolved);
        }
        Value::Residual(residual) => {
            attach_to_residual(residual, &resolved);
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
            Violation::Until { reason, .. } => {
                if let UntilViolation::BeforeRight { left, right } = reason {
                    queue.push(left.as_mut());
                    queue.push(right.as_mut());
                }
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
            Residual::And { left, right } | Residual::Or { left, right } => {
                queue.push(left.as_mut());
                queue.push(right.as_mut());
            }
            Residual::AndAlways { pending, .. }
            | Residual::OrEventually { pending, .. } => {
                for residual in pending {
                    queue.push(residual);
                }
            }
            Residual::Until { pending, .. } => {
                for pending in pending {
                    queue.push(&mut pending.left);
                    queue.push(&mut pending.right);
                }
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

#[cfg(test)]
mod tests {
    use crate::{
        eval::{Evaluator, Residual, Value},
        formula::Formula,
        stop::{StopDefault, stop_default},
        test_domain::{
            TestDomain, TestState, TestTime, Variable, evaluate_with_state,
            step_with_state,
        },
        violation::{EventuallyViolation, UntilViolation, Violation},
    };
    use std::{cell::Cell, time::Duration};

    type D = TestDomain;
    type F = Formula<D>;

    fn atom(variable: Variable) -> F {
        Formula::Thunk {
            function: variable,
            negated: false,
        }
    }
    fn until(left: F, right: F) -> F {
        Formula::Until(Box::new(left), Box::new(right))
    }
    fn next(formula: F) -> F {
        Formula::Next(Box::new(formula))
    }
    fn eventually(formula: F) -> F {
        Formula::Eventually(Box::new(formula), None)
    }
    fn always(formula: F) -> F {
        Formula::Always(Box::new(formula), None)
    }
    fn state((x, y): (bool, bool)) -> TestState {
        TestState { x, y, z: false }
    }
    fn evaluate(formula: &F, current: (bool, bool)) -> Value<D> {
        evaluate_with_state(formula, &state(current), |_| ())
    }
    fn step(
        residual: &Residual<D>,
        current: (bool, bool),
        time: u64,
    ) -> Value<D> {
        step_with_state(residual, &state(current), TestTime(time), |_| ())
    }
    fn pending(value: Value<D>) -> Residual<D> {
        let Value::Residual(residual) = value else {
            panic!("expected residual, got {value:?}")
        };
        residual
    }
    fn before_right<'a>(
        violation: &'a Violation<D>,
        formula: &F,
    ) -> (&'a Violation<D>, &'a Violation<D>) {
        let Violation::Until {
            left,
            right,
            start,
            reason:
                UntilViolation::BeforeRight {
                    left: failed_left,
                    right: failed_right,
                },
        } = violation
        else {
            panic!("expected contextual until failure, got {violation:?}")
        };
        assert_eq!(until(*left.clone(), *right.clone()), *formula);
        assert_eq!(*start, TestTime(0));
        (failed_left, failed_right)
    }
    fn assert_observation(
        violation: &Violation<D>,
        variable: Variable,
        time: u64,
    ) {
        assert_eq!(
            violation,
            &Violation::False {
                time: TestTime(time),
                condition: format!("{variable:?}=false"),
                state: (),
            }
        );
    }

    #[test]
    fn until_preserves_both_failure_observations() {
        for (left_next, right_next) in
            [(false, false), (true, false), (false, true), (true, true)]
        {
            let p = atom(Variable::X);
            let q = atom(Variable::Y);
            let formula = until(
                if left_next { next(p) } else { p },
                if right_next { next(q) } else { q },
            );
            let initial = evaluate(&formula, (false, false));
            let value = match initial {
                Value::Residual(residual) => {
                    step(&residual, (false, !right_next), 1)
                }
                value => value,
            };
            let Value::False(violation, None) = value else {
                panic!("expected failure, got {value:?}")
            };
            let (left, right) = before_right(&violation, &formula);
            assert_observation(left, Variable::X, u64::from(left_next));
            assert_observation(right, Variable::Y, u64::from(right_next));
        }
    }

    #[test]
    fn until_preserves_pending_context_at_test_end() {
        for (left_pending, right_pending) in
            [(true, false), (false, true), (true, true)]
        {
            let p = atom(Variable::X);
            let q = atom(Variable::Y);
            let formula = until(
                if left_pending { eventually(p) } else { p },
                if right_pending { eventually(q) } else { q },
            );
            let residual = pending(evaluate(&formula, (false, false)));
            let Some(StopDefault::False(violation)) =
                stop_default(&residual, TestTime(0))
            else {
                panic!("expected test-end failure")
            };
            let (left, right) = before_right(&violation, &formula);
            for (child, variable, temporal) in [
                (left, Variable::X, left_pending),
                (right, Variable::Y, right_pending),
            ] {
                if temporal {
                    assert!(matches!(
                        child,
                        Violation::Eventually {
                            reason: EventuallyViolation::TestEnded,
                            ..
                        }
                    ));
                } else {
                    assert_observation(child, variable, 0);
                }
            }
        }
        let residual = pending(evaluate(
            &until(atom(Variable::X), atom(Variable::Y)),
            (true, false),
        ));
        assert!(matches!(
            stop_default(&residual, TestTime(0)),
            Some(StopDefault::False(Violation::Until {
                reason: UntilViolation::TestEnded,
                ..
            }))
        ));
    }

    #[test]
    fn repeated_until_obligations_stay_small() {
        let p = atom(Variable::X);
        let q = atom(Variable::Y);
        let formulas = [
            (until(p.clone(), q.clone()), (true, false)),
            (until(p.clone(), eventually(q.clone())), (true, false)),
            (
                until(
                    p.clone(),
                    Formula::Or(
                        Box::new(eventually(q.clone())),
                        Box::new(eventually(atom(Variable::Z))),
                    ),
                ),
                (true, false),
            ),
            (
                until(eventually(p.clone()), eventually(q.clone())),
                (false, false),
            ),
            (until(eventually(p), q), (false, false)),
        ];
        for (formula, current) in formulas {
            let mut residual = pending(evaluate(&formula, current));
            for time in 1..=2000 {
                let size = residual.size();
                assert!(
                    size.nodes <= 10 && size.depth <= 5,
                    "grew at {time}: {size:?}, {formula:?}"
                );
                residual = pending(step(&residual, current, time));
            }
            assert!(matches!(
                step(&residual, (true, true), 2001),
                Value::True(_)
            ));
        }
    }

    #[test]
    fn until_reevaluates_right_thunks() {
        let formula = until(
            Formula::Pure {
                value: true,
                pretty: "true".into(),
            },
            atom(Variable::Y),
        );
        let time = Cell::new(0);
        let mut thunk = |_: &Variable, _: bool| -> Result<_, ()> {
            Ok((
                eventually(Formula::Pure {
                    value: time.get() > 0,
                    pretty: format!("frozen at {}", time.get()),
                }),
                (),
            ))
        };
        let mut evaluator = Evaluator::new(&mut thunk);
        let residual =
            pending(evaluator.evaluate(&formula, TestTime(0)).unwrap());
        time.set(1);
        assert!(matches!(
            evaluator.step(&residual, TestTime(1)).unwrap(),
            Value::True(_)
        ));
    }

    #[test]
    fn earlier_pending_right_can_rescue_a_later_left_failure() {
        let formula = until(atom(Variable::X), next(next(atom(Variable::Y))));
        assert!(run(
            &formula,
            &[(true, false), (false, false), (false, false), (false, true)]
        ));
        let formula = until(atom(Variable::X), always(atom(Variable::Y)));
        let residual = pending(evaluate(&formula, (true, true)));
        let residual = pending(step(&residual, (false, true), 1));
        assert!(matches!(
            stop_default(&residual, TestTime(1)),
            Some(StopDefault::True(_))
        ));
        let Value::False(violation, None) = step(&residual, (false, false), 2)
        else {
            panic!("expected failed pending witnesses")
        };
        let (left, right) = before_right(&violation, &formula);
        assert_observation(left, Variable::X, 1);
        assert!(matches!(right, Violation::Always { .. }));
    }

    #[test]
    fn until_keeps_distinct_right_deadlines() {
        let formula = until(
            atom(Variable::X),
            Formula::Eventually(
                Box::new(atom(Variable::Y)),
                Some(Duration::from_millis(1)),
            ),
        );
        assert!(!run(
            &formula,
            &[(false, false), (false, false), (false, true)]
        ));
        assert!(run(
            &formula,
            &[(true, false), (false, false), (false, true)]
        ));
    }

    fn denotation(formula: &F, trace: &[(bool, bool)], at: usize) -> bool {
        match formula {
            Formula::Pure { value, .. } => *value,
            Formula::Thunk { function, negated } => {
                let value = match function {
                    Variable::X => trace[at].0,
                    Variable::Y => trace[at].1,
                    Variable::Z => false,
                };
                value != *negated
            }
            Formula::And(left, right) => {
                denotation(left, trace, at) && denotation(right, trace, at)
            }
            Formula::Or(left, right) => {
                denotation(left, trace, at) || denotation(right, trace, at)
            }
            Formula::Implies(left, right) => {
                !denotation(left, trace, at) || denotation(right, trace, at)
            }
            Formula::Next(sub) => {
                at + 1 == trace.len() || denotation(sub, trace, at + 1)
            }
            Formula::Always(sub, None) => {
                (at..trace.len()).all(|i| denotation(sub, trace, i))
            }
            Formula::Eventually(sub, None) => {
                (at..trace.len()).any(|i| denotation(sub, trace, i))
            }
            Formula::Until(left, right) => (at..trace.len()).any(|witness| {
                denotation(right, trace, witness)
                    && (at..witness).all(|i| denotation(left, trace, i))
            }),
            _ => panic!("bounded operators are not generated"),
        }
    }
    fn run(formula: &F, trace: &[(bool, bool)]) -> bool {
        let mut value = evaluate(formula, trace[0]);
        for (time, &current) in trace.iter().enumerate().skip(1) {
            let Value::Residual(residual) = value else {
                break;
            };
            value = step(&residual, current, time as u64);
        }
        match value {
            Value::True(_) => true,
            Value::False(_, _) => false,
            Value::Residual(residual) => matches!(
                stop_default(&residual, TestTime(trace.len() as u64 - 1)),
                Some(StopDefault::True(_))
            ),
        }
    }

    #[test]
    fn until_matches_finite_trace_denotation() {
        let p = atom(Variable::X);
        let q = atom(Variable::Y);
        let operands = [
            p.clone(),
            q.clone(),
            next(p.clone()),
            next(q.clone()),
            always(p.clone()),
            always(q.clone()),
            eventually(p.clone()),
            eventually(q.clone()),
            Formula::And(Box::new(p.clone()), Box::new(q.clone())),
            Formula::Or(Box::new(p.clone()), Box::new(q.clone())),
            Formula::Implies(Box::new(p.clone()), Box::new(q.clone())),
            until(p, q.clone()),
        ];
        let mut formulas = Vec::new();
        for left in &operands {
            for right in &operands {
                formulas.push(until(left.clone(), right.clone()));
            }
            formulas.extend([
                next(until(left.clone(), q.clone())),
                always(until(left.clone(), q.clone())),
                eventually(until(left.clone(), q.clone())),
            ]);
        }
        for len in 1..=4usize {
            for code in 0..4usize.pow(len as u32) {
                let trace: Vec<_> = (0..len)
                    .map(|i| {
                        let bits = (code >> (2 * i)) & 3;
                        (bits & 1 != 0, bits & 2 != 0)
                    })
                    .collect();
                for formula in &formulas {
                    assert_eq!(
                        run(formula, &trace),
                        denotation(formula, &trace, 0),
                        "{formula:?}, {trace:?}"
                    );
                }
            }
        }
    }
}
