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
                write!(
                    f,
                    "eventually {}",
                    RenderedFormula((*subformula).as_ref())
                )?;
                match reason {
                    EventuallyViolation::TimedOut(time) => {
                        write!(
                            f,
                            " (which timed out at {}ms)",
                            time_to_ms(time)
                        )?;
                    }
                    EventuallyViolation::TestEnded => {
                        write!(f, " (which never occurred)")?;
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
                if !antecedent_snapshot_references.is_empty() {
                    render_snapshot_inline(
                        f,
                        antecedent_snapshot_references,
                        self.trace,
                    )?;
                    write!(f, ", implying:")?;
                } else {
                    write!(f, "{} implies:", RenderedFormula(left))?;
                }
                write!(
                    f,
                    "\n\n{}",
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
                     but at {}ms:\n\n{}",
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
                     but at {}ms:\n\n{}",
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
                        writeln!(f)?;
                    }
                    first = false;
                    let fallback = format!("extractor[{}]", extractor_index);
                    let name = snapshot.name.as_deref().unwrap_or(&fallback);
                    write!(f, "{} =", name)?;
                    write_json_value(f, &snapshot.value, 1)?;
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
                    if !first {
                        write!(f, ", ")?;
                    }
                    first = false;
                    let fallback = format!("extractor[{}]", extractor_index);
                    let name = snapshot.name.as_deref().unwrap_or(&fallback);
                    write!(f, "{} = ", name)?;
                    write_json_scalar(f, &snapshot.value)?;
                }
            }
        }
    }
    Ok(())
}

fn is_printable(s: &str) -> bool {
    s.chars().all(|c| !c.is_control() || c == '\n' || c == '\t')
}

fn write_indent(
    f: &mut std::fmt::Formatter<'_>,
    depth: usize,
) -> std::fmt::Result {
    for _ in 0..depth {
        write!(f, "  ")?;
    }
    Ok(())
}

fn write_json_scalar(
    f: &mut std::fmt::Formatter<'_>,
    value: &json::Value,
) -> std::fmt::Result {
    match value {
        json::Value::String(s) if is_printable(s) => write!(f, "{}", s),
        other => write!(f, "{}", other),
    }
}

fn write_json_value(
    f: &mut std::fmt::Formatter<'_>,
    value: &json::Value,
    depth: usize,
) -> std::fmt::Result {
    match value {
        json::Value::Array(items) => {
            for item in items {
                writeln!(f)?;
                write_indent(f, depth)?;
                write!(f, "- ")?;
                write_json_value(f, item, depth + 1)?;
            }
        }
        json::Value::Object(map) => {
            for (key, val) in map {
                writeln!(f)?;
                write_indent(f, depth)?;
                write!(f, "{}:", key)?;
                write_json_value(f, val, depth + 1)?;
            }
        }
        scalar => {
            write!(f, " ")?;
            write_json_scalar(f, scalar)?;
        }
    }
    Ok(())
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
                    "{} and {}",
                    RenderedFormula(left),
                    RenderedFormula(right)
                )
            }
            Formula::Or(left, right) => {
                write!(
                    f,
                    "{} or {}",
                    RenderedFormula(left),
                    RenderedFormula(right)
                )
            }
            Formula::Implies(left, right) => {
                write!(
                    f,
                    "{} implies {}",
                    RenderedFormula(left),
                    RenderedFormula(right)
                )
            }
            Formula::Next(formula) => {
                write!(f, "next {}", RenderedFormula(formula))
            }
            Formula::Always(formula, None) => {
                write!(f, "always {}", RenderedFormula(formula))
            }
            Formula::Always(formula, Some(bound)) => {
                write!(
                    f,
                    "always {} within {}ms",
                    RenderedFormula(formula),
                    bound.as_millis()
                )
            }
            Formula::Eventually(formula, None) => {
                write!(f, "eventually {}", RenderedFormula(formula))
            }
            Formula::Eventually(formula, Some(bound)) => {
                write!(
                    f,
                    "eventually {} within {}ms",
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::specification::ltl::ExtractorSet;

    fn pretty(s: &str) -> PrettyFunction {
        PrettyFunction(s.to_string())
    }

    fn thunk(s: &str) -> Formula<PrettyFunction> {
        Formula::Thunk {
            function: pretty(s),
            negated: false,
        }
    }

    fn time_at(ms: u64) -> Time {
        UNIX_EPOCH + Duration::from_millis(ms)
    }

    fn extractor_set(indices: &[usize]) -> ExtractorSet {
        let mut set = ExtractorSet::default();
        for &i in indices {
            set.insert(i);
        }
        set
    }

    #[test]
    fn test_render_always_implies_eventually() {
        // always((() => x > 10).implies(eventually(() => y == 20)))
        //
        // x > 10 becomes true at step 1, but y == 20 never occurs.
        let violation = Violation::Always {
            subformula: Box::new(Formula::Implies(
                Box::new(thunk("x > 10")),
                Box::new(Formula::Eventually(Box::new(thunk("y == 20")), None)),
            )),
            start: time_at(100),
            end: None,
            time: time_at(500),
            violation: Box::new(Violation::Implies {
                left: thunk("x > 10"),
                right: Box::new(Violation::Eventually {
                    subformula: Box::new(thunk("y == 20")),
                    reason: EventuallyViolation::TestEnded,
                }),
                antecedent_snapshot_references: vec![(1, extractor_set(&[0]))],
            }),
        };

        let trace = vec![
            vec![
                Snapshot {
                    name: Some("x".into()),
                    value: json::json!(5),
                },
                Snapshot {
                    name: Some("y".into()),
                    value: json::json!(0),
                },
            ],
            vec![
                Snapshot {
                    name: Some("x".into()),
                    value: json::json!(11),
                },
                Snapshot {
                    name: Some("y".into()),
                    value: json::json!(0),
                },
            ],
        ];

        let rendered = render_violation(&violation, &trace);
        assert_eq!(
            rendered,
            "\
as of 100ms, it should always be the case that:

x > 10 implies eventually y == 20

but at 500ms:

x = 11, implying:

eventually y == 20 (which never occurred)"
        );
    }

    #[test]
    fn test_render_invariant_violation() {
        // always(() => count.current <= 5)
        //
        // count becomes 6 at step 2.
        let violation = Violation::Always {
            subformula: Box::new(thunk("count.current <= 5")),
            start: time_at(0),
            end: None,
            time: time_at(300),
            violation: Box::new(Violation::False {
                time: time_at(300),
                condition: "count.current <= 5".into(),
                snapshot_references: vec![(2, extractor_set(&[0]))],
            }),
        };

        let trace = vec![
            vec![Snapshot {
                name: Some("count".into()),
                value: json::json!(0),
            }],
            vec![Snapshot {
                name: Some("count".into()),
                value: json::json!(3),
            }],
            vec![Snapshot {
                name: Some("count".into()),
                value: json::json!(6),
            }],
        ];

        let rendered = render_violation(&violation, &trace);
        assert_eq!(
            rendered,
            "\
as of 0ms, it should always be the case that:

count.current <= 5

but at 300ms:

count = 6"
        );
    }
}
