use std::{
    cell::RefCell,
    time::{Duration, UNIX_EPOCH},
};

use crate::specification::{
    ltl::*,
    stop::{stop_default, StopDefault},
};
use proptest::prelude::*;

use crate::specification::syntax::Syntax;

#[derive(Debug)]
struct State {
    x: bool,
    y: bool,
}

fn state() -> BoxedStrategy<State> {
    any::<(bool, bool)>()
        .prop_map(|(x, y)| State { x, y })
        .boxed()
}

fn trace() -> BoxedStrategy<Vec<State>> {
    prop::collection::vec(state(), 1..10).boxed()
}

#[derive(Copy, Clone, Debug, PartialEq)]
enum Variable {
    X,
    Y,
}

fn variable() -> BoxedStrategy<Variable> {
    use Variable::*;
    prop_oneof![Just(X), Just(Y)].boxed()
}

fn bound() -> BoxedStrategy<Option<Duration>> {
    prop::option::of((0..10u64).prop_map(Duration::from_millis)).boxed()
}

#[derive(Clone, Debug, PartialEq)]
enum Thunk {
    Atomic(Variable),
    Subformula(Box<Syntax<Thunk>>),
}

fn syntax() -> BoxedStrategy<Syntax<Thunk>> {
    let leaf = prop_oneof![
        // leaf nodes
        any::<bool>().prop_map(|value| Syntax::Pure {
            value,
            pretty: format!("{}", value)
        }),
        variable().prop_map(|value| Syntax::Thunk(Thunk::Atomic(value))),
    ]
    .boxed();

    leaf.prop_recursive(8, 256, 10, |inner| {
        // recursive nodes
        prop_oneof![
            inner.clone().prop_map(|subformula| {
                Syntax::Thunk(Thunk::Subformula(Box::new(subformula)))
            }),
            inner
                .clone()
                .prop_map(|subformula| { Syntax::Not(Box::new(subformula)) }),
            (inner.clone(), inner.clone()).prop_map(|(left, right)| {
                Syntax::And(Box::new(left), Box::new(right))
            }),
            (inner.clone(), inner.clone()).prop_map(|(left, right)| {
                Syntax::Or(Box::new(left), Box::new(right))
            }),
            (inner.clone(), inner.clone()).prop_map(|(left, right)| {
                Syntax::Implies(Box::new(left), Box::new(right))
            }),
            inner
                .clone()
                .prop_map(|subformula| { Syntax::Next(Box::new(subformula)) }),
            (inner.clone(), bound()).prop_map(|(subformula, bound)| {
                Syntax::Always(Box::new(subformula), bound)
            }),
            (inner.clone(), bound()).prop_map(|(subformula, bound)| {
                Syntax::Eventually(Box::new(subformula), bound)
            }),
        ]
    })
    .boxed()
}

#[derive(Copy, Clone, Debug, PartialEq)]
enum ValueEqMode {
    Strict,
    UpToViolations,
}

fn assert_values_eq<Function: Clone + PartialEq + std::fmt::Debug>(
    value_left: Value<Function>,
    value_right: Value<Function>,
    time: Time,
    mode: ValueEqMode,
) {
    match (value_left, value_right) {
        (Value::True, Value::True) => {}
        (Value::False(left), Value::False(right)) => {
            if mode == ValueEqMode::Strict {
                assert_eq!(left, right);
            }
        }
        (Value::Residual(left), Value::Residual(right)) => {
            let default_left = stop_default(&left, time);
            let default_right = stop_default(&right, time);
            match mode {
                ValueEqMode::Strict => assert_eq!(default_left, default_right),
                ValueEqMode::UpToViolations => {
                    match (default_left, default_right) {
                        (None, None) => {}
                        (Some(StopDefault::True), Some(StopDefault::True)) => {}
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

fn check_equivalence(
    formula_left: Formula<Thunk>,
    formula_right: Formula<Thunk>,
    trace: Vec<State>,
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
            Ok(Formula::Pure {
                value,
                pretty: format!("{}", value),
            })
        }
        Thunk::Subformula(syntax) => {
            let syntax = if negated {
                Syntax::Not(syntax.clone())
            } else {
                *syntax.clone()
            };
            Ok(syntax.nnf())
        }
    };
    let mut evaluator = Evaluator::new(&mut evaluate_thunk);

    let mut time = UNIX_EPOCH;

    let mut value_left = evaluator.evaluate(&formula_left, time).unwrap();
    let mut value_right = evaluator.evaluate(&formula_right, time).unwrap();

    for _ in 1..trace.len() {
        *current.borrow_mut() += 1;
        time = time.checked_add(Duration::from_millis(1)).unwrap();

        if let Value::Residual(left) = &value_left
            && let Value::Residual(right) = &value_right
        {
            value_left = evaluator.step(left, time).unwrap();
            value_right = evaluator.step(right, time).unwrap();
        } else {
            break;
        }
    }

    assert_values_eq(value_left, value_right, time, mode);
}

// Properties organically sourced from: https://en.wikipedia.org/wiki/Linear_temporal_logic

// Distributivity
proptest! {
    // X(φ ∨ ψ) ⇔ (X φ) ∨ (X ψ)
    #[test]
    fn test_next_disjunction_distributivity(φ in syntax(), ψ in syntax(), trace in trace()) {
        let formula_left =
            Syntax::Next(Box::new(Syntax::Or(Box::new(φ.clone()), Box::new(ψ.clone())))).nnf();
        let formula_right =
            Syntax::Or(Box::new(Syntax::Next(Box::new(φ.clone()))), Box::new(Syntax::Next(Box::new(ψ.clone())))).nnf();
        check_equivalence(formula_left, formula_right, trace, ValueEqMode::UpToViolations);
    }

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
}

// Negation propagation
proptest! {
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
}
