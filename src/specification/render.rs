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
            Violation::Always {
                violation,
                subformula,
                start,
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
            Violation::And { left, right } => {
                write!(
                    f,
                    "{} and {}",
                    RenderedViolation(left),
                    RenderedViolation(right),
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
            Formula::True { pretty } => write!(f, "{}", pretty),
            Formula::False { pretty } => write!(f, "{}", pretty),
            Formula::Contextful(function) => write!(f, "{}", function),
            Formula::Always(formula) => {
                write!(f, "always({})", RenderedFormula(formula))
            }
            Formula::Eventually(formula, duration) => {
                write!(
                    f,
                    "eventually({}).within({}, \"milliseconds\")",
                    RenderedFormula(formula),
                    duration.as_millis()
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
