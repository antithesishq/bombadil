use std::time::Duration;

use hegel::TestCase;

use crate::{
    eval::*,
    formula::*,
    test_domain::{
        TestDomain, TestState, TestTime, Variable, evaluate_with_state,
        formula_depth, has_nested_unbounded_always, residual_depth, state,
        step_with_state, syntax, violation_depth,
    },
};

#[test]
fn test_eventually_eventually_violation_doesnt_grow() {
    let state = TestState {
        x: true,
        y: true,
        z: true,
    };
    let formula: Formula<TestDomain> = Formula::Eventually(
        Box::new(Formula::Eventually(
            Box::new(Formula::Implies(
                Box::new(Formula::Pure {
                    value: true,
                    pretty: "true".to_string(),
                }),
                Box::new(Formula::Pure {
                    value: false,
                    pretty: "false".to_string(),
                }),
            )),
            Some(Duration::from_millis(10)),
        )),
        None,
    );
    let mut value = evaluate_with_state(&formula, &state, |_| ());
    for i in 1..=2000u64 {
        let residual = match value {
            Value::Residual(residual) => residual,
            other => {
                panic!("expected residual at step {}, got {:?}", i, other)
            }
        };
        value = step_with_state(&residual, &state, TestTime(i), |_| ());
        if let Value::False(violation, residual) = &value {
            let depth = violation_depth(violation);
            assert!(
                depth <= 3,
                "violation depth {depth} at step {i} does not match formula depth:\n\nviolation: {violation:?}\n\nresidual: {residual:?}\n",
            );
            if let Some(residual) = residual {
                value = Value::Residual(residual.clone())
            }
        }
    }
}

#[hegel::test(test_cases = 1000)]
fn test_violation_doesnt_grow_larger_than_formula(tc: TestCase) {
    let formula = tc.draw(syntax()).nnf();
    let state = tc.draw(state());
    // We currently do not support nested unbounded always in the evaluator, even if it's
    // allowed syntactically.
    tc.assume(!has_nested_unbounded_always(&formula));
    let depth_formula = formula_depth(&formula);

    let mut value = evaluate_with_state(&formula, &state, |_| ());
    for i in 1..=2000u64 {
        let residual = match value {
            Value::Residual(residual) => residual,
            _ => break,
        };
        value = step_with_state(&residual, &state, TestTime(i), |_| ());
        if let Value::False(violation, residual) = &value {
            let depth_violation = violation_depth(violation);
            assert!(
                depth_violation <= depth_formula,
                "violation depth {depth_violation} at step {i} does not match formula depth {depth_formula}:\n\nviolation: {violation:?}\n\nresidual: {residual:?}\n",
            );
            if let Some(residual) = residual {
                assert!(residual_depth(residual) <= depth_formula);
                value = Value::Residual(residual.clone())
            }
        }
    }
}

#[test]
fn test_always_implies_eventually_violation_doesnt_grow() {
    let formula: Formula<TestDomain> = Formula::Always(
        Box::new(Formula::Implies(
            Box::new(Formula::Thunk {
                function: Variable::Y,
                negated: false,
            }),
            Box::new(Formula::Eventually(
                Box::new(Formula::Thunk {
                    function: Variable::X,
                    negated: false,
                }),
                None,
            )),
        )),
        None,
    );
    let mut value = evaluate_with_state(
        &formula,
        &TestState {
            x: true,
            y: false,
            z: true,
        },
        |_| (),
    );
    for i in 1..=1000u64 {
        let residual = match value {
            Value::Residual(residual) => residual,
            other => {
                panic!("expected residual at step {}, got {:?}", i, other)
            }
        };
        assert!(residual_depth(&residual) < 20);

        value = step_with_state(
            &residual,
            &TestState {
                x: i < 10,
                y: true,
                z: true,
            },
            TestTime(i),
            |_| (),
        );
        if let Value::False(violation, residual) = &value {
            let depth = violation_depth(violation);
            assert!(
                depth <= 3,
                "violation depth {depth} at step {i} does not match formula depth:\n\nviolation: {violation:?}\n\nresidual: {residual:?}\n",
            );
            if let Some(residual) = residual {
                value = Value::Residual(residual.clone())
            }
        }
    }
}

#[test]
fn test_always_next_residual_stays_bounded() {
    let eval_state = TestState {
        x: true,
        y: true,
        z: true,
    };
    let formula: Formula<TestDomain> = Formula::Always(
        Box::new(Formula::Next(Box::new(Formula::Pure {
            value: true,
            pretty: "true".to_string(),
        }))),
        None,
    );
    let mut value = evaluate_with_state(&formula, &eval_state, |_| ());
    for i in 1..=2000u64 {
        let residual = match value {
            Value::Residual(residual) => residual,
            other => {
                panic!("expected residual at step {}, got {:?}", i, other)
            }
        };
        let depth = residual_depth(&residual);
        assert!(depth <= 4, "residual depth grew to {} at step {}", depth, i,);
        value = step_with_state(&residual, &eval_state, TestTime(i), |_| ());
    }
}
