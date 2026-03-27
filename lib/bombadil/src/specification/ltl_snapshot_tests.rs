use std::time::{Duration, UNIX_EPOCH};

use bit_set::BitSet;

use crate::specification::{
    ltl::*,
    stop::{StopDefault, stop_default},
    verifier::Snapshot,
};

fn snapshot(name: &str, value: serde_json::Value) -> Snapshot {
    Snapshot {
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
        snapshot("x_val", serde_json::json!(1)),
        snapshot("y_val", serde_json::json!(2)),
        snapshot("z_val", serde_json::json!(3)),
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

fn evaluate_with_state(
    formula: &Formula<Variable>,
    state: &TestState,
) -> Value<Variable> {
    let snapshots = make_snapshots();
    let mut evaluate_thunk = |variable: &Variable, negated: bool| {
        let value = match variable {
            Variable::X => state.x,
            Variable::Y => state.y,
            Variable::Z => state.z,
        };
        let value = if negated { !value } else { value };
        let mut accessed = BitSet::new();
        match variable {
            Variable::X => accessed.insert(0),
            Variable::Y => accessed.insert(1),
            Variable::Z => accessed.insert(2),
        };
        Ok((
            Formula::Pure {
                value,
                pretty: format!("{:?}={}", variable, value),
            },
            accessed,
        ))
    };
    let mut evaluator = Evaluator::new(&mut evaluate_thunk, &snapshots);
    evaluator.evaluate(formula, UNIX_EPOCH).unwrap()
}

fn step_with_state(
    residual: &Residual<Variable>,
    state: &TestState,
    time: Time,
) -> Value<Variable> {
    let snapshots = make_snapshots();
    let mut evaluate_thunk = |variable: &Variable, negated: bool| {
        let value = match variable {
            Variable::X => state.x,
            Variable::Y => state.y,
            Variable::Z => state.z,
        };
        let value = if negated { !value } else { value };
        let mut accessed = BitSet::new();
        match variable {
            Variable::X => accessed.insert(0),
            Variable::Y => accessed.insert(1),
            Variable::Z => accessed.insert(2),
        };
        Ok((
            Formula::Pure {
                value,
                pretty: format!("{:?}={}", variable, value),
            },
            accessed,
        ))
    };
    let mut evaluator = Evaluator::new(&mut evaluate_thunk, &snapshots);
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
        snapshot("a", serde_json::json!(1)),
        snapshot("b", serde_json::json!(2)),
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
