use std::time::UNIX_EPOCH;

use serde::Serialize;
use serde_json as json;

use crate::specification::{
    js::RuntimeFunction,
    ltl::{EventuallyViolation, Formula, SnapshotReferences, Time, Violation},
    verifier::Snapshot,
};

pub fn render_violation(
    violation: &Violation<PrettyFunction>,
    trace: &[Vec<Snapshot>],
) -> String {
    format!("{}", RenderedViolation { violation, trace })
}

struct RenderedViolation<'a> {
    violation: &'a Violation<PrettyFunction>,
    trace: &'a [Vec<Snapshot>],
}

impl<'a> std::fmt::Display for RenderedViolation<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.violation {
            Violation::False {
                condition,
                snapshot_references,
                ..
            } => {
                if snapshot_references.is_empty() {
                    write!(f, "{}", condition)?;
                } else {
                    render_snapshot_values(f, snapshot_references, self.trace)?;
                }
            }
            Violation::Eventually { subformula, reason } => {
                write!(f, "{}", RenderedFormula((*subformula).as_ref()))?;
                match reason {
                    EventuallyViolation::TimedOut(time) => {
                        write!(f, " (timed out at {}ms)", time_to_ms(time))?;
                    }
                    EventuallyViolation::TestEnded => {
                        write!(f, " (never occurred)")?;
                    }
                }
            }
            Violation::And { left, right } => {
                write!(
                    f,
                    "{}\n\nand\n\n{}",
                    RenderedViolation {
                        violation: left,
                        trace: self.trace
                    },
                    RenderedViolation {
                        violation: right,
                        trace: self.trace
                    },
                )?;
            }
            Violation::Or { left, right } => {
                write!(
                    f,
                    "{} or {}",
                    RenderedViolation {
                        violation: left,
                        trace: self.trace
                    },
                    RenderedViolation {
                        violation: right,
                        trace: self.trace
                    },
                )?;
            }
            Violation::Implies {
                left,
                right,
                antecedent_snapshot_references,
            } => {
                write!(f, "{} implies", RenderedFormula(left))?;
                if !antecedent_snapshot_references.is_empty() {
                    write!(f, " (was true")?;
                    render_snapshot_inline(
                        f,
                        antecedent_snapshot_references,
                        self.trace,
                    )?;
                    write!(f, ")")?;
                }
                write!(
                    f,
                    ":\n\n{}",
                    RenderedViolation {
                        violation: right,
                        trace: self.trace
                    },
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
                    "as of {}ms, it should always be the case that:\n\n\
                     {}\n\n\
                     but at {}ms, {}",
                    time_to_ms(start),
                    RenderedFormula((*subformula).as_ref()),
                    time_to_ms(time),
                    RenderedViolation {
                        violation,
                        trace: self.trace
                    },
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
                    "as of {}ms and until {}ms, \
                     it should always be the case that:\n\n\
                     {}\n\n\
                     but at {}ms, {}",
                    time_to_ms(start),
                    time_to_ms(end),
                    RenderedFormula((*subformula).as_ref()),
                    time_to_ms(time),
                    RenderedViolation {
                        violation,
                        trace: self.trace
                    },
                )?;
            }
        };
        Ok(())
    }
}

fn render_snapshot_values(
    f: &mut std::fmt::Formatter<'_>,
    references: &SnapshotReferences,
    trace: &[Vec<Snapshot>],
) -> std::fmt::Result {
    let mut first = true;
    for (state_index, extractor_set) in references {
        if let Some(snapshots) = trace.get(*state_index) {
            for extractor_index in extractor_set.iter() {
                if let Some(snapshot) = snapshots.get(extractor_index) {
                    if !first {
                        write!(f, ", ")?;
                    }
                    first = false;
                    let fallback = format!("extractor[{}]", extractor_index);
                    let name = snapshot.name.as_deref().unwrap_or(&fallback);
                    write!(
                        f,
                        "{} = {}",
                        name,
                        format_json_value(&snapshot.value),
                    )?;
                }
            }
        }
    }
    Ok(())
}

fn render_snapshot_inline(
    f: &mut std::fmt::Formatter<'_>,
    references: &SnapshotReferences,
    trace: &[Vec<Snapshot>],
) -> std::fmt::Result {
    let mut first = true;
    for (state_index, extractor_set) in references {
        if let Some(snapshots) = trace.get(*state_index) {
            for extractor_index in extractor_set.iter() {
                if let Some(snapshot) = snapshots.get(extractor_index) {
                    if first {
                        write!(f, " with ")?;
                    } else {
                        write!(f, ", ")?;
                    }
                    first = false;
                    let fallback = format!("extractor[{}]", extractor_index);
                    let name = snapshot.name.as_deref().unwrap_or(&fallback);
                    write!(
                        f,
                        "{} = {}",
                        name,
                        format_json_value(&snapshot.value),
                    )?;
                }
            }
        }
    }
    Ok(())
}

fn format_json_value(value: &json::Value) -> String {
    match value {
        json::Value::String(s) => format!("{:?}", s),
        other => other.to_string(),
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

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PrettyFunction(String);

impl std::fmt::Display for PrettyFunction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Formula<RuntimeFunction> {
    pub fn with_pretty_functions(&self) -> Formula<PrettyFunction> {
        self.map_function(|f| PrettyFunction(f.pretty.clone()))
    }
}

impl Violation<RuntimeFunction> {
    pub fn with_pretty_functions(&self) -> Violation<PrettyFunction> {
        self.map_function(|f| PrettyFunction(f.pretty.clone()))
    }
}
