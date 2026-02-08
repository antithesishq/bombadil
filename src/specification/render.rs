use std::time::UNIX_EPOCH;

use crate::specification::ltl::{
    EventuallyViolation, Formula, PrettyFunction, Time, Violation,
};

pub fn render_violation(violation: &Violation<PrettyFunction>) -> String {
    format!("{}", RenderedViolation(violation))
}

struct RenderedViolation<'a>(&'a Violation<PrettyFunction>);

impl<'a> std::fmt::Display for RenderedViolation<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            Violation::False { condition, .. } => {
                write!(f, "!({})", condition)?;
            }
            Violation::Eventually { subformula, reason } => {
                match reason {
                    EventuallyViolation::TimedOut(time) => {
                        write!(f, "timed out at {}ms: ", time_to_ms(time))?
                    }
                    EventuallyViolation::TestEnded => {
                        write!(f, "failed at test end: ")?
                    }
                }
                write!(f, "{}", RenderedFormula((*subformula).as_ref()))?;
            }
            Violation::And { left, right } => {
                write!(
                    f,
                    "{}\n\nand\n\n{}",
                    RenderedViolation(left),
                    RenderedViolation(right),
                )?;
            }
            Violation::Or { left, right } => {
                write!(
                    f,
                    "{} or {}",
                    RenderedViolation(left),
                    RenderedViolation(right),
                )?;
            }
            Violation::Implies { left, right } => {
                write!(
                    f,
                    "{} since {}",
                    RenderedViolation(right),
                    RenderedFormula(left),
                )?;
            }
            Violation::Always {
                violation,
                subformula,
                start,
                end: None,
                time,
            } => {
                write!(
                    f,
                    "as of {}ms, it should always be the case that\n\n{}\n\nbut at {}ms\n\n{}",
                    time_to_ms(start),
                    RenderedFormula((*subformula).as_ref()),
                    time_to_ms(time),
                    RenderedViolation(violation),
                )?;
            }
            Violation::Always {
                violation,
                subformula,
                start,
                end: Some(end),
                time,
            } => {
                write!(
                    f,
                    "as of {}ms and until {}ms, it should alwaays be the case that\n\n{}\n\nbut at {}ms\n\n{}",
                    time_to_ms(start),
                    time_to_ms(end),
                    RenderedFormula((*subformula).as_ref()),
                    time_to_ms(time),
                    RenderedViolation(violation),
                )?;
            }
        };
        Ok(())
    }
}

struct RenderedFormula<'a>(&'a Formula<PrettyFunction>);

impl<'a> std::fmt::Display for RenderedFormula<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            Formula::Pure { value: _, pretty } => write!(f, "{}", pretty),
            Formula::Thunk { function, negated } => {
                if *negated {
                    write!(f, "not({})", function)
                } else {
                    write!(f, "{}", function)
                }
            }
            Formula::And(left, right) => {
                write!(
                    f,
                    "{}.and({})",
                    RenderedFormula(left),
                    RenderedFormula(right)
                )
            }
            Formula::Or(left, right) => {
                write!(
                    f,
                    "{}.or({})",
                    RenderedFormula(left),
                    RenderedFormula(right)
                )
            }
            Formula::Implies(left, right) => {
                write!(
                    f,
                    "{}.implies({})",
                    RenderedFormula(left),
                    RenderedFormula(right)
                )
            }
            Formula::Next(formula) => {
                write!(f, "next({})", RenderedFormula(formula))
            }
            Formula::Always(formula, None) => {
                write!(f, "always({})", RenderedFormula(formula))
            }
            Formula::Always(formula, Some(bound)) => {
                write!(
                    f,
                    "always({}).within({}, \"milliseconds\")",
                    RenderedFormula(formula),
                    bound.as_millis()
                )
            }
            Formula::Eventually(formula, None) => {
                write!(f, "eventually({})", RenderedFormula(formula))
            }
            Formula::Eventually(formula, Some(bound)) => {
                write!(
                    f,
                    "eventually({}).within({}, \"milliseconds\")",
                    RenderedFormula(formula),
                    bound.as_millis()
                )
            }
        }
    }
}

fn time_to_ms(time: &Time) -> u128 {
    time.duration_since(UNIX_EPOCH)
        .expect("timestamp millisecond conversion failed")
        .as_millis()
}
