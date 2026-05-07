use crate::formula::{Domain, Formula};

/// A formula in its syntactic form. In Bombadil this structure is parsed from
/// JavaScript runtime objects.
#[derive(Debug, Clone, PartialEq)]
pub enum Syntax<D: Domain> {
    Pure { value: bool, pretty: String },
    Thunk(D::Function),
    Not(Box<Syntax<D>>),
    And(Box<Syntax<D>>, Box<Syntax<D>>),
    Or(Box<Syntax<D>>, Box<Syntax<D>>),
    Implies(Box<Syntax<D>>, Box<Syntax<D>>),
    Next(Box<Syntax<D>>),
    Always(Box<Syntax<D>>, Option<D::Duration>),
    Eventually(Box<Syntax<D>>, Option<D::Duration>),
}

impl<D: Domain> Syntax<D> {
    pub fn nnf(&self) -> Formula<D> {
        fn go<D: Domain>(node: &Syntax<D>, negated: bool) -> Formula<D> {
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
