use crate::test_domain::step_with_state;
use std::collections::{BTreeMap, BTreeSet};

use anyhow::Error;
use hegel::{
    Generator, TestCase,
    generators::{booleans, deferred, just, one_of},
    tuples,
};

use crate::{
    eval::*,
    formula::*,
    stop::{StopDefault, stop_default},
    syntax::Syntax,
    test_domain::{
        TestDomain, TestState, TestTime, Variable, evaluate_with_state,
    },
    violation::*,
};

/// A named snapshot entry for testing.
#[derive(Clone, Debug, PartialEq)]
struct TestSnapshot {
    index: usize,
    name: String,
}

/// State that tracks named snapshots, keyed by index.
#[derive(Clone, Debug, PartialEq, Default)]
struct TestSnapshots(BTreeMap<usize, TestSnapshot>);

impl State for TestSnapshots {
    fn merge(&self, other: &Self) -> Self {
        let mut merged = self.0.clone();
        merged.extend(other.0.iter().map(|(k, v)| (*k, v.clone())));
        TestSnapshots(merged)
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl TestSnapshots {
    fn from_snapshot(snapshot: TestSnapshot) -> Self {
        TestSnapshots(BTreeMap::from([(snapshot.index, snapshot)]))
    }

    fn names(&self) -> Vec<String> {
        self.0.values().map(|s| s.name.clone()).collect()
    }
}

type SnapshotDomain = TestDomain<TestSnapshots>;

fn snapshot(index: usize, name: &str) -> TestSnapshot {
    TestSnapshot {
        index,
        name: name.to_string(),
    }
}

fn state_names(value: &Value<SnapshotDomain>) -> Vec<String> {
    match value {
        Value::True(state) => state.names(),
        Value::False(violation, _) => violation_state_names(violation),
        Value::Residual(_) => vec![],
    }
}

fn violation_state_names(violation: &Violation<SnapshotDomain>) -> Vec<String> {
    match violation {
        Violation::False { state, .. } => state.names(),
        Violation::Implies { state, right, .. } => {
            let mut names = state.names();
            names.extend(violation_state_names(right));
            names
        }
        _ => vec![],
    }
}

fn make_snapshots() -> TestSnapshots {
    TestSnapshots(BTreeMap::from([
        (0, snapshot(0, "x_val")),
        (1, snapshot(1, "y_val")),
        (2, snapshot(2, "z_val")),
    ]))
}

fn thunk(variable: Variable) -> Formula<SnapshotDomain> {
    Formula::Thunk {
        function: variable,
        negated: false,
    }
}

fn variable_snapshot(variable: &Variable) -> TestSnapshots {
    let index = variable_index(variable);
    let all = make_snapshots();
    TestSnapshots(BTreeMap::from([(index, all.0[&index].clone())]))
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
    let value = evaluate_with_state(&formula, &state, variable_snapshot);
    assert!(matches!(value, Value::True(_)));
    let names = state_names(&value);
    assert!(names.contains(&"x_val".to_string()));
    assert!(names.contains(&"y_val".to_string()));
}

#[test]
fn test_and_preserves_left_snapshots_with_residual() {
    let state = TestState {
        x: true,
        y: true,
        z: false,
    };
    let formula = Formula::And(
        Box::new(thunk(Variable::X)),
        Box::new(Formula::Next(Box::new(thunk(Variable::Y)))),
    );
    let value = evaluate_with_state(&formula, &state, variable_snapshot);
    assert!(matches!(value, Value::Residual(_)));

    if let Value::Residual(residual) = &value {
        let time = TestTime::from_millis(1);
        let stepped =
            step_with_state(residual, &state, time, variable_snapshot);
        assert!(matches!(stepped, Value::True(_)));
        let names = state_names(&stepped);
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
    let state = TestState {
        x: true,
        y: true,
        z: false,
    };
    let formula = Formula::And(
        Box::new(Formula::Next(Box::new(thunk(Variable::X)))),
        Box::new(thunk(Variable::Y)),
    );
    let value = evaluate_with_state(&formula, &state, variable_snapshot);
    assert!(matches!(value, Value::Residual(_)));

    if let Value::Residual(residual) = &value {
        let time = TestTime::from_millis(1);
        let stepped =
            step_with_state(residual, &state, time, variable_snapshot);
        assert!(matches!(stepped, Value::True(_)));
        let names = state_names(&stepped);
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
    let value = evaluate_with_state(&formula, &state, variable_snapshot);
    assert!(matches!(value, Value::False(_, _)));
    if let Value::False(violation, _) = &value {
        let names = violation_state_names(violation);
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
    let antecedent = Formula::And(
        Box::new(thunk(Variable::X)),
        Box::new(thunk(Variable::Y)),
    );
    let inner =
        Formula::Implies(Box::new(antecedent), Box::new(thunk(Variable::Z)));
    let formula = Formula::Always(Box::new(inner), None);

    let state1 = TestState {
        x: true,
        y: true,
        z: true,
    };
    let value = evaluate_with_state(&formula, &state1, variable_snapshot);
    assert!(matches!(value, Value::Residual(_)));

    if let Value::Residual(residual) = &value {
        let state2 = TestState {
            x: true,
            y: true,
            z: false,
        };
        let time = TestTime::from_millis(1);
        let stepped =
            step_with_state(residual, &state2, time, variable_snapshot);
        assert!(matches!(stepped, Value::False(_, _)));
        if let Value::False(Violation::Always { violation, .. }, _) = &stepped {
            let names = violation_state_names(violation);
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
    let value = evaluate_with_state(&formula, &state, variable_snapshot);
    assert!(matches!(value, Value::True(_)));
    let names = state_names(&value);
    assert!(names.contains(&"x_val".to_string()));
    assert!(names.contains(&"y_val".to_string()));
}

#[test]
fn test_or_true_short_circuits_with_snapshots() {
    let state = TestState {
        x: true,
        y: true,
        z: false,
    };
    let formula = Formula::Or(
        Box::new(thunk(Variable::X)),
        Box::new(Formula::Next(Box::new(thunk(Variable::Y)))),
    );
    let value = evaluate_with_state(&formula, &state, variable_snapshot);
    assert!(matches!(value, Value::True(_)));
    let names = state_names(&value);
    assert!(
        names.contains(&"x_val".to_string()),
        "x snapshots lost: {:?}",
        names
    );
}

#[test]
fn test_implies_after_or_has_all_antecedent_snapshots() {
    let state = TestState {
        x: true,
        y: true,
        z: false,
    };
    let antecedent =
        Formula::Or(Box::new(thunk(Variable::X)), Box::new(thunk(Variable::Y)));
    let formula =
        Formula::Implies(Box::new(antecedent), Box::new(thunk(Variable::Z)));
    let value = evaluate_with_state(&formula, &state, variable_snapshot);
    assert!(matches!(value, Value::False(_, _)));
    if let Value::False(violation, _) = &value {
        let names = violation_state_names(violation);
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
    let state = TestSnapshots(BTreeMap::from([
        (0, snapshot(0, "a")),
        (1, snapshot(1, "b")),
    ]));
    let left_formula: Formula<SnapshotDomain> = Formula::Pure {
        value: true,
        pretty: "true".to_string(),
    };
    let residual: Residual<SnapshotDomain> = Residual::Implies {
        left_formula: left_formula.clone(),
        left: Box::new(Residual::True(state.clone())),
        right: Box::new(Residual::False(Violation::False {
            time: TestTime::ZERO,
            condition: "z".to_string(),
            state: TestSnapshots::default(),
        })),
    };
    let time = TestTime::ZERO;
    let result = stop_default(&residual, time);
    match result {
        Some(StopDefault::False(Violation::Implies {
            state: antecedent_state,
            ..
        })) => {
            let names = antecedent_state.names();
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

fn prop_variable() -> impl Generator<Variable> {
    one_of([just(Variable::X).boxed(), just(Variable::Y).boxed()]).boxed()
}

fn nontemporal_syntax() -> impl Generator<Syntax<SnapshotDomain>> {
    let syntax = deferred::<Syntax<SnapshotDomain>>();
    let leaf = one_of([
        booleans()
            .map(|value| Syntax::Pure {
                value,
                pretty: format!("{}", value),
            })
            .boxed(),
        prop_variable().map(Syntax::Thunk).boxed(),
    ]);

    let branch = one_of([
        syntax.generator().map(|s| Syntax::Not(Box::new(s))).boxed(),
        tuples!(syntax.generator(), syntax.generator())
            .map(|(l, r)| Syntax::And(Box::new(l), Box::new(r)))
            .boxed(),
        tuples!(syntax.generator(), syntax.generator())
            .map(|(l, r)| Syntax::Or(Box::new(l), Box::new(r)))
            .boxed(),
        tuples!(syntax.generator(), syntax.generator())
            .map(|(l, r)| Syntax::Implies(Box::new(l), Box::new(r)))
            .boxed(),
    ]);

    let result = syntax.generator();
    syntax.set(one_of([leaf.boxed(), branch.boxed()]));
    result
}

/// Recursively compute which thunk indices contributed to a formula being true. Returns
/// `Some(indices)` when the formula is true, `None` when false.
fn truth_contributing(
    formula: &Formula<SnapshotDomain>,
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

fn actual_snapshot_indices(value: &Value<SnapshotDomain>) -> BTreeSet<usize> {
    match value {
        Value::True(state) => state.0.values().map(|s| s.index).collect(),
        _ => BTreeSet::new(),
    }
}

#[hegel::test]
fn test_true_snapshots_equal_truth_contributing(tc: TestCase) {
    let syntax = tc.draw(nontemporal_syntax());
    let state_x = tc.draw(booleans());
    let state_y = tc.draw(booleans());
    let formula = syntax.nnf();
    let expected = truth_contributing(&formula, state_x, state_y);

    let mut evaluate_thunk = |variable: &Variable, negated: bool| {
        let raw = match variable {
            Variable::X => state_x,
            Variable::Y => state_y,
            Variable::Z => unreachable!(),
        };
        let value = if negated { !raw } else { raw };
        let index = variable_index(variable);
        let name = match variable {
            Variable::X => "x_val",
            Variable::Y => "y_val",
            Variable::Z => "z_val",
        };
        Ok((
            Formula::Pure {
                value,
                pretty: format!("{:?}={}", variable, value),
            },
            TestSnapshots::from_snapshot(snapshot(index, name)),
        ))
    };
    let mut evaluator: Evaluator<'_, SnapshotDomain, Error> =
        Evaluator::new(&mut evaluate_thunk);
    let value = evaluator.evaluate(&formula, TestTime::ZERO).unwrap();

    match (&expected, &value) {
        (Some(expected_indices), Value::True(_)) => {
            let actual = actual_snapshot_indices(&value);
            assert_eq!(
                expected_indices, &actual,
                "formula: {:?}, x={}, y={}",
                syntax, state_x, state_y,
            );
        }
        (None, Value::False(_, _)) => {}
        (Some(_), Value::False(_, _)) => {
            panic!(
                "oracle=true, evaluator=false: {:?}, x={}, y={}",
                syntax, state_x, state_y,
            );
        }
        (None, Value::True(_)) => {
            panic!(
                "oracle=false, evaluator=true: {:?}, x={}, y={}",
                syntax, state_x, state_y,
            );
        }
        (_, Value::Residual(_)) => {
            panic!("non-temporal formula produced Residual",);
        }
    }
}

#[test]
fn test_thunk_returning_implies_preserves_outer_snapshots() {
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
            Variable::X => Ok((
                Formula::Implies(
                    Box::new(Formula::Pure {
                        value: true,
                        pretty: "true".to_string(),
                    }),
                    Box::new(thunk(Variable::Y)),
                ),
                variable_snapshot(variable),
            )),
            _ => Ok((
                Formula::Pure {
                    value,
                    pretty: format!("{:?}={}", variable, value),
                },
                variable_snapshot(variable),
            )),
        }
    };

    let mut evaluator: Evaluator<'_, SnapshotDomain, Error> =
        Evaluator::new(&mut evaluate_thunk);
    let value = evaluator
        .evaluate(&thunk(Variable::X), TestTime::ZERO)
        .unwrap();

    assert!(matches!(value, Value::False(_, _)));
    if let Value::False(violation, _) = &value {
        let names = violation_state_names(violation);
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
    let time_t0 = TestTime::ZERO;
    let time_t1 = TestTime::from_secs(1);

    let mut evaluate_thunk = |variable: &Variable, negated: bool| {
        let state = current_state.borrow();
        let value = match variable {
            Variable::X => state.x,
            Variable::Y => state.y,
            Variable::Z => state.z,
        };
        let value = if negated { !value } else { value };

        match variable {
            Variable::X => Ok((
                Formula::Implies(
                    Box::new(thunk(Variable::Y)),
                    Box::new(thunk(Variable::Z)),
                ),
                variable_snapshot(variable),
            )),
            _ => {
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
                    TestSnapshots::from_snapshot(snapshot(index, name)),
                ))
            }
        }
    };

    let mut evaluator: Evaluator<'_, SnapshotDomain, Error> =
        Evaluator::new(&mut evaluate_thunk);

    let value = evaluator
        .evaluate(
            &Formula::Always(Box::new(thunk(Variable::X)), None),
            time_t0,
        )
        .unwrap();
    assert!(matches!(value, Value::Residual(_)));

    *current_state.borrow_mut() = &state_t1;
    let residual = match value {
        Value::Residual(r) => r,
        _ => unreachable!(),
    };
    let value = evaluator.step(&residual, time_t1).unwrap();

    assert!(matches!(value, Value::False(_, _)));
    if let Value::False(Violation::Always { violation, .. }, _) = &value {
        if let Violation::Implies { state, right, .. } = violation.as_ref() {
            let names = state.names();

            assert!(
                names.contains(&"x_val".to_string()),
                "x snapshot from outer thunk missing from antecedent: \
                 {:?}",
                names
            );
            assert!(
                names.contains(&"y_val".to_string()),
                "y snapshot missing from antecedent: {:?}",
                names
            );

            if let Violation::False {
                state: consequent_state,
                ..
            } = right.as_ref()
            {
                let consequent_names = consequent_state.names();
                assert!(
                    consequent_names.contains(&"z_val".to_string()),
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
