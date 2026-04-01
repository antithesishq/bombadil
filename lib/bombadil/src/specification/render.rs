use std::time::{Duration, SystemTime};

use serde::Serialize;
use serde_json as json;

use crate::specification::{
    js::RuntimeFunction,
    ltl::{EventuallyViolation, Formula, Time, Violation},
    verifier::Snapshot,
};

pub fn render_violation(
    violation: &Violation<PrettyFunction>,
    test_start: SystemTime,
) -> String {
    format!(
        "{}",
        RenderedViolation {
            violation,
            test_start,
        }
    )
}

struct RenderedViolation<'a> {
    violation: &'a Violation<PrettyFunction>,
    test_start: SystemTime,
}

impl<'a> std::fmt::Display for RenderedViolation<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.violation {
            Violation::False {
                condition,
                snapshots,
                ..
            } => {
                if snapshots.is_empty() {
                    write!(f, "!({})", condition)?;
                } else {
                    render_snapshot_values(f, snapshots)?;
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
                            " (which timed out at {})",
                            format_time(time, self.test_start)
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
                        test_start: self.test_start,
                    },
                    RenderedViolation {
                        violation: right,
                        test_start: self.test_start,
                    },
                )?;
            }
            Violation::Or { left, right } => {
                write!(
                    f,
                    "{} or {}",
                    RenderedViolation {
                        violation: left,
                        test_start: self.test_start,
                    },
                    RenderedViolation {
                        violation: right,
                        test_start: self.test_start,
                    },
                )?;
            }
            Violation::Implies {
                left,
                right,
                antecedent_snapshots,
            } => {
                if !antecedent_snapshots.is_empty() {
                    render_snapshot_inline(f, antecedent_snapshots)?;
                    write!(f, ", implying:")?;
                } else {
                    write!(f, "{} implies:", RenderedFormula(left))?;
                }
                write!(
                    f,
                    "\n\n{}",
                    RenderedViolation {
                        violation: right,
                        test_start: self.test_start,
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
                    "as of {}, it should always be the case \
                     that:\n\n\
                     {}\n\n\
                     but at {}:\n\n{}",
                    format_time(start, self.test_start),
                    RenderedFormula((*subformula).as_ref()),
                    format_time(time, self.test_start),
                    RenderedViolation {
                        violation,
                        test_start: self.test_start,
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
                    "as of {} and until {}, \
                     it should always be the case that:\n\n\
                     {}\n\n\
                     but at {}:\n\n{}",
                    format_time(start, self.test_start),
                    format_time(end, self.test_start),
                    RenderedFormula((*subformula).as_ref()),
                    format_time(time, self.test_start),
                    RenderedViolation {
                        violation,
                        test_start: self.test_start,
                    },
                )?;
            }
        };
        Ok(())
    }
}

fn render_snapshot_values(
    f: &mut std::fmt::Formatter<'_>,
    snapshots: &[Snapshot],
) -> std::fmt::Result {
    let mut first = true;
    for (i, snapshot) in snapshots.iter().enumerate() {
        if !first {
            writeln!(f)?;
        }
        first = false;
        let fallback = format!("extractor[{}]", i);
        let name = snapshot.name.as_deref().unwrap_or(&fallback);
        write!(f, "{} =", name)?;
        write_json_value(f, &snapshot.value, 1)?;
    }
    Ok(())
}

fn render_snapshot_inline(
    f: &mut std::fmt::Formatter<'_>,
    snapshots: &[Snapshot],
) -> std::fmt::Result {
    let mut first = true;
    for (i, snapshot) in snapshots.iter().enumerate() {
        if !first {
            write!(f, ", ")?;
        }
        first = false;
        let fallback = format!("extractor[{}]", i);
        let name = snapshot.name.as_deref().unwrap_or(&fallback);
        write!(f, "{} = ", name)?;
        write_json_scalar(f, &snapshot.value)?;
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
        json::Value::Array(items) if items.is_empty() => {
            write!(f, " []")?;
        }
        json::Value::Array(items) => {
            for item in items {
                writeln!(f)?;
                write_indent(f, depth)?;
                write!(f, "-")?;
                write_json_value(f, item, depth + 1)?;
            }
        }
        json::Value::Object(map) if map.is_empty() => {
            write!(f, " {{}}")?;
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
                    "always {} within {}",
                    RenderedFormula(formula),
                    format_bound(bound)
                )
            }
            Formula::Eventually(formula, None) => {
                write!(f, "eventually {}", RenderedFormula(formula))
            }
            Formula::Eventually(formula, Some(bound)) => {
                write!(
                    f,
                    "eventually {} within {}",
                    RenderedFormula(formula),
                    format_bound(bound)
                )
            }
        }
    }
}

fn format_time(time: &Time, test_start: SystemTime) -> String {
    format_duration(
        time.duration_since(test_start)
            .expect("timestamp millisecond conversion failed"),
    )
}

fn format_bound(duration: &Duration) -> String {
    let millis = duration.as_millis();
    if millis == 0 {
        return "0 milliseconds".to_string();
    }
    let (value, unit) = if millis.is_multiple_of(60_000) {
        let minutes = millis / 60_000;
        (minutes, if minutes == 1 { "minute" } else { "minutes" })
    } else if millis.is_multiple_of(1_000) {
        let seconds = millis / 1_000;
        (seconds, if seconds == 1 { "second" } else { "seconds" })
    } else {
        (
            millis,
            if millis == 1 {
                "millisecond"
            } else {
                "milliseconds"
            },
        )
    };
    format!("{} {}", value, unit)
}

