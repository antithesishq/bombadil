use std::time::{Duration, SystemTime};

use crate::specification::{
    bombadil_exports::BombadilExports,
    result::{Result, SpecificationError},
};
use boa_engine::{js_string, Context, JsObject, JsValue};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeFunction {
    pub object: JsObject,
    pub pretty: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PrettyFunction(String);

impl std::fmt::Display for PrettyFunction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A formula in its syntactic form, "parsed" from JavaScript runtime objects.
#[derive(Debug, Clone, PartialEq)]
pub enum Syntax {
    Pure { value: bool, pretty: String },
    Thunk(RuntimeFunction),
    Not(Box<Syntax>),
    And(Box<Syntax>, Box<Syntax>),
    Or(Box<Syntax>, Box<Syntax>),
    Implies(Box<Syntax>, Box<Syntax>),
    Next(Box<Syntax>),
    Always(Box<Syntax>, Option<Duration>),
    Eventually(Box<Syntax>, Option<Duration>),
}

impl Syntax {
    pub fn from_value(
        value: &JsValue,
        bombadil: &BombadilExports,
        context: &mut Context,
    ) -> Result<Self> {
        use Syntax::*;

        let object =
            value.as_object().ok_or(SpecificationError::OtherError(
                format!("formula is not an object: {}", value.display()),
            ))?;

        if value.instance_of(&bombadil.pure, context)? {
            let value = object
                .get(js_string!("value"), context)?
                .as_boolean()
                .ok_or(SpecificationError::OtherError(
                    "Pure.value is not a boolean".to_string(),
                ))?;
            let pretty = object
                .get(js_string!("pretty"), context)?
                .as_string()
                .ok_or(SpecificationError::OtherError(
                    "Pure.pretty is not a string".to_string(),
                ))?
                .to_std_string_escaped();
            return Ok(Self::Pure { value, pretty });
        }

        if value.instance_of(&bombadil.thunk, context)? {
            let apply_object = object
                .get(js_string!("apply"), context)?
                .as_callable()
                .ok_or(SpecificationError::OtherError(
                    "Thunk.apply is not callable".to_string(),
                ))?;
            let pretty_value = object.get(js_string!("pretty"), context)?;
            let pretty = pretty_value
                .as_string()
                .ok_or(SpecificationError::OtherError(format!(
                    "Thunk.pretty is not a string: {}",
                    pretty_value.display()
                )))?
                .to_std_string_escaped();
            return Ok(Self::Thunk(RuntimeFunction {
                object: apply_object,
                pretty,
            }));
        }

        if value.instance_of(&bombadil.not, context)? {
            let value = object.get(js_string!("subformula"), context)?;
            let subformula = Self::from_value(&value, bombadil, context)?;
            return Ok(Not(Box::new(subformula)));
        }

        if value.instance_of(&bombadil.and, context)? {
            let left_value = object.get(js_string!("left"), context)?;
            let right_value = object.get(js_string!("right"), context)?;
            let left = Self::from_value(&left_value, bombadil, context)?;
            let right = Self::from_value(&right_value, bombadil, context)?;
            return Ok(And(Box::new(left), Box::new(right)));
        }

        if value.instance_of(&bombadil.or, context)? {
            let left_value = object.get(js_string!("left"), context)?;
            let right_value = object.get(js_string!("right"), context)?;
            let left = Self::from_value(&left_value, bombadil, context)?;
            let right = Self::from_value(&right_value, bombadil, context)?;
            return Ok(Or(Box::new(left), Box::new(right)));
        }

        if value.instance_of(&bombadil.implies, context)? {
            let left_value = object.get(js_string!("left"), context)?;
            let right_value = object.get(js_string!("right"), context)?;
            let left = Self::from_value(&left_value, bombadil, context)?;
            let right = Self::from_value(&right_value, bombadil, context)?;
            return Ok(Implies(Box::new(left), Box::new(right)));
        }

        if value.instance_of(&bombadil.next, context)? {
            let subformula_value =
                object.get(js_string!("subformula"), context)?;
            let subformula =
                Self::from_value(&subformula_value, bombadil, context)?;
            return Ok(Next(Box::new(subformula)));
        }

        if value.instance_of(&bombadil.always, context)? {
            let subformula_value =
                object.get(js_string!("subformula"), context)?;
            let subformula =
                Self::from_value(&subformula_value, bombadil, context)?;
            let bound = optional_duration_from_js(
                object.get(js_string!("bound"), context)?,
                context,
            )?;
            return Ok(Always(Box::new(subformula), bound));
        }

        if value.instance_of(&bombadil.eventually, context)? {
            let subformula_value =
                object.get(js_string!("subformula"), context)?;
            let subformula =
                Self::from_value(&subformula_value, bombadil, context)?;
            let bound = optional_duration_from_js(
                object.get(js_string!("bound"), context)?,
                context,
            )?;
            return Ok(Eventually(Box::new(subformula), bound));
        }

        Err(SpecificationError::OtherError(format!(
            "can't convert to formula: {}",
            value.display()
        )))
    }

    pub fn nnf(&self) -> Formula<RuntimeFunction> {
        fn go(node: &Syntax, negated: bool) -> Formula<RuntimeFunction> {
            match node {
                Syntax::Pure { value, pretty } => Formula::Pure {
                    value: if negated { !*value } else { *value },
                    pretty: pretty.clone(),
                },
                Syntax::Thunk(function) => Formula::Thunk {
                    function: function.clone(),
                    negated,
                },
                Syntax::Not(syntax) => go(syntax, !negated),
                Syntax::And(left, right) => {
                    if negated {
                        //   ¬(l ∧ r)
                        // ⇔ (¬l ∨ ¬r)
                        Formula::Or(
                            Box::new(go(left, negated)),
                            Box::new(go(right, negated)),
                        )
                    } else {
                        Formula::And(
                            Box::new(go(left, negated)),
                            Box::new(go(right, negated)),
                        )
                    }
                }
                Syntax::Or(left, right) => {
                    if negated {
                        //   ¬(l ∨ r)
                        // ⇔ (¬l ∧ ¬r)
                        Formula::And(
                            Box::new(go(left, negated)),
                            Box::new(go(right, negated)),
                        )
                    } else {
                        Formula::Or(
                            Box::new(go(left, negated)),
                            Box::new(go(right, negated)),
                        )
                    }
                }
                Syntax::Implies(left, right) => {
                    if negated {
                        //   ¬(l ⇒ r)
                        // ⇔ ¬(¬l ∨ r)
                        // ⇔ l ∧ ¬r
                        Formula::And(
                            Box::new(go(left, false)),
                            Box::new(go(right, true)),
                        )
                    } else {
                        Formula::Implies(
                            Box::new(go(left, negated)),
                            Box::new(go(right, negated)),
                        )
                    }
                }
                Syntax::Next(sub) => Formula::Next(Box::new(go(sub, negated))),
                Syntax::Always(sub, bound) => {
                    if negated {
                        Formula::Eventually(Box::new(go(sub, negated)), *bound)
                    } else {
                        Formula::Always(Box::new(go(sub, negated)), *bound)
                    }
                }
                Syntax::Eventually(sub, bound) => {
                    if negated {
                        Formula::Always(Box::new(go(sub, negated)), *bound)
                    } else {
                        Formula::Eventually(Box::new(go(sub, negated)), *bound)
                    }
                }
            }
        }
        go(self, false)
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

impl Formula<RuntimeFunction> {
    pub fn with_pretty_functions(&self) -> Formula<PrettyFunction> {
        self.map_function(|f| PrettyFunction(f.pretty.clone()))
    }
}

impl<Function: Clone> Formula<Function> {
    fn map_function<Result>(
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

fn optional_duration_from_js(
    value: JsValue,
    context: &mut Context,
) -> Result<Option<Duration>> {
    if value.is_null_or_undefined() {
        return Ok(None);
    }

    let object =
        value
            .as_object()
            .ok_or(SpecificationError::OtherError(format!(
                "duration is not an object: {}",
                value.display()
            )))?;
    let milliseconds_value = object.get(js_string!("milliseconds"), context)?;

    let milliseconds = milliseconds_value.as_number().ok_or(
        SpecificationError::OtherError(format!(
            "milliseconds is not a number: {}",
            milliseconds_value.display()
        )),
    )?;
    if milliseconds < 0.0 {
        return Err(SpecificationError::OtherError(format!(
            "milliseconds is negative: {}",
            milliseconds_value.display()
        )));
    }
    if milliseconds.is_infinite() {
        return Err(SpecificationError::OtherError(format!(
            "milliseconds is {}",
            milliseconds_value.display()
        )));
    }
    Ok(Some(Duration::from_millis(milliseconds as u64)))
}

pub type Time = SystemTime;

#[derive(Clone, Debug)]
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

impl Violation<RuntimeFunction> {
    pub fn with_pretty_functions(&self) -> Violation<PrettyFunction> {
        self.map_function(|f| PrettyFunction(f.pretty.clone()))
    }
}

impl<Function: Clone> Violation<Function> {
    fn map_function<Result>(
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

#[derive(Clone, Debug)]
pub enum Leaning<Function> {
    AssumeTrue,
    AssumeFalse(Violation<Function>),
}

#[derive(Clone, Debug)]
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

#[derive(Clone, Debug)]
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
    // bombadil_exports: &'a BombadilExports,
    // context: &'a mut Context,
}

impl<'a, Function: Clone> Evaluator<'a, Function> {
    pub fn new(
        // bombadil_exports: &'a BombadilExports,
        // context: &'a mut Context,
        evaluate_thunk: EvaluateThunk<'a, Function>,
    ) -> Self {
        Evaluator {
            evaluate_thunk, // bombadil_exports,
                            // context,
        }
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
