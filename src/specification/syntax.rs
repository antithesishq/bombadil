use std::time::Duration;

use crate::specification::ltl::Formula;

/// A formula in its syntactic form, "parsed" from JavaScript runtime objects.
#[derive(Debug, Clone, PartialEq)]
pub enum Syntax<Function> {
    Pure { value: bool, pretty: String },
    Thunk(Function),
    Not(Box<Syntax<Function>>),
    And(Box<Syntax<Function>>, Box<Syntax<Function>>),
    Or(Box<Syntax<Function>>, Box<Syntax<Function>>),
    Implies(Box<Syntax<Function>>, Box<Syntax<Function>>),
    Next(Box<Syntax<Function>>),
    Always(Box<Syntax<Function>>, Option<Duration>),
    Eventually(Box<Syntax<Function>>, Option<Duration>),
}

impl<Function: Clone> Syntax<Function> {
    pub fn nnf(&self) -> Formula<Function> {
        fn go<Function: Clone>(
            node: &Syntax<Function>,
            negated: bool,
        ) -> Formula<Function> {
            match node {
                Syntax::Pure { value, pretty } => Formula::Pure {
                    value: if negated { !*value } else { *value },
                    pretty: pretty.clone(),
                },
                Syntax::Thunk(function) => Formula::Thunk {
                    function: function.clone(),
                    negated,
                },
                Syntax::Not(syntax) => go(syntax, !negated),
                Syntax::And(left, right) => {
                    if negated {
                        //   ¬(l ∧ r)
                        // ⇔ (¬l ∨ ¬r)
                        Formula::Or(
                            Box::new(go(left, negated)),
                            Box::new(go(right, negated)),
                        )
                    } else {
                        Formula::And(
                            Box::new(go(left, negated)),
                            Box::new(go(right, negated)),
                        )
                    }
                }
                Syntax::Or(left, right) => {
                    if negated {
                        //   ¬(l ∨ r)
                        // ⇔ (¬l ∧ ¬r)
                        Formula::And(
                            Box::new(go(left, negated)),
                            Box::new(go(right, negated)),
                        )
                    } else {
                        Formula::Or(
                            Box::new(go(left, negated)),
                            Box::new(go(right, negated)),
                        )
                    }
                }
                Syntax::Implies(left, right) => {
                    if negated {
                        //   ¬(l ⇒ r)
                        // ⇔ ¬(¬l ∨ r)
                        // ⇔ l ∧ ¬r
                        Formula::And(
                            Box::new(go(left, false)),
                            Box::new(go(right, true)),
                        )
                    } else {
                        Formula::Implies(
                            Box::new(go(left, negated)),
                            Box::new(go(right, negated)),
                        )
                    }
                }
                Syntax::Next(sub) => Formula::Next(Box::new(go(sub, negated))),
                Syntax::Always(sub, bound) => {
                    if negated {
                        Formula::Eventually(Box::new(go(sub, negated)), *bound)
                    } else {
                        Formula::Always(Box::new(go(sub, negated)), *bound)
                    }
                }
                Syntax::Eventually(sub, bound) => {
                    if negated {
                        Formula::Always(Box::new(go(sub, negated)), *bound)
                    } else {
                        Formula::Eventually(Box::new(go(sub, negated)), *bound)
                    }
                }
            }
        }
        go(self, false)
    }
}
