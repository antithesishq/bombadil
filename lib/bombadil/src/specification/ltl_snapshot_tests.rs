use std::{
    collections::BTreeSet,
    time::{Duration, UNIX_EPOCH},
};

use proptest::prelude::*;

use crate::specification::{
    ltl::*,
    stop::{StopDefault, stop_default},
    syntax::Syntax,
    verifier::Snapshot,
};

fn snapshot(index: usize, name: &str, value: serde_json::Value) -> Snapshot {
    Snapshot {
        index,
        name: Some(name.to_string()),
        value,
    }
}

fn snapshot_names(value: &Value<Variable>) -> Vec<String> {
    match value {
        Value::True(snapshots) => {
            snapshots.iter().filter_map(|s| s.name.clone()).collect()
        }
        Value::False(violation, _) => violation_snapshot_names(violation),
        Value::Residual(_) => vec![],
    }
}

fn violation_snapshot_names(violation: &Violation<Variable>) -> Vec<String> {
    match violation {
        Violation::False { snapshots, .. } => {
            snapshots.iter().filter_map(|s| s.name.clone()).collect()
        }
        Violation::Implies {
            antecedent_snapshots,
            ..
        } => antecedent_snapshots
            .iter()
            .filter_map(|s| s.name.clone())
            .collect(),
        _ => vec![],
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
enum Variable {
    X,
    Y,
    Z,
}

fn make_snapshots() -> Vec<Snapshot> {
    vec![
        snapshot(0, "x_val", serde_json::json!(1)),
        snapshot(1, "y_val", serde_json::json!(2)),
        snapshot(2, "z_val", serde_json::json!(3)),
    ]
}

fn thunk(variable: Variable) -> Formula<Variable> {
    Formula::Thunk {
        function: variable,
        negated: false,
    }
}

struct TestState {
    x: bool,
    y: bool,
    z: bool,
}

fn variable_snapshot(variable: &Variable) -> Snapshot {
    let all = make_snapshots();
    match variable {
        Variable::X => all[0].clone(),
        Variable::Y => all[1].clone(),
        Variable::Z => all[2].clone(),
    }
}

fn evaluate_with_state(
    formula: &Formula<Variable>,
    state: &TestState,
) -> Value<Variable> {
    let mut evaluate_thunk = |variable: &Variable, negated: bool| {
        let value = match variable {
            Variable::X => state.x,
            Variable::Y => state.y,
            Variable::Z => state.z,
        };
        let value = if negated { !value } else { value };
        Ok((
            Formula::Pure {
                value,
                pretty: format!("{:?}={}", variable, value),
            },
            vec![variable_snapshot(variable)],
        ))
    };
    let mut evaluator = Evaluator::new(&mut evaluate_thunk);
    evaluator.evaluate(formula, UNIX_EPOCH).unwrap()
}

fn step_with_state(
    residual: &Residual<Variable>,
    state: &TestState,
    time: Time,
) -> Value<Variable> {
    let mut evaluate_thunk = |variable: &Variable, negated: bool| {
        let value = match variable {
            Variable::X => state.x,
            Variable::Y => state.y,
            Variable::Z => state.z,
        };
        let value = if negated { !value } else { value };
        Ok((
            Formula::Pure {
                value,
                pretty: format!("{:?}={}", variable, value),
            },
            vec![variable_snapshot(variable)],
        ))
    };
    let mut evaluator = Evaluator::new(&mut evaluate_thunk);
    evaluator.step(residual, time).unwrap()
}

#[test]
fn test_and_merges_snapshots_when_both_true() {
    let state = TestState {
        x: true,
        y: true,
        z: false,
    };
    let formula = Formula::And(
        Box::new(thunk(Variable::X)),
        Box::new(thunk(Variable::Y)),
    );
    let value = evaluate_with_state(&formula, &state);
    assert!(matches!(value, Value::True(_)));
    let names = snapshot_names(&value);
    assert!(names.contains(&"x_val".to_string()));
    assert!(names.contains(&"y_val".to_string()));
}

#[test]
fn test_and_preserves_left_snapshots_with_residual() {
    // x AND next(y): x is true (has snapshots), next(y) is residual
    let state = TestState {
        x: true,
        y: true,
        z: false,
    };
    let formula = Formula::And(
        Box::new(thunk(Variable::X)),
        Box::new(Formula::Next(Box::new(thunk(Variable::Y)))),
    );
    let value = evaluate_with_state(&formula, &state);
    assert!(matches!(value, Value::Residual(_)));

    // Step the residual with y=true
    if let Value::Residual(residual) = &value {
        let time = UNIX_EPOCH.checked_add(Duration::from_millis(1)).unwrap();
        let stepped = step_with_state(residual, &state, time);
        assert!(matches!(stepped, Value::True(_)));
        let names = snapshot_names(&stepped);
        assert!(
            names.contains(&"x_val".to_string()),
            "left snapshots lost: {:?}",
            names
        );
        assert!(
            names.contains(&"y_val".to_string()),
            "right snapshots lost: {:?}",
            names
        );
    }
}

#[test]
fn test_and_preserves_right_snapshots_with_residual() {
    // next(x) AND y: y is true (has snapshots), next(x) is residual
    let state = TestState {
        x: true,
        y: true,
        z: false,
    };
    let formula = Formula::And(
        Box::new(Formula::Next(Box::new(thunk(Variable::X)))),
        Box::new(thunk(Variable::Y)),
    );
    let value = evaluate_with_state(&formula, &state);
    assert!(matches!(value, Value::Residual(_)));

    if let Value::Residual(residual) = &value {
        let time = UNIX_EPOCH.checked_add(Duration::from_millis(1)).unwrap();
        let stepped = step_with_state(residual, &state, time);
        assert!(matches!(stepped, Value::True(_)));
        let names = snapshot_names(&stepped);
        assert!(
            names.contains(&"x_val".to_string()),
            "left snapshots lost: {:?}",
            names
        );
        assert!(
            names.contains(&"y_val".to_string()),
            "right snapshots lost: {:?}",
            names
        );
    }
}

#[test]
fn test_implies_after_and_has_all_antecedent_snapshots() {
    // (x AND y) IMPLIES z, where x=true, y=true, z=false
    let state = TestState {
        x: true,
        y: true,
        z: false,
    };
    let antecedent = Formula::And(
        Box::new(thunk(Variable::X)),
        Box::new(thunk(Variable::Y)),
    );
    let formula =
        Formula::Implies(Box::new(antecedent), Box::new(thunk(Variable::Z)));
    let value = evaluate_with_state(&formula, &state);
    assert!(matches!(value, Value::False(_, _)));
    if let Value::False(violation, _) = &value {
        let names = violation_snapshot_names(violation);
        assert!(
            names.contains(&"x_val".to_string()),
            "x snapshots missing from antecedent: {:?}",
            names
        );
        assert!(
            names.contains(&"y_val".to_string()),
            "y snapshots missing from antecedent: {:?}",
            names
        );
    }
}

#[test]
fn test_always_implies_and_has_all_antecedent_snapshots() {
    // always(x.and(y).implies(z)), z becomes false at step 2
    let antecedent = Formula::And(
        Box::new(thunk(Variable::X)),
        Box::new(thunk(Variable::Y)),
    );
    let inner =
        Formula::Implies(Box::new(antecedent), Box::new(thunk(Variable::Z)));
    let formula = Formula::Always(Box::new(inner), None);

    // Step 1: x=true, y=true, z=true (no violation)
    let state1 = TestState {
        x: true,
        y: true,
        z: true,
    };
    let value = evaluate_with_state(&formula, &state1);
    assert!(matches!(value, Value::Residual(_)));

    // Step 2: x=true, y=true, z=false (violation)
    if let Value::Residual(residual) = &value {
        let state2 = TestState {
            x: true,
            y: true,
            z: false,
        };
        let time = UNIX_EPOCH.checked_add(Duration::from_millis(1)).unwrap();
        let stepped = step_with_state(residual, &state2, time);
        assert!(matches!(stepped, Value::False(_, _)));
        if let Value::False(Violation::Always { violation, .. }, _) = &stepped {
            let names = violation_snapshot_names(violation);
            assert!(
                names.contains(&"x_val".to_string()),
                "x snapshots missing: {:?}",
                names
            );
            assert!(
                names.contains(&"y_val".to_string()),
                "y snapshots missing: {:?}",
                names
            );
        } else {
            panic!("expected Always violation, got: {:?}", stepped);
        }
    }
}

#[test]
fn test_or_merges_snapshots_when_both_true() {
    let state = TestState {
        x: true,
        y: true,
        z: false,
    };
    let formula =
        Formula::Or(Box::new(thunk(Variable::X)), Box::new(thunk(Variable::Y)));
    let value = evaluate_with_state(&formula, &state);
    assert!(matches!(value, Value::True(_)));
    let names = snapshot_names(&value);
    assert!(names.contains(&"x_val".to_string()));
    assert!(names.contains(&"y_val".to_string()));
}

#[test]
fn test_or_true_short_circuits_with_snapshots() {
    // x OR next(y): x is true, OR short-circuits to True with x's snapshots
    let state = TestState {
        x: true,
        y: true,
        z: false,
    };
    let formula = Formula::Or(
        Box::new(thunk(Variable::X)),
        Box::new(Formula::Next(Box::new(thunk(Variable::Y)))),
    );
    let value = evaluate_with_state(&formula, &state);
    assert!(matches!(value, Value::True(_)));
    let names = snapshot_names(&value);
    assert!(
        names.contains(&"x_val".to_string()),
        "x snapshots lost: {:?}",
        names
    );
}

#[test]
fn test_implies_after_or_has_all_antecedent_snapshots() {
    // (x OR y) IMPLIES z, where x=true, y=true, z=false
    let state = TestState {
        x: true,
        y: true,
        z: false,
    };
    let antecedent =
        Formula::Or(Box::new(thunk(Variable::X)), Box::new(thunk(Variable::Y)));
    let formula =
        Formula::Implies(Box::new(antecedent), Box::new(thunk(Variable::Z)));
    let value = evaluate_with_state(&formula, &state);
    assert!(matches!(value, Value::False(_, _)));
    if let Value::False(violation, _) = &value {
        let names = violation_snapshot_names(violation);
        assert!(
            names.contains(&"x_val".to_string()),
            "x snapshots missing from antecedent: {:?}",
            names
        );
        assert!(
            names.contains(&"y_val".to_string()),
            "y snapshots missing from antecedent: {:?}",
            names
        );
    }
}

#[test]
fn test_stop_implies_preserves_antecedent_snapshots() {
    let snapshots = vec![
        snapshot(0, "a", serde_json::json!(1)),
        snapshot(1, "b", serde_json::json!(2)),
    ];
    let left_formula: Formula<Variable> = Formula::Pure {
        value: true,
        pretty: "true".to_string(),
    };
    let residual = Residual::Implies {
        left_formula: left_formula.clone(),
        left: Box::new(Residual::True(snapshots.clone())),
        right: Box::new(Residual::False(Violation::False {
            time: UNIX_EPOCH,
            condition: "z".to_string(),
            snapshots: vec![],
        })),
    };
    let time = UNIX_EPOCH;
    let result = stop_default(&residual, time);
    match result {
        Some(StopDefault::False(Violation::Implies {
            antecedent_snapshots,
            ..
        })) => {
            let names: Vec<String> = antecedent_snapshots
                .iter()
                .filter_map(|s| s.name.clone())
                .collect();
            assert!(
                names.contains(&"a".to_string()),
                "snapshot 'a' missing: {:?}",
                names
            );
            assert!(
                names.contains(&"b".to_string()),
                "snapshot 'b' missing: {:?}",
                names
            );
        }
        other => {
            panic!("expected StopDefault::False(Implies), got: {:?}", other)
        }
    }
}

// Property: for non-temporal formulas, the snapshots in a True result exactly equal the
// "truth-contributing" thunks — those whose true evaluation was necessary for the formula to be
// true. This is computed by an independent oracle that doesn't share any code with the evaluator.

fn variable_index(variable: &Variable) -> usize {
    match variable {
        Variable::X => 0,
        Variable::Y => 1,
        Variable::Z => 2,
    }
}

fn prop_variable() -> BoxedStrategy<Variable> {
    prop_oneof![Just(Variable::X), Just(Variable::Y)].boxed()
}

fn nontemporal_syntax() -> BoxedStrategy<Syntax<Variable>> {
    let leaf = prop_oneof![
        any::<bool>().prop_map(|value| Syntax::Pure {
            value,
            pretty: format!("{}", value)
        }),
        prop_variable().prop_map(Syntax::Thunk),
    ]
    .boxed();

    leaf.prop_recursive(8, 256, 10, |inner| {
        prop_oneof![
            inner.clone().prop_map(|s| Syntax::Not(Box::new(s))),
            (inner.clone(), inner.clone())
                .prop_map(|(l, r)| Syntax::And(Box::new(l), Box::new(r))),
            (inner.clone(), inner.clone())
                .prop_map(|(l, r)| Syntax::Or(Box::new(l), Box::new(r))),
            (inner.clone(), inner.clone())
                .prop_map(|(l, r)| Syntax::Implies(Box::new(l), Box::new(r))),
        ]
    })
    .boxed()
}

/// Recursively compute which thunk indices contributed to a formula being true. Returns
/// `Some(indices)` when the formula is true, `None` when false.
fn truth_contributing(
    formula: &Formula<Variable>,
    state_x: bool,
    state_y: bool,
) -> Option<BTreeSet<usize>> {
    match formula {
        Formula::Pure { value, .. } => {
            if *value {
                Some(BTreeSet::new())
            } else {
                None
            }
        }
        Formula::Thunk { function, negated } => {
            let raw = match function {
                Variable::X => state_x,
                Variable::Y => state_y,
                Variable::Z => unreachable!(),
            };
            let value = if *negated { !raw } else { raw };
            if value {
                Some(BTreeSet::from([variable_index(function)]))
            } else {
                None
            }
        }
        Formula::And(left, right) => {
            let l = truth_contributing(left, state_x, state_y);
            let r = truth_contributing(right, state_x, state_y);
            match (l, r) {
                (Some(mut a), Some(b)) => {
                    a.extend(b);
                    Some(a)
                }
                _ => None,
            }
        }
        Formula::Or(left, right) => {
            let l = truth_contributing(left, state_x, state_y);
            let r = truth_contributing(right, state_x, state_y);
            match (l, r) {
                (Some(mut a), Some(b)) => {
                    a.extend(b);
                    Some(a)
                }
                (some @ Some(_), None) | (None, some @ Some(_)) => some,
                (None, None) => None,
            }
        }
        Formula::Implies(left, right) => {
            let l = truth_contributing(left, state_x, state_y);
            let r = truth_contributing(right, state_x, state_y);
            match (l, r) {
                (None, _) => Some(BTreeSet::new()),
                (Some(mut a), Some(b)) => {
                    a.extend(b);
                    Some(a)
                }
                (Some(_), None) => None,
            }
        }
        _ => unreachable!("non-temporal formulas only"),
    }
}

fn actual_snapshot_indices(value: &Value<Variable>) -> BTreeSet<usize> {
    match value {
        Value::True(snapshots) => snapshots
            .iter()
            .filter_map(|s| s.name.as_ref())
            .map(|name| match name.as_str() {
                "x_val" => 0,
                "y_val" => 1,
                _ => panic!("unexpected snapshot: {}", name),
            })
            .collect(),
        _ => BTreeSet::new(),
    }
}

proptest! {
    #[test]
    fn test_true_snapshots_equal_truth_contributing(
        syntax in nontemporal_syntax(),
        state_x in any::<bool>(),
        state_y in any::<bool>(),
    ) {
        let formula = syntax.nnf();
        let expected = truth_contributing(&formula, state_x, state_y);

        let all_snapshots = [snapshot(0, "x_val", serde_json::json!(1)),
            snapshot(1, "y_val", serde_json::json!(2))];
        let mut evaluate_thunk = |variable: &Variable, negated: bool| {
            let raw = match variable {
                Variable::X => state_x,
                Variable::Y => state_y,
                Variable::Z => unreachable!(),
            };
            let value = if negated { !raw } else { raw };
            let snapshot =
                all_snapshots[variable_index(variable)].clone();
            Ok((
                Formula::Pure {
                    value,
                    pretty: format!("{:?}={}", variable, value),
                },
                vec![snapshot],
            ))
        };
        let mut evaluator = Evaluator::new(&mut evaluate_thunk);
        let value = evaluator.evaluate(&formula, UNIX_EPOCH).unwrap();

        match (&expected, &value) {
            (Some(expected_indices), Value::True(_)) => {
                let actual = actual_snapshot_indices(&value);
                prop_assert_eq!(
                    expected_indices, &actual,
                    "formula: {:?}, x={}, y={}",
                    syntax, state_x, state_y,
                );
            }
            (None, Value::False(_, _)) => {}
            (Some(_), Value::False(_, _)) => {
                prop_assert!(
                    false,
                    "oracle=true, evaluator=false: {:?}, x={}, y={}",
                    syntax, state_x, state_y,
                );
            }
            (None, Value::True(_)) => {
                prop_assert!(
                    false,
                    "oracle=false, evaluator=true: {:?}, x={}, y={}",
                    syntax, state_x, state_y,
                );
            }
            (_, Value::Residual(_)) => {
                prop_assert!(
                    false,
                    "non-temporal formula produced Residual",
                );
            }
        }
    }
}
