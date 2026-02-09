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

#[derive(Clone, Debug, PartialEq)]
enum Thunk {
    Atomic(Variable),
    Continuation(Box<Syntax<Thunk>>),
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
                Syntax::Thunk(Thunk::Continuation(Box::new(subformula)))
            }),
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
            inner.clone().prop_map(|subformula| {
                Syntax::Always(Box::new(subformula), None)
            }),
            inner.clone().prop_map(|subformula| {
                Syntax::Eventually(Box::new(subformula), None)
            }),
        ]
    })
    .boxed()
}

fn assert_values_eq_up_to_violations<Function: Clone + std::fmt::Debug>(
    value_left: Value<Function>,
    value_right: Value<Function>,
    time: Time,
) {
    match (value_left, value_right) {
        (Value::True, Value::True) => {}
        (Value::False(_), Value::False(_)) => {}
        (Value::Residual(left), Value::Residual(right)) => {
            match (stop_default(&left, time), stop_default(&right, time)) {
                (None, None) => {}
                (Some(StopDefault::True), Some(StopDefault::True)) => {}
                (Some(StopDefault::False(_)), Some(StopDefault::False(_))) => {}
                (left, right) => panic!("{:?} != {:?}", left, right),
            }
        }
        (left, right) => panic!("{:?} != {:?}", left, right),
    }
}

fn check_equivalence(
    formula_left: Formula<Thunk>,
    formula_right: Formula<Thunk>,
    trace: Vec<State>,
    check_violations: bool,
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
        Thunk::Continuation(syntax) => {
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

    if check_violations {
        assert_eq!(value_left, value_right);
    } else {
        assert_values_eq_up_to_violations(value_left, value_right, time);
    }
}

proptest! {
    /// X(¬φ) ⇔ ¬X(φ)
    #[test]
    fn test_next_self_duality(syntax in syntax(), trace in trace()) {
        let formula_left =
            Syntax::Next(Box::new(Syntax::Not(Box::new(syntax.clone())))).nnf();
        let formula_right =
            Syntax::Not(Box::new(Syntax::Next(Box::new(syntax.clone())))).nnf();
        check_equivalence(formula_left, formula_right, trace, true);
    }

    /// A(¬φ) ⇔ ¬F(φ)
    #[test]
    fn test_always_eventually_duality(syntax in syntax(), trace in trace()) {
        let formula_left =
            Syntax::Always(Box::new(Syntax::Not(Box::new(syntax.clone()))), None).nnf();
        let formula_right =
            Syntax::Not(Box::new(Syntax::Eventually(Box::new(syntax.clone()), None))).nnf();
        check_equivalence(formula_left, formula_right, trace, true);
    }

    /// F(¬φ) ⇔ F(F(φ))
    #[test]
    fn test_eventually_idempotency(syntax in syntax(), trace in trace()) {
        let formula_left =
            Syntax::Eventually(Box::new(syntax.clone()), None).nnf();
        let formula_right =
            Syntax::Eventually(Box::new(Syntax::Eventually(Box::new(syntax.clone()), None)), None).nnf();
        check_equivalence(formula_left, formula_right, trace, false);
    }

    /// G(¬φ) ⇔ G(G(φ))
    #[test]
    fn test_always_idempotency(syntax in syntax(), trace in trace()) {
        let formula_left =
            Syntax::Always(Box::new(syntax.clone()), None).nnf();
        let formula_right =
            Syntax::Always(Box::new(Syntax::Always(Box::new(syntax.clone()), None)), None).nnf();
        check_equivalence(formula_left, formula_right, trace, false);
    }
}
