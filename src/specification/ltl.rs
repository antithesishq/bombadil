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

#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum Formula<Function = RuntimeFunction> {
    True { pretty: String },
    False { pretty: String },
    Contextful(Function),
    And(Box<Formula<Function>>, Box<Formula<Function>>),
    Or(Box<Formula<Function>>, Box<Formula<Function>>),
    Implies(Box<Formula<Function>>, Box<Formula<Function>>),
    Next(Box<Formula<Function>>),
    Always(Box<Formula<Function>>),
    Eventually(Box<Formula<Function>>, Duration),
}

impl Formula {
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
            Formula::True { pretty } => Formula::True {
                pretty: pretty.clone(),
            },
            Formula::False { pretty } => Formula::False {
                pretty: pretty.clone(),
            },
            Formula::Contextful(function) => Formula::Contextful(f(function)),
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
            Formula::Always(formula) => {
                Formula::Always(Box::new(formula.clone().map_function_ref(f)))
            }
            Formula::Eventually(formula, timeout) => Formula::Eventually(
                Box::new(formula.clone().map_function_ref(f)),
                *timeout,
            ),
        }
    }
}

impl Formula {
    pub fn from_value(
        value: &JsValue,
        bombadil: &BombadilExports,
        context: &mut Context,
    ) -> Result<Self> {
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
            return Ok(if value {
                Self::True { pretty }
            } else {
                Self::False { pretty }
            });
        }

        if value.instance_of(&bombadil.thunk, context)? {
            let apply_object = object
                .get(js_string!("apply"), context)?
                .as_callable()
                .ok_or(SpecificationError::OtherError(
                    "Contextful.apply is not callable".to_string(),
                ))?;
            let pretty_value = object.get(js_string!("pretty"), context)?;
            let pretty = pretty_value
                .as_string()
                .ok_or(SpecificationError::OtherError(format!(
                    "Contextful.pretty is not a string: {}",
                    pretty_value.display()
                )))?
                .to_std_string_escaped();
            return Ok(Self::Contextful(RuntimeFunction {
                object: apply_object,
                pretty,
            }));
        }

        if value.instance_of(&bombadil.and, context)? {
            let left_value = object.get(js_string!("left"), context)?;
            let right_value = object.get(js_string!("right"), context)?;
            let left = Formula::from_value(&left_value, bombadil, context)?;
            let right = Formula::from_value(&right_value, bombadil, context)?;
            return Ok(Formula::And(Box::new(left), Box::new(right)));
        }

        if value.instance_of(&bombadil.or, context)? {
            let left_value = object.get(js_string!("left"), context)?;
            let right_value = object.get(js_string!("right"), context)?;
            let left = Formula::from_value(&left_value, bombadil, context)?;
            let right = Formula::from_value(&right_value, bombadil, context)?;
            return Ok(Formula::Or(Box::new(left), Box::new(right)));
        }

        if value.instance_of(&bombadil.implies, context)? {
            let left_value = object.get(js_string!("left"), context)?;
            let right_value = object.get(js_string!("right"), context)?;
            let left = Formula::from_value(&left_value, bombadil, context)?;
            let right = Formula::from_value(&right_value, bombadil, context)?;
            return Ok(Formula::Implies(Box::new(left), Box::new(right)));
        }

        if value.instance_of(&bombadil.next, context)? {
            let subformula_value =
                object.get(js_string!("subformula"), context)?;
            let subformula =
                Formula::from_value(&subformula_value, bombadil, context)?;
            return Ok(Formula::Next(Box::new(subformula)));
        }

        if value.instance_of(&bombadil.always, context)? {
            let subformula_value =
                object.get(js_string!("subformula"), context)?;
            let subformula =
                Formula::from_value(&subformula_value, bombadil, context)?;
            return Ok(Formula::Always(Box::new(subformula)));
        }

        if value.instance_of(&bombadil.eventually, context)? {
            let subformula_value =
                object.get(js_string!("subformula"), context)?;
            let subformula =
                Formula::from_value(&subformula_value, bombadil, context)?;

            let timeout = duration_from_js(
                object.get(js_string!("timeout"), context)?,
                context,
            )?;

            return Ok(Formula::Eventually(Box::new(subformula), timeout));
        }

        Err(SpecificationError::OtherError(format!(
            "can't convert to formula: {}",
            value.display()
        )))
    }
}

