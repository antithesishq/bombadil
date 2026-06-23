use std::{cell::RefCell, ops::Add, time::Duration};

use anyhow::Error;

use crate::{
    eval::*,
    formula::*,
    stop::{StopDefault, stop_default},
};
use hegel::{
    Generator, TestCase,
    generators::{booleans, deferred, integers, just, one_of, optional, vecs},
    tuples,
};

use crate::syntax::Syntax;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct TestTime(u64);

impl Ord for TestTime {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.cmp(&other.0)
    }
}

impl PartialOrd for TestTime {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Add<Duration> for TestTime {
    type Output = Self;
    fn add(self, rhs: Duration) -> Self {
        TestTime(self.0 + rhs.as_millis() as u64)
    }
}

#[derive(Clone, Debug, PartialEq)]
struct TestDomain;

impl Domain for TestDomain {
    type Function = Thunk;
    type Time = TestTime;
    type Duration = Duration;
    type State = ();
}

#[derive(Debug)]
struct TraceState {
    x: bool,
    y: bool,
}

fn state() -> impl Generator<TraceState> {
    tuples!(booleans(), booleans(),)
        .map(|(x, y)| TraceState { x, y })
        .boxed()
}

fn trace() -> impl Generator<Vec<TraceState>> {
    vecs(state()).min_size(1).max_size(10).boxed()
}

#[derive(Copy, Clone, Debug, PartialEq)]
enum Variable {
    X,
    Y,
}

fn variable() -> impl Generator<Variable> {
    one_of([just(Variable::X).boxed(), just(Variable::Y).boxed()]).boxed()
}

fn bound() -> impl Generator<Option<Duration>> {
    optional(
        integers()
            .min_value(0)
            .max_value(10)
            .map(Duration::from_millis),
    )
    .boxed()
}

#[derive(Clone, Debug, PartialEq)]
enum Thunk {
    Atomic(Variable),
    Subformula(Box<Syntax<TestDomain>>),
}

fn syntax() -> impl Generator<Syntax<TestDomain>> {
    let syntax = deferred::<Syntax<TestDomain>>();

    let leaf = one_of([
        booleans()
            .map(|value| Syntax::Pure {
                value,
                pretty: format!("{}", value),
            })
            .boxed(),
        variable()
            .map(|value| Syntax::Thunk(Thunk::Atomic(value)))
            .boxed(),
    ])
    .boxed();

    let branch = one_of([
        syntax
            .generator()
            .map(|subformula| {
                Syntax::Thunk(Thunk::Subformula(Box::new(subformula)))
            })
            .boxed(),
        syntax
            .generator()
            .map(|subformula| Syntax::Not(Box::new(subformula)))
            .boxed(),
        tuples!(syntax.generator(), syntax.generator())
            .map(|(left, right)| Syntax::And(Box::new(left), Box::new(right)))
            .boxed(),
        tuples!(syntax.generator(), syntax.generator())
            .map(|(left, right)| Syntax::Or(Box::new(left), Box::new(right)))
            .boxed(),
        tuples!(syntax.generator(), syntax.generator())
            .map(|(left, right)| {
                Syntax::Implies(Box::new(left), Box::new(right))
            })
            .boxed(),
        syntax
            .generator()
            .map(|subformula| Syntax::Next(Box::new(subformula)))
            .boxed(),
        tuples!(syntax.generator(), bound())
            .map(|(subformula, bound)| {
                Syntax::Always(Box::new(subformula), bound)
            })
            .boxed(),
        tuples!(syntax.generator(), bound())
            .map(|(subformula, bound)| {
                Syntax::Eventually(Box::new(subformula), bound)
            })
            .boxed(),
    ]);

    let result = syntax.generator();
    syntax.set(one_of([leaf.boxed(), branch.boxed()]));
    result
}

#[derive(Copy, Clone, Debug, PartialEq)]
enum ValueEqMode {
    Strict,
    UpToViolations,
}

fn assert_values_eq(
    value_left: Value<TestDomain>,
    value_right: Value<TestDomain>,
    time: TestTime,
    mode: ValueEqMode,
) {
    match (&value_left, &value_right) {
        (Value::True(_), Value::True(_)) => {}
        (Value::False(left, _), Value::False(right, _)) => {
            if mode == ValueEqMode::Strict {
                assert_eq!(left, right);
            }
        }
        (Value::Residual(left), Value::Residual(right)) => {
            let default_left = stop_default(left, time);
            let default_right = stop_default(right, time);
            match mode {
                ValueEqMode::Strict => assert_eq!(default_left, default_right),
                ValueEqMode::UpToViolations => {
                    match (default_left, default_right) {
                        (None, None) => {}
                        (
                            Some(StopDefault::True(_)),
                            Some(StopDefault::True(_)),
                        ) => {}
                        (
                            Some(StopDefault::False(_)),
                            Some(StopDefault::False(_)),
                        ) => {}
                        (left, right) => {
                            panic!("\n{:?}\n\n!=\n\n{:?}\n", left, right)
                        }
                    }
                }
            }
        }
        (left, right) => panic!("\n{:?}\n\n!=\n\n{:?}\n", left, right),
    }
}

fn next_residual(value: &Value<TestDomain>) -> Option<Residual<TestDomain>> {
    match value {
        Value::Residual(r) => Some(r.clone()),
        Value::False(_, Some(c)) => Some(c.clone()),
        _ => None,
    }
}

fn check_equivalence(
    formula_left: Formula<TestDomain>,
    formula_right: Formula<TestDomain>,
    trace: Vec<TraceState>,
    mode: ValueEqMode,
) {
    let current = RefCell::new(0);
    let mut evaluate_thunk = |thunk: &Thunk, negated| match thunk {
        Thunk::Atomic(variable) => {
            let state = &trace[*current.borrow()];

            let value = match variable {
                Variable::X => state.x,
                Variable::Y => state.y,
            };
            let value = if negated { !value } else { value };
            Ok((
                Formula::Pure {
                    value,
                    pretty: format!("{}", value),
                },
                (),
            ))
        }
        Thunk::Subformula(syntax) => {
            let syntax = if negated {
                Syntax::Not(syntax.clone())
            } else {
                *syntax.clone()
            };
            Ok((syntax.nnf(), ()))
        }
    };
    let mut evaluator: Evaluator<'_, TestDomain, Error> =
        Evaluator::new(&mut evaluate_thunk);

    let mut time = TestTime(0);

    let mut value_left = evaluator.evaluate(&formula_left, time).unwrap();
    let mut value_right = evaluator.evaluate(&formula_right, time).unwrap();

    for _ in 1..trace.len() {
        *current.borrow_mut() += 1;
        time = time + Duration::from_millis(1);

        let next_left = next_residual(&value_left);
        let next_right = next_residual(&value_right);

        match (next_left, next_right) {
            (Some(left), Some(right)) => {
                value_left = evaluator.step(&left, time).unwrap();
                value_right = evaluator.step(&right, time).unwrap();
            }
            _ => break,
        }
    }

    assert_values_eq(value_left, value_right, time, mode);
}

// Properties organically sourced from: https://en.wikipedia.org/wiki/Linear_temporal_logic

// Distributivity

// X(φ ∨ ψ) ⇔ (X φ) ∨ (X ψ)
#[hegel::test]
fn test_next_disjunction_distributivity(tc: TestCase) {
    let φ = tc.draw(syntax());
    let ψ = tc.draw(syntax());
    let trace = tc.draw(trace());

    let formula_left = Syntax::Next(Box::new(Syntax::Or(
        Box::new(φ.clone()),
        Box::new(ψ.clone()),
    )))
    .nnf();
    let formula_right = Syntax::Or(
        Box::new(Syntax::Next(Box::new(φ.clone()))),
        Box::new(Syntax::Next(Box::new(ψ.clone()))),
    )
    .nnf();
    check_equivalence(
        formula_left,
        formula_right,
        trace,
        ValueEqMode::UpToViolations,
    );
}
/*

// X (φ ∧ ψ) ⇔ (X φ) ∧ (X ψ)
#[test]
fn test_next_conjunction_distributivity(φ in syntax(), ψ in syntax(), trace in trace()) {
    let formula_left =
        Syntax::Next(Box::new(Syntax::And(Box::new(φ.clone()), Box::new(ψ.clone())))).nnf();
    let formula_right =
        Syntax::And(Box::new(Syntax::Next(Box::new(φ.clone()))), Box::new(Syntax::Next(Box::new(ψ.clone())))).nnf();
    check_equivalence(formula_left, formula_right, trace, ValueEqMode::UpToViolations);
}

// F(φ ∨ ψ) ⇔ (F φ) ∨ (F ψ)
#[test]
fn test_eventually_disjunction_distributivity(φ in syntax(), ψ in syntax(), bound in bound(), trace in trace()) {
    let formula_left =
        Syntax::Eventually(Box::new(Syntax::Or(Box::new(φ.clone()), Box::new(ψ.clone()))), bound).nnf();
    let formula_right =
        Syntax::Or(Box::new(Syntax::Eventually(Box::new(φ.clone()), bound)), Box::new(Syntax::Eventually(Box::new(ψ.clone()), bound))).nnf();
    check_equivalence(formula_left, formula_right, trace, ValueEqMode::UpToViolations);
}

// G(φ ∧ ψ) ⇔ (G φ) ∧ (G ψ)
#[test]
fn test_always_conjunction_distributivity(φ in syntax(), ψ in syntax(), bound in bound(), trace in trace()) {
    let formula_left =
        Syntax::Always(Box::new(Syntax::And(Box::new(φ.clone()), Box::new(ψ.clone()))), bound).nnf();
    let formula_right =
        Syntax::And(Box::new(Syntax::Always(Box::new(φ.clone()), bound)), Box::new(Syntax::Always(Box::new(ψ.clone()), bound))).nnf();
    check_equivalence(formula_left, formula_right, trace, ValueEqMode::UpToViolations);
}


// Negation propagation

// X(¬φ) ⇔ ¬X(φ)
#[test]
fn test_next_self_duality(φ in syntax(), trace in trace()) {
    let formula_left =
        Syntax::Next(Box::new(Syntax::Not(Box::new(φ.clone())))).nnf();
    let formula_right =
        Syntax::Not(Box::new(Syntax::Next(Box::new(φ.clone())))).nnf();
    check_equivalence(formula_left, formula_right, trace, ValueEqMode::Strict);
}

// G(¬φ) ⇔ ¬F(φ)
#[test]
fn test_always_eventually_duality(φ in syntax(), trace in trace()) {
    let formula_left =
        Syntax::Always(Box::new(Syntax::Not(Box::new(φ.clone()))), None).nnf();
    let formula_right =
        Syntax::Not(Box::new(Syntax::Eventually(Box::new(φ.clone()), None))).nnf();
    check_equivalence(formula_left, formula_right, trace, ValueEqMode::Strict);
}

// F(φ) ⇔ F(F(φ))
#[test]
fn test_eventually_idempotency(φ in syntax(), trace in trace()) {
    let formula_left =
        Syntax::Eventually(Box::new(φ.clone()), None).nnf();
    let formula_right =
        Syntax::Eventually(Box::new(Syntax::Eventually(Box::new(φ.clone()), None)), None).nnf();
    check_equivalence(formula_left, formula_right, trace, ValueEqMode::UpToViolations);
}

// G(φ) ⇔ G(G(φ))
#[test]
fn test_always_idempotency(φ in syntax(), trace in trace()) {
    let formula_left =
        Syntax::Always(Box::new(φ.clone()), None).nnf();
    let formula_right =
        Syntax::Always(Box::new(Syntax::Always(Box::new(φ.clone()), None)), None).nnf();
    check_equivalence(formula_left, formula_right, trace, ValueEqMode::UpToViolations);
}

*/
