use std::time::Duration;

use crate::schema::{
    EventuallyViolation, Formula, PropertyViolation, Snapshot, Time, Violation,
};

#[derive(Debug, Clone)]
pub enum Inline {
    Text(String),
    Code(String),
    Time(Time),
    Keyword(String),
}

#[derive(Debug, Clone)]
pub enum Markup {
    Span(Vec<Inline>),
    CodeBlock(String),
    Snapshots(Vec<SnapshotMarkup>),
    Stack(Vec<Markup>),
    Join(Vec<Markup>),
    Comma,
}

#[derive(Debug, Clone)]
pub struct SnapshotMarkup {
    pub name: String,
    pub value: serde_json::Value,
}

pub fn render_violation(violation: &PropertyViolation) -> Markup {
    let current_time = get_violation_time(&violation.violation);
    render_violation_inner(&violation.violation, current_time)
}

fn get_violation_time(violation: &Violation) -> Time {
    let mut current = violation;
    loop {
        match current {
            Violation::False { time, .. } => return *time,
            Violation::Always { time, .. } => return *time,
            Violation::Eventually { reason, .. } => {
                return match reason {
                    EventuallyViolation::TimedOut(time) => *time,
                    EventuallyViolation::TestEnded => Time::from_system_time(
                        std::time::SystemTime::UNIX_EPOCH,
                    ),
                };
            }
            Violation::Implies { right, .. } => {
                current = right.as_ref();
            }
            Violation::And { left, .. } => {
                current = left.as_ref();
            }
            Violation::Or { left, .. } => {
                current = left.as_ref();
            }
        }
    }
}

fn render_violation_inner(violation: &Violation, current_time: Time) -> Markup {
    match violation {
        Violation::False {
            snapshots,
            condition,
            ..
        } => {
            if snapshots.is_empty() {
                render_code(format!("!({condition})"))
            } else {
                render_snapshot_values(snapshots, current_time)
            }
        }
        Violation::Eventually { subformula, reason } => match reason {
            EventuallyViolation::TimedOut(time) => Markup::Join(vec![
                Markup::Span(vec![Inline::Keyword("eventually".into())]),
                render_formula(subformula),
                Markup::Comma,
                Markup::Span(vec![Inline::Text("which timed out at".into())]),
                Markup::Span(vec![Inline::Time(*time)]),
            ]),
            EventuallyViolation::TestEnded => Markup::Join(vec![
                Markup::Span(vec![Inline::Keyword("eventually".into())]),
                render_formula(subformula),
                Markup::Comma,
                Markup::Span(vec![Inline::Text("which never occurred".into())]),
            ]),
        },
        Violation::Always {
            violation,
            subformula,
            start,
            end: None,
            time,
        } => Markup::Join(vec![
            Markup::Span(vec![Inline::Text("as of".into())]),
            Markup::Span(vec![Inline::Time(*start)]),
            Markup::Comma,
            Markup::Span(vec![Inline::Text(
                "it should always be the case that".into(),
            )]),
            render_formula(subformula),
            Markup::Comma,
            Markup::Span(vec![Inline::Text("however".into())]),
            render_violation_inner(violation, *time),
        ]),
        Violation::Always {
            violation,
            subformula,
            start,
            end: Some(end),
            time,
        } => Markup::Join(vec![
            Markup::Span(vec![Inline::Text("as of".into())]),
            Markup::Span(vec![Inline::Time(*start)]),
            Markup::Span(vec![Inline::Text("and until".into())]),
            Markup::Span(vec![Inline::Time(*end)]),
            Markup::Comma,
            Markup::Span(vec![Inline::Text(
                "it should always be the case that".into(),
            )]),
            render_formula(subformula),
            Markup::Comma,
            Markup::Span(vec![Inline::Text("however".into())]),
            render_violation_inner(violation, *time),
        ]),
        Violation::And { left, right } => Markup::Join(vec![
            render_violation_inner(left, current_time),
            Markup::Span(vec![Inline::Keyword("and".into())]),
            render_violation_inner(right, current_time),
        ]),
        Violation::Or { left, right } => Markup::Join(vec![
            render_violation_inner(left, current_time),
            Markup::Span(vec![Inline::Keyword("and".into())]),
            render_violation_inner(right, current_time),
        ]),
        Violation::Implies {
            left,
            right,
            antecedent_snapshots,
        } => {
            // Use the consequent's time as "current" for grouping snapshots
            let implies_time = get_violation_time(right);
            if !antecedent_snapshots.is_empty() {
                Markup::Join(vec![
                    render_snapshot_values(antecedent_snapshots, implies_time),
                    Markup::Comma,
                    Markup::Span(vec![Inline::Text(
                        "which implied that".into(),
                    )]),
                    render_formula(left),
                    Markup::Comma,
                    Markup::Span(vec![Inline::Text(
                        "however the implication failed because".into(),
                    )]),
                    render_violation_inner(right, implies_time),
                ])
            } else {
                Markup::Join(vec![
                    render_formula(left),
                    Markup::Span(vec![Inline::Keyword("implies".into())]),
                    render_violation_inner(right, implies_time),
                ])
            }
        }
    }
}