fn duration_from_js(value: JsValue, context: &mut Context) -> Result<Duration> {
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
    Ok(Duration::from_millis(milliseconds as u64))
}

pub type Time = SystemTime;

#[derive(Clone, Debug)]
pub enum Value {
    True,
    False(Violation),
    Residual(Residual),
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum Violation<Function = RuntimeFunction> {
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

impl Violation {
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
                time,
            } => Violation::Always {
                violation: Box::new(violation.map_function_ref(f)),
                subformula: Box::new(subformula.map_function_ref(f)),
                start: *start,
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
pub enum Leaning<Function = RuntimeFunction> {
    AssumeTrue,
    AssumeFalse(Violation<Function>),
}

#[derive(Clone, Debug)]
pub enum Residual<Function = RuntimeFunction> {
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
        deadline: Time,
        start: Time,
        left: Box<Residual<Function>>,
        right: Box<Residual<Function>>,
    },
    AndAlways {
        subformula: Box<Formula<Function>>,
        start: Time,
        left: Box<Residual<Function>>,
        right: Box<Residual<Function>>,
    },
}

#[derive(Clone, Debug)]
pub enum Derived<Function = RuntimeFunction> {
    Once {
        start: Time,
        subformula: Box<Formula<Function>>,
    },
    Always {
        start: Time,
        subformula: Box<Formula<Function>>,
    },
    Eventually {
        start: Time,
        deadline: Time,
        subformula: Box<Formula<Function>>,
    },
}

pub struct Evaluator<'a> {
    bombadil_exports: &'a BombadilExports,
    context: &'a mut Context,
}

impl<'a> Evaluator<'a> {
    pub fn new(
        bombadil_exports: &'a BombadilExports,
        context: &'a mut Context,
    ) -> Self {
        Evaluator {
            bombadil_exports,
            context,
        }
    }

    pub fn evaluate(&mut self, formula: &Formula, time: Time) -> Result<Value> {
        match formula {
            Formula::True { .. } => Ok(Value::True),
            Formula::False { pretty } => Ok(Value::False(Violation::False {
                time,
                condition: pretty.clone(),
            })),
            Formula::Contextful(function) => {
                let value = function.object.call(
                    &JsValue::undefined(),
                    &[],
                    self.context,
                )?;
                let formula = Formula::from_value(
                    &value,
                    self.bombadil_exports,
                    self.context,
                )?;
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
            Formula::Always(formula) => {
                self.evaluate_always(formula.clone(), time, time)
            }
            Formula::Eventually(formula, timeout) => self.evaluate_eventually(
                formula.clone(),
                time,
                time.checked_add(*timeout).ok_or(
                    SpecificationError::OtherError(
                        "failed to add timeout to time".to_string(),
                    ),
                )?,
                time,
            ),
        }
    }

    fn evaluate_and(&mut self, left: &Value, right: &Value) -> Value {
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

    fn evaluate_or(&mut self, left: &Value, right: &Value) -> Value {
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
        left_formula: &Formula,
        left: &Value,
        right: &Value,
    ) -> Value {
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
        subformula: Box<Formula>,
        start: Time,
        time: Time,
    ) -> Result<Value> {
        let residual = Residual::Derived(
            Derived::Always {
                subformula: subformula.clone(),
                start,
            },
            Leaning::AssumeTrue,
        );

        Ok(match self.evaluate(&subformula, time)? {
            Value::True => Value::Residual(residual),
            Value::False(violation) => Value::False(Violation::Always {
                violation: Box::new(violation),
                subformula: subformula.clone(),
                start,
                time,
            }),
            Value::Residual(left) => Value::Residual(Residual::AndAlways {
                subformula: subformula.clone(),
                start,
                left: Box::new(left),
                right: Box::new(residual),
            }),
        })
    }

    fn evaluate_and_always(
        &mut self,
        subformula: Box<Formula>,
        start: Time,
        time: Time,
        left: Value,
        right: Value,
    ) -> Result<Value> {
        Ok(match (left, right) {
            (Value::True, Value::True) => Value::True,
            (Value::False(violation), _) => Value::False(Violation::Always {
                violation: Box::new(violation.clone()),
                subformula,
                start,
                time,
            }),
            (_, Value::False(violation)) => Value::False(Violation::Always {
                violation: Box::new(violation.clone()),
                subformula,
                start,
                time,
            }),
            (Value::Residual(left), Value::True) => {
                Value::Residual(Residual::AndAlways {
                    subformula,
                    start,
                    left: Box::new(left),
                    right: Box::new(Residual::True),
                })
            }
            (Value::True, Value::Residual(right)) => {
                Value::Residual(Residual::AndAlways {
                    subformula,
                    start,
                    left: Box::new(Residual::True),
                    right: Box::new(right),
                })
            }
            (Value::Residual(left), Value::Residual(right)) => {
                Value::Residual(Residual::AndAlways {
                    subformula,
                    start,
                    left: Box::new(left),
                    right: Box::new(right),
                })
            }
        })
    }

    fn evaluate_eventually(
        &mut self,
        subformula: Box<Formula>,
        start: Time,
        deadline: Time,
        time: Time,
    ) -> Result<Value> {
        if deadline < time {
            return Ok(Value::False(Violation::Eventually {
                subformula: subformula.clone(),
                reason: EventuallyViolation::TimedOut(time),
            }));
        }

        let residual = Residual::Derived(
            Derived::Eventually {
                subformula: subformula.clone(),
                start,
                deadline,
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
                deadline,
                start,
                left: Box::new(left),
                right: Box::new(residual),
            }),
        })
    }

