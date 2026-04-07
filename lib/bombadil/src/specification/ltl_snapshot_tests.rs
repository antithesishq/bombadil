use std::{collections::BTreeSet, time::Duration};

use proptest::prelude::*;

use crate::specification::{
    ltl::*,
    stop::{StopDefault, stop_default},
    syntax::Syntax,
    verifier::Snapshot,
};

fn t0() -> Time {
    Time::from_system_time(std::time::SystemTime::UNIX_EPOCH)
}

fn time_from_millis(millis: u64) -> Time {
    Time::from_system_time(
        std::time::SystemTime::UNIX_EPOCH + Duration::from_millis(millis),
    )
}

fn time_from_secs(secs: u64) -> Time {
    Time::from_system_time(
        std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(secs),
    )
}

fn snapshot(index: usize, name: &str, value: serde_json::Value) -> Snapshot {
    Snapshot {
        index,
        name: Some(name.to_string()),
        value,
        time: t0(),
    }
}

fn snapshot_names(value: &Value<Variable>) -> Vec<String> {
    match value {
        Value::True(snapshots) => snapshots
            .iter()
            .filter_map(|(_, s)| s.name.clone())
            .collect(),
        Value::False(violation, _) => violation_snapshot_names(violation),
        Value::Residual(_) => vec![],
    }
}

