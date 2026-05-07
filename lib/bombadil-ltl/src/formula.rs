use std::fmt::Debug;
use std::ops::Add;

pub trait State: Clone + Default + Debug + PartialEq {
    fn merge(&self, other: &Self) -> Self;
    fn is_empty(&self) -> bool;
}

impl State for () {
    fn merge(&self, _other: &Self) -> Self {}
    fn is_empty(&self) -> bool {
        true
    }
}

pub trait Domain: Clone + Debug + PartialEq {
    type Function: Clone + Debug + PartialEq;
    type Time: Copy
        + Ord
        + Debug
        + PartialEq
        + Add<Self::Duration, Output = Self::Time>;
    type Duration: Copy + Debug + PartialEq;
    type State: State;
}

/// A formula in negation normal form (NNF), up to thunks. Note
/// that `Implies` is preserved for better error messages.
#[derive(Clone, Debug, PartialEq)]
pub enum Formula<D: Domain> {
    Pure {
        value: bool,
        pretty: String,
    },
    Thunk {
        function: D::Function,
        negated: bool,
    },
    And(Box<Formula<D>>, Box<Formula<D>>),
    Or(Box<Formula<D>>, Box<Formula<D>>),
    Implies(Box<Formula<D>>, Box<Formula<D>>),
    Next(Box<Formula<D>>),
    Always(Box<Formula<D>>, Option<D::Duration>),
    Eventually(Box<Formula<D>>, Option<D::Duration>),
}

impl<D: Domain> Formula<D> {
    pub fn map_function<
        U: Domain<Time = D::Time, Duration = D::Duration, State = D::State>,
    >(
        &self,
        f: impl Fn(&D::Function) -> U::Function,
    ) -> Formula<U> {
        self.map_function_ref(&f)
    }

    pub(crate) fn map_function_ref<
        U: Domain<Time = D::Time, Duration = D::Duration, State = D::State>,
    >(
        &self,
        f: &impl Fn(&D::Function) -> U::Function,
    ) -> Formula<U> {
        match self {
            Formula::Pure { value, pretty } => Formula::Pure {
                value: *value,
                pretty: pretty.clone(),
            },
            Formula::Thunk { function, negated } => Formula::Thunk {
                function: f(function),
                negated: *negated,
            },
            Formula::And(left, right) => Formula::And(
                Box::new(left.map_function_ref(f)),
                Box::new(right.map_function_ref(f)),
            ),
            Formula::Or(left, right) => Formula::Or(
                Box::new(left.map_function_ref(f)),
                Box::new(right.map_function_ref(f)),
            ),
            Formula::Implies(left, right) => Formula::Implies(
                Box::new(left.map_function_ref(f)),
                Box::new(right.map_function_ref(f)),
            ),
            Formula::Next(formula) => {
                Formula::Next(Box::new(formula.map_function_ref(f)))
            }
            Formula::Always(formula, bound) => {
                Formula::Always(Box::new(formula.map_function_ref(f)), *bound)
            }
            Formula::Eventually(formula, bound) => Formula::Eventually(
                Box::new(formula.map_function_ref(f)),
                *bound,
            ),
        }
    }
}