    fn evaluate_or_eventually(
        &mut self,
        subformula: Box<Formula>,
        start: Time,
        deadline: Time,
        time: Time,
        left: Value,
        right: Value,
    ) -> Result<Value> {
        if deadline < time {
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
                    deadline,
                    left: Box::new(left.clone()),
                    right: Box::new(right.clone()),
                })
            }
        })
    }

    pub fn step(&mut self, residual: &Residual, time: Time) -> Result<Value> {
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
                Derived::Always { start, subformula } => {
                    self.evaluate_always(subformula.clone(), *start, time)?
                }
                Derived::Eventually {
                    start,
                    deadline,
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
                deadline,
                start,
                left,
                right,
            } => {
                let left = self.step(left, time)?;
                let right = self.step(right, time)?;

                self.evaluate_or_eventually(
                    subformula.clone(),
                    *start,
                    *deadline,
                    time,
                    left,
                    right,
                )?
            }
            Residual::AndAlways {
                subformula,
                start,
                left,
                right,
            } => {
                let left = self.step(left, time)?;
                let right = self.step(right, time)?;
                self.evaluate_and_always(
                    subformula.clone(),
                    *start,
                    time,
                    left,
                    right,
                )?
            }
        })
    }
}

#[derive(Clone, Debug)]
pub enum StopDefault<Function = RuntimeFunction> {
    True,
    False(Violation<Function>),
}

pub fn stop_default<Function: Clone>(
    residual: &Residual<Function>,
    time: Time,
) -> Option<StopDefault<Function>> {
    use self::Residual::*;
    match residual {
        True => Some(StopDefault::True),
        False(violation) => Some(StopDefault::False(violation.clone())),
        Derived(_, leaning) => match leaning {
            Leaning::AssumeFalse(violation) => {
                Some(StopDefault::False(violation.clone()))
            }
            Leaning::AssumeTrue => Some(StopDefault::True),
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
            left,
            right,
        } => stop_default(left, time).and_then(|s1| {
            stop_default(right, time).map(|s2| {
                stop_and_always_default(subformula, *start, time, &s1, &s2)
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
        (True, right) => right.clone(),
        (left, True) => left.clone(),
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
        (True, _) => True,
        (_, True) => True,
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
        (False(_), _) => True,
        (True, False(violation)) => False(Violation::Implies {
            left: left_formula.clone(),
            right: Box::new(violation.clone()),
        }),
        (True, True) => True,
    }
}

fn stop_and_always_default<Function: Clone>(
    subformula: &Formula<Function>,
    start: Time,
    time: Time,
    left: &StopDefault<Function>,
    right: &StopDefault<Function>,
) -> StopDefault<Function> {
    use StopDefault::*;
    match (left, right) {
        (True, right) => right.clone(),
        (False(violation), _) => StopDefault::False(Violation::Always {
            violation: Box::new(violation.clone()),
            subformula: Box::new(subformula.clone()),
            start,
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
        (True, _) => True,
        (_, True) => True,
        (_, False(right)) => False(right.clone()),
    }
}