fn violation_snapshot_names(violation: &Violation<Variable>) -> Vec<String> {
    match violation {
        Violation::False { snapshots, .. } => snapshots
            .iter()
            .filter_map(|snapshot| snapshot.name.clone())
            .collect(),
        Violation::Implies {
            antecedent_snapshots,
            right,
            ..
        } => {
            let mut names: Vec<String> = antecedent_snapshots
                .iter()
                .filter_map(|snapshot| snapshot.name.clone())
                .collect();
            names.extend(violation_snapshot_names(right));
            names
        }
        _ => vec![],
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
enum Variable {
    X,
    Y,
    Z,
}

fn make_snapshots() -> UniqueSnapshots {
    UniqueSnapshots::from([
        ((0, t0()), snapshot(0, "x_val", serde_json::json!(1))),
        ((1, t0()), snapshot(1, "y_val", serde_json::json!(2))),
        ((2, t0()), snapshot(2, "z_val", serde_json::json!(3))),
    ])
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

fn variable_snapshot(variable: &Variable) -> ((usize, Time), Snapshot) {
    let all = make_snapshots();
    let index = variable_index(variable);
    let time = t0();
    ((index, time), all[&(index, time)].clone())
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
            UniqueSnapshots::from([variable_snapshot(variable)]),
        ))
    };
    let mut evaluator = Evaluator::new(&mut evaluate_thunk);
    evaluator.evaluate(formula, t0()).unwrap()
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
            UniqueSnapshots::from([variable_snapshot(variable)]),
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
        let time = time_from_millis(1);
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
        let time = time_from_millis(1);
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
        let time = time_from_millis(1);
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
    let snapshots = UniqueSnapshots::from([
        ((0, t0()), snapshot(0, "a", serde_json::json!(1))),
        ((1, t0()), snapshot(1, "b", serde_json::json!(2))),
    ]);
    let left_formula: Formula<Variable> = Formula::Pure {
        value: true,
        pretty: "true".to_string(),
    };
    let residual = Residual::Implies {
        left_formula: left_formula.clone(),
        left: Box::new(Residual::True(snapshots.clone())),
        right: Box::new(Residual::False(Violation::False {
            time: t0(),
            condition: "z".to_string(),
            snapshots: vec![],
        })),
    };
    let time = t0();
    let result = stop_default(&residual, time);
    match result {
        Some(StopDefault::False(Violation::Implies {
            antecedent_snapshots,
            ..
        })) => {
            let names: Vec<String> = antecedent_snapshots
                .iter()
                .filter_map(|snapshot| snapshot.name.clone())
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
            inner
                .clone()
                .prop_map(|snapshot| Syntax::Not(Box::new(snapshot))),
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
            .filter_map(|(_, s)| s.name.as_ref())
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
            let index = variable_index(variable);
            let snapshot = all_snapshots[index].clone();
            Ok((
                Formula::Pure {
                    value,
                    pretty: format!("{:?}={}", variable, value),
                },
                UniqueSnapshots::from([((index, snapshot.time), snapshot)]),
            ))
        };
        let mut evaluator = Evaluator::new(&mut evaluate_thunk);
        let value = evaluator.evaluate(&formula, t0()).unwrap();

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

#[test]
fn test_thunk_returning_implies_preserves_outer_snapshots() {
    // Simulates: now(() => { const x = X.current; return now(() => true).implies(Y) })
    // The outer thunk accesses X, returns an implication that should include X in antecedent_snapshots
    let state = TestState {
        x: true,
        y: false,
        z: false,
    };

    let mut evaluate_thunk = |variable: &Variable, negated: bool| {
        let value = match variable {
            Variable::X => state.x,
            Variable::Y => state.y,
            Variable::Z => state.z,
        };
        let value = if negated { !value } else { value };

        match variable {
            Variable::X => {
                // Outer thunk: accesses X, returns an implication
                Ok((
                    Formula::Implies(
                        Box::new(Formula::Pure {
                            value: true,
                            pretty: "true".to_string(),
                        }),
                        Box::new(thunk(Variable::Y)),
                    ),
                    UniqueSnapshots::from([variable_snapshot(variable)]),
                ))
            }
            _ => {
                // Inner thunk
                Ok((
                    Formula::Pure {
                        value,
                        pretty: format!("{:?}={}", variable, value),
                    },
                    UniqueSnapshots::from([variable_snapshot(variable)]),
                ))
            }
        }
    };

    let mut evaluator = Evaluator::new(&mut evaluate_thunk);
    let value = evaluator.evaluate(&thunk(Variable::X), t0()).unwrap();

    assert!(matches!(value, Value::False(_, _)));
    if let Value::False(violation, _) = &value {
        let names = violation_snapshot_names(violation);
        assert!(
            names.contains(&"x_val".to_string()),
            "x snapshot from outer thunk missing from antecedent: {:?}",
            names
        );
        assert!(
            names.contains(&"y_val".to_string()),
            "y snapshot from consequent missing: {:?}",
            names
        );
    }
}

#[test]
fn test_always_with_outer_thunk_preserves_snapshots() {
    // Simulates: always(() => { const x = X.current; return Y.implies(Z) })
    // At T0: X=true, Y=true, Z=true -> residual
    // At T1: X=true, Y=true, Z=false -> violation
    // Should capture X from T1's outer thunk in antecedent snapshots

    let state_t0 = TestState {
        x: true,
        y: true,
        z: true,
    };
    let state_t1 = TestState {
        x: true,
        y: true,
        z: false,
    };

    let current_state = std::cell::RefCell::new(&state_t0);
    let time_t0 = t0();
    let time_t1 = time_from_secs(1);

    let mut evaluate_thunk = |variable: &Variable, negated: bool| {
        let state = current_state.borrow();
        let value = match variable {
            Variable::X => state.x,
            Variable::Y => state.y,
            Variable::Z => state.z,
        };
        let value = if negated { !value } else { value };

        match variable {
            Variable::X => {
                // Outer thunk: accesses X, returns Y.implies(Z)
                let time = if state.z { time_t0 } else { time_t1 };
                Ok((
                    Formula::Implies(
                        Box::new(thunk(Variable::Y)),
                        Box::new(thunk(Variable::Z)),
                    ),
                    UniqueSnapshots::from([(
                        (0, time),
                        snapshot(0, "x_val", serde_json::json!(value)),
                    )]),
                ))
            }
            _ => {
                // Inner thunks
                let time = if current_state.borrow().z {
                    time_t0
                } else {
                    time_t1
                };
                let index = variable_index(variable);
                let name = match variable {
                    Variable::Y => "y_val",
                    Variable::Z => "z_val",
                    _ => unreachable!(),
                };
                Ok((
                    Formula::Pure {
                        value,
                        pretty: format!("{:?}={}", variable, value),
                    },
                    UniqueSnapshots::from([(
                        (index, time),
                        snapshot(index, name, serde_json::json!(value)),
                    )]),
                ))
            }
        }
    };

    let mut evaluator = Evaluator::new(&mut evaluate_thunk);

    // T0: should be residual
    let value = evaluator
        .evaluate(
            &Formula::Always(Box::new(thunk(Variable::X)), None),
            time_t0,
        )
        .unwrap();
    assert!(matches!(value, Value::Residual(_)));

    // T1: should be false
    *current_state.borrow_mut() = &state_t1;
    let residual = match value {
        Value::Residual(r) => r,
        _ => unreachable!(),
    };
    let value = evaluator.step(&residual, time_t1).unwrap();

    assert!(matches!(value, Value::False(_, _)));
    if let Value::False(Violation::Always { violation, .. }, _) = &value {
        if let Violation::Implies {
            antecedent_snapshots,
            right,
            ..
        } = violation.as_ref()
        {
            let snapshot_names: Vec<_> = antecedent_snapshots
                .iter()
                .filter_map(|s| s.name.as_ref())
                .collect();

            assert!(
                snapshot_names.contains(&&"x_val".to_string()),
                "x snapshot from outer thunk missing from antecedent: {:?}",
                snapshot_names
            );
            assert!(
                snapshot_names.contains(&&"y_val".to_string()),
                "y snapshot missing from antecedent: {:?}",
                snapshot_names
            );

            // Also check the consequent has Z
            if let Violation::False { snapshots, .. } = right.as_ref() {
                let consequent_names: Vec<_> =
                    snapshots.iter().filter_map(|s| s.name.as_ref()).collect();
                assert!(
                    consequent_names.contains(&&"z_val".to_string()),
                    "z snapshot missing from consequent: {:?}",
                    consequent_names
                );
            }
        } else {
            panic!("Expected Implies violation, got: {:?}", violation);
        }
    } else {
        panic!("Expected Always(Implies(...)) violation, got: {:?}", value);
    }
}