fn format_duration(duration: Duration) -> String {
    let total_secs = duration.as_secs();
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;
    if hours > 0 {
        format!("{:02}:{:02}:{:02}", hours, minutes, seconds)
    } else {
        format!("{:02}:{:02}", minutes, seconds)
    }
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

impl Formula<PrettyFunction> {
    pub fn to_api(&self) -> bombadil_schema::Formula {
        match self {
            Formula::Pure { value, pretty } => bombadil_schema::Formula::Pure {
                value: *value,
                pretty: pretty.clone(),
            },
            Formula::Thunk { function, negated } => {
                bombadil_schema::Formula::Thunk {
                    function: function.0.clone(),
                    negated: *negated,
                }
            }
            Formula::And(left, right) => bombadil_schema::Formula::And(
                Box::new(left.to_api()),
                Box::new(right.to_api()),
            ),
            Formula::Or(left, right) => bombadil_schema::Formula::Or(
                Box::new(left.to_api()),
                Box::new(right.to_api()),
            ),
            Formula::Implies(left, right) => bombadil_schema::Formula::Implies(
                Box::new(left.to_api()),
                Box::new(right.to_api()),
            ),
            Formula::Next(formula) => {
                bombadil_schema::Formula::Next(Box::new(formula.to_api()))
            }
            Formula::Always(formula, bound) => {
                bombadil_schema::Formula::Always(
                    Box::new(formula.to_api()),
                    *bound,
                )
            }
            Formula::Eventually(formula, bound) => {
                bombadil_schema::Formula::Eventually(
                    Box::new(formula.to_api()),
                    *bound,
                )
            }
        }
    }
}

impl Violation<PrettyFunction> {
    pub fn to_api(&self) -> bombadil_schema::Violation {
        match self {
            Violation::False {
                time,
                condition,
                snapshots,
            } => bombadil_schema::Violation::False {
                time: *time,
                condition: condition.clone(),
                snapshots: snapshots.iter().map(|s| s.to_api()).collect(),
            },
            Violation::Eventually { subformula, reason } => {
                bombadil_schema::Violation::Eventually {
                    subformula: Box::new(subformula.to_api()),
                    reason: reason.to_api(),
                }
            }
            Violation::Always {
                violation,
                subformula,
                start,
                end,
                time,
            } => bombadil_schema::Violation::Always {
                violation: Box::new(violation.to_api()),
                subformula: Box::new(subformula.to_api()),
                start: *start,
                end: *end,
                time: *time,
            },
            Violation::And { left, right } => bombadil_schema::Violation::And {
                left: Box::new(left.to_api()),
                right: Box::new(right.to_api()),
            },
            Violation::Or { left, right } => bombadil_schema::Violation::Or {
                left: Box::new(left.to_api()),
                right: Box::new(right.to_api()),
            },
            Violation::Implies {
                left,
                right,
                antecedent_snapshots,
            } => bombadil_schema::Violation::Implies {
                left: left.to_api(),
                right: Box::new(right.to_api()),
                antecedent_snapshots: antecedent_snapshots
                    .iter()
                    .map(|s| s.to_api())
                    .collect(),
            },
        }
    }
}

impl EventuallyViolation {
    pub fn to_api(&self) -> bombadil_schema::EventuallyViolation {
        match self {
            EventuallyViolation::TimedOut(time) => {
                bombadil_schema::EventuallyViolation::TimedOut(*time)
            }
            EventuallyViolation::TestEnded => {
                bombadil_schema::EventuallyViolation::TestEnded
            }
        }
    }
}

impl Snapshot {
    pub fn to_api(&self) -> bombadil_schema::Snapshot {
        bombadil_schema::Snapshot {
            index: self.index,
            name: self.name.clone(),
            value: self.value.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::specification::verifier::Snapshot;

    use super::*;

    fn pretty(s: &str) -> PrettyFunction {
        PrettyFunction(s.to_string())
    }

    fn thunk(s: &str) -> Formula<PrettyFunction> {
        Formula::Thunk {
            function: pretty(s),
            negated: false,
        }
    }

    const TEST_START: SystemTime = SystemTime::UNIX_EPOCH;

    fn time_at(seconds: u64) -> Time {
        SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
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
            start: time_at(60),
            end: None,
            time: time_at(120),
            violation: Box::new(Violation::Implies {
                left: thunk("x > 10"),
                right: Box::new(Violation::Eventually {
                    subformula: Box::new(thunk("y == 20")),
                    reason: EventuallyViolation::TestEnded,
                }),
                antecedent_snapshots: vec![Snapshot {
                    index: 0,
                    name: Some("x".into()),
                    value: json::json!(11),
                }],
            }),
        };

        let rendered = render_violation(&violation, TEST_START);
        assert_eq!(
            rendered,
            "\
as of 01:00, it should always be the case that:

x > 10 implies eventually y == 20

but at 02:00:

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
            time: time_at(305),
            violation: Box::new(Violation::False {
                time: time_at(305),
                condition: "count.current <= 5".into(),
                snapshots: vec![Snapshot {
                    index: 0,
                    name: Some("count".into()),
                    value: json::json!(6),
                }],
            }),
        };

        let rendered = render_violation(&violation, TEST_START);
        assert_eq!(
            rendered,
            "\
as of 00:00, it should always be the case that:

count.current <= 5

but at 05:05:

count = 6"
        );
    }
}