fn render_code(code: String) -> Markup {
    if code.contains("\n") {
        Markup::CodeBlock(code)
    } else {
        Markup::Span(vec![Inline::Code(code)])
    }
}

fn render_snapshot_values(
    snapshots: &[Snapshot],
    current_time: Time,
) -> Markup {
    use std::collections::BTreeMap;

    let (current, closed_over): (Vec<_>, Vec<_>) =
        snapshots.iter().partition(|s| s.time == current_time);

    // Group closed-over snapshots by their time
    let mut by_time: BTreeMap<Time, Vec<&Snapshot>> = BTreeMap::new();
    for snapshot in &closed_over {
        by_time.entry(snapshot.time).or_default().push(snapshot);
    }

    let mut result = Vec::new();

    // Show current snapshots first
    if !current.is_empty() {
        result.push(Markup::Span(vec![
            Inline::Text("at ".into()),
            Inline::Time(current_time),
        ]));
        result.push(Markup::Snapshots(render_snapshot_items(&current)));
    }

    // Show closed-over snapshots grouped by time (newest to oldest)
    for (time, snapshots) in by_time.iter().rev() {
        if !result.is_empty() {
            result.push(Markup::Comma);
            result.push(Markup::Span(vec![Inline::Text(" and ".into())]));
        }
        result.push(Markup::Span(vec![
            Inline::Text("from the prior state at ".into()),
            Inline::Time(*time),
        ]));
        result.push(Markup::Snapshots(render_snapshot_items(snapshots)));
    }

    Markup::Join(result)
}

fn render_snapshot_items(snapshots: &[&Snapshot]) -> Vec<SnapshotMarkup> {
    let mut items = Vec::new();
    for snapshot in snapshots.iter() {
        let name = snapshot_name(snapshot);
        items.push(SnapshotMarkup {
            name,
            value: snapshot.value.clone(),
        });
    }
    items
}

fn snapshot_name(snapshot: &Snapshot) -> String {
    snapshot
        .name
        .as_deref()
        .map(String::from)
        .unwrap_or_else(|| format!("extractors[{}]", snapshot.index))
}

pub fn format_bound(duration: Duration) -> String {
    let milliseconds = duration.as_millis();

    if milliseconds == 0 {
        return "0 milliseconds".to_string();
    }

    if milliseconds.is_multiple_of(60_000) {
        let minutes = milliseconds / 60_000;
        if minutes == 1 {
            "1 minute".to_string()
        } else {
            format!("{} minutes", minutes)
        }
    } else if milliseconds.is_multiple_of(1_000) {
        let seconds = milliseconds / 1_000;
        if seconds == 1 {
            "1 second".to_string()
        } else {
            format!("{} seconds", seconds)
        }
    } else if milliseconds == 1 {
        "1 millisecond".to_string()
    } else {
        format!("{} milliseconds", milliseconds)
    }
}

fn render_formula(formula: &Formula) -> Markup {
    match formula {
        Formula::Pure { value: _, pretty } => render_code(pretty.clone()),
        Formula::Thunk {
            function,
            negated: true,
        } => render_code(format!("not({})", function)),
        Formula::Thunk {
            function,
            negated: false,
        } => render_code(function.clone()),
        Formula::And(left, right) => Markup::Join(vec![
            render_formula(left),
            Markup::Span(vec![Inline::Keyword("and".into())]),
            render_formula(right),
        ]),
        Formula::Or(left, right) => Markup::Join(vec![
            render_formula(left),
            Markup::Span(vec![Inline::Keyword("or".into())]),
            render_formula(right),
        ]),
        Formula::Implies(left, right) => Markup::Join(vec![
            Markup::Span(vec![Inline::Keyword("if".into())]),
            render_formula(left),
            Markup::Span(vec![Inline::Keyword("then".into())]),
            render_formula(right),
        ]),
        Formula::Next(formula) => Markup::Join(vec![
            Markup::Span(vec![Inline::Keyword("next".into())]),
            render_formula(formula),
        ]),
        Formula::Always(formula, None) => Markup::Join(vec![
            Markup::Span(vec![Inline::Keyword("always".into())]),
            render_formula(formula),
        ]),
        Formula::Always(formula, Some(bound)) => Markup::Join(vec![
            Markup::Span(vec![Inline::Text(format!(
                "for {}",
                format_bound(*bound)
            ))]),
            render_formula(formula),
        ]),
        Formula::Eventually(formula, None) => Markup::Join(vec![
            Markup::Span(vec![Inline::Keyword("eventually".into())]),
            render_formula(formula),
        ]),
        Formula::Eventually(formula, Some(bound)) => Markup::Join(vec![
            Markup::Span(vec![Inline::Text("within".into())]),
            Markup::Span(vec![Inline::Text(format_bound(*bound))]),
            render_formula(formula),
        ]),
    }
}
