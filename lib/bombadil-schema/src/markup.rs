use std::time::{Duration, SystemTime};

use crate::schema::{
    EventuallyViolation, Formula, PropertyViolation, Snapshot, Violation,
};

#[derive(Debug, Clone)]
pub enum Inline {
    Text(String),
    Code(String),
    Time(SystemTime),
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
    render_violation_inner(&violation.violation)
}

fn render_violation_inner(violation: &Violation) -> Markup {
    match violation {
        Violation::False {
            snapshots,
            condition,
            ..
        } => {
            if snapshots.is_empty() {
                render_code(format!("!({condition})"))
            } else {
                render_snapshot_values(snapshots)
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
            Markup::Span(vec![Inline::Text("but at".into())]),
            Markup::Span(vec![Inline::Time(*time)]),
            Markup::Comma,
            render_violation_inner(violation),
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
            Markup::Span(vec![Inline::Text("but at".into())]),
            Markup::Span(vec![Inline::Time(*time)]),
            Markup::Comma,
            render_violation_inner(violation),
        ]),
        Violation::And { left, right } => Markup::Join(vec![
            render_violation_inner(left),
            Markup::Span(vec![Inline::Keyword("and".into())]),
            render_violation_inner(right),
        ]),
        Violation::Or { left, right } => Markup::Join(vec![
            render_violation_inner(left),
            Markup::Span(vec![Inline::Keyword("and".into())]),
            render_violation_inner(right),
        ]),
        Violation::Implies {
            left,
            right,
            antecedent_snapshots,
        } => {
            if !antecedent_snapshots.is_empty() {
                Markup::Join(vec![
                    render_snapshot_values(antecedent_snapshots),
                    Markup::Comma,
                    Markup::Span(vec![Inline::Text("implying that".into())]),
                    render_violation_inner(right),
                ])
            } else {
                Markup::Join(vec![
                    render_formula(left),
                    Markup::Span(vec![Inline::Keyword("implies".into())]),
                    render_violation_inner(right),
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

fn render_snapshot_values(snapshots: &[Snapshot]) -> Markup {
    let items = render_snapshot_items(snapshots);
    Markup::Snapshots(items)
}

fn render_snapshot_items(snapshots: &[Snapshot]) -> Vec<SnapshotMarkup> {
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
