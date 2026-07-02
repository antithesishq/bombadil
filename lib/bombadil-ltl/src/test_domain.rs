use std::{marker::PhantomData, ops::Add, time::Duration};

use anyhow::Error;
use hegel::{
    Generator,
    generators::{booleans, deferred, durations, just, one_of, optional},
    tuples,
};

use crate::{eval::*, formula::*, syntax::Syntax, violation::*};

#[derive(Clone, Debug, PartialEq)]
pub struct TestDomain<Snapshot = ()> {
    _marker: PhantomData<Snapshot>,
}

impl<Snapshots: Clone + std::fmt::Debug + State> Domain
    for TestDomain<Snapshots>
{
    type Function = Variable;
    type Time = TestTime;
    type Duration = Duration;
    type State = Snapshots;
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct TestState {
    pub x: bool,
    pub y: bool,
    pub z: bool,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Variable {
    X,
    Y,
    Z,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TestTime(pub u64);

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

impl TestTime {
    pub const ZERO: Self = TestTime(0);

    pub fn from_millis(millis: u64) -> TestTime {
        TestTime(millis)
    }

    pub fn from_secs(secs: u64) -> TestTime {
        TestTime(secs * 1000)
    }
}

pub fn evaluate_with_state<Snapshot: State>(
    formula: &Formula<TestDomain<Snapshot>>,
    state: &TestState,
    variable_to_state: fn(&Variable) -> Snapshot,
) -> Value<TestDomain<Snapshot>> {
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
            variable_to_state(variable),
        ))
    };
    let mut evaluator: Evaluator<'_, TestDomain<Snapshot>, Error> =
        Evaluator::new(&mut evaluate_thunk);
    evaluator.evaluate(formula, TestTime(0)).unwrap()
}

pub fn step_with_state<Snapshot: State>(
    residual: &Residual<TestDomain<Snapshot>>,
    state: &TestState,
    time: TestTime,
    variable_to_state: fn(&Variable) -> Snapshot,
) -> Value<TestDomain<Snapshot>> {
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
            variable_to_state(variable),
        ))
    };
    let mut evaluator: Evaluator<'_, TestDomain<Snapshot>, Error> =
        Evaluator::new(&mut evaluate_thunk);
    evaluator.step(residual, time).unwrap()
}

pub fn has_nested_unbounded_always<Snapshot: State>(
    root: &Formula<TestDomain<Snapshot>>,
) -> bool {
    let mut stack = vec![(root, false)];
    while let Some((formula, in_always)) = stack.pop() {
        match formula {
            Formula::Pure { .. } | Formula::Thunk { .. } => {}
            Formula::And(left, right)
            | Formula::Or(left, right)
            | Formula::Implies(left, right) => {
                stack.push((left, in_always));
                stack.push((right, in_always));
            }
            Formula::Next(formula)
            | Formula::Eventually(formula, _)
            | Formula::Always(formula, _) => {
                if in_always {
                    return true;
                } else {
                    stack.push((formula, true));
                }
            }
        }
    }
    false
}

pub fn residual_depth<D: Domain>(root: &Residual<D>) -> usize {
    let mut stack: Vec<(&Residual<D>, usize)> = vec![(root, 1)];
    let mut depth_max = 0;
    while let Some((residual, depth)) = stack.pop() {
        depth_max = depth_max.max(depth);
        match residual {
            Residual::True(_)
            | Residual::False(_)
            | Residual::Derived(_, _) => {}
            Residual::And { left, right }
            | Residual::Or { left, right }
            | Residual::OrEventually { left, right, .. }
            | Residual::Implies { left, right, .. } => {
                stack.push((left, depth + 1));
                stack.push((right, depth + 1));
            }
            Residual::AndAlways { pending, .. } => {
                for residual in pending {
                    stack.push((residual, depth + 1));
                }
            }
        }
    }
    depth_max
}

pub fn formula_depth<Snapshot: State>(
    root: &Formula<TestDomain<Snapshot>>,
) -> usize {
    let mut stack: Vec<(&Formula<TestDomain<Snapshot>>, usize)> =
        vec![(root, 1)];
    let mut depth_max = 0;
    while let Some((residual, depth)) = stack.pop() {
        depth_max = depth_max.max(depth);
        match residual {
            Formula::Pure { .. } | Formula::Thunk { .. } => {}
            Formula::And(left, right)
            | Formula::Or(left, right)
            | Formula::Implies(left, right) => {
                stack.push((left, depth + 1));
                stack.push((right, depth + 1));
            }
            Formula::Next(subformula)
            | Formula::Eventually(subformula, _)
            | Formula::Always(subformula, _) => {
                stack.push((subformula, depth + 1))
            }
        }
    }
    depth_max
}

pub fn violation_depth<Snapshot: State>(
    root: &Violation<TestDomain<Snapshot>>,
) -> usize {
    let mut stack: Vec<(&Violation<TestDomain<Snapshot>>, usize)> =
        vec![(root, 1)];
    let mut depth_max = 0;
    while let Some((violation, depth)) = stack.pop() {
        depth_max = depth_max.max(depth);
        match violation {
            Violation::False { .. } | Violation::Eventually { .. } => {}
            Violation::Always { violation, .. } => {
                stack.push((violation, depth + 1))
            }
            Violation::And { left, right } | Violation::Or { left, right } => {
                stack.push((left, depth + 1));
                stack.push((right, depth + 1));
            }
            Violation::Implies { right, .. } => {
                stack.push((right, depth + 1));
            }
        }
    }
    depth_max
}

pub fn variable() -> impl Generator<Variable> {
    one_of([just(Variable::X).boxed(), just(Variable::Y).boxed()]).boxed()
}

pub fn state() -> impl Generator<TestState> {
    tuples!(booleans(), booleans(), booleans()).map(|(x, y, z)| TestState {
        x,
        y,
        z,
    })
}

pub fn syntax<Snapshot: State + 'static>()
-> impl Generator<Syntax<TestDomain<Snapshot>>> {
    let syntax = deferred::<Syntax<TestDomain<Snapshot>>>();
    let leaf = one_of([
        booleans()
            .map(|value| Syntax::Pure {
                value,
                pretty: format!("{}", value),
            })
            .boxed(),
        variable().map(Syntax::Thunk).boxed(),
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
        tuples!(
            syntax.generator(),
            optional(
                durations()
                    .min_value(Duration::from_millis(1))
                    .max_value(Duration::from_millis(10))
            )
        )
        .map(|(l, r)| Syntax::Eventually(Box::new(l), r))
        .boxed(),
        tuples!(
            syntax.generator(),
            optional(
                durations()
                    .min_value(Duration::from_millis(1))
                    .max_value(Duration::from_millis(10))
            )
        )
        .map(|(l, r)| Syntax::Always(Box::new(l), r))
        .boxed(),
    ]);

    let result = syntax.generator();
    syntax.set(one_of([leaf.boxed(), branch.boxed()]));
    result
}
