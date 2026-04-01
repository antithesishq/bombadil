use std::time::SystemTime;

use bombadil_inspect_api::{
    EventuallyViolation, Formula, PropertyViolation, Snapshot, Violation,
};
use yew::prelude::*;

use crate::duration::{format_bound, format_duration};

pub enum Inline {
    Text(String),
    Code(String),
    Time(SystemTime),
    Keyword(String),
}

pub enum Markup {
    Span(Vec<Inline>),
    CodeBlock(String),
    Snapshots(Vec<SnapshotMarkup>),
    Stack(Vec<Markup>),
    Join(Vec<Markup>),
    Comma,
}

pub struct SnapshotMarkup {
    pub name: String,
    pub value: serde_json::Value,
}

pub fn render_violation(
    violation: &PropertyViolation,
    test_start: SystemTime,
) -> Markup {
    render_violation_inner(&violation.violation, test_start)
}

fn render_violation_inner(
    violation: &Violation,
    test_start: SystemTime,
) -> Markup {
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
            render_violation_inner(violation, test_start),
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
            render_violation_inner(violation, test_start),
        ]),
        Violation::And { left, right } => Markup::Join(vec![
            render_violation_inner(left, test_start),
            Markup::Span(vec![Inline::Keyword("and".into())]),
            render_violation_inner(right, test_start),
        ]),
        Violation::Or { left, right } => Markup::Join(vec![
            render_violation_inner(left, test_start),
            Markup::Span(vec![Inline::Keyword("or".into())]),
            render_violation_inner(right, test_start),
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
                    render_violation_inner(right, test_start),
                ])
            } else {
                Markup::Join(vec![
                    render_formula(left),
                    Markup::Span(vec![Inline::Keyword("implies".into())]),
                    render_violation_inner(right, test_start),
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

fn is_inline(markup: &Markup) -> bool {
    match markup {
        Markup::Span(_) => true,
        Markup::CodeBlock(_) => false,
        Markup::Snapshots(items) => {
            items.iter().all(|item| is_json_inline(&item.value))
        }
        Markup::Stack(_) => false,
        Markup::Join(items) => items.iter().all(is_inline),
        Markup::Comma => true,
    }
}

fn is_json_inline(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Array(items) => items.is_empty(),
        serde_json::Value::Object(map) => map.is_empty(),
        _ => true,
    }
}

pub fn markup_to_html(markup: &Markup, test_start: SystemTime) -> Html {
    match markup {
        Markup::Span(inlines) => {
            html!(
                <span>
                    { for inlines.iter().map(|inline| inline_to_html(inline, test_start)) }
                </span>
            )
        }
        Markup::CodeBlock(code) => html!(<pre><code>{code}</code></pre>),
        Markup::Snapshots(items) => {
            let all_inline =
                items.iter().all(|item| is_json_inline(&item.value));
            if all_inline {
                html!(
                    <span class="snapshot-inline">
                        <dl class="snapshot-values inline">
                            { for items.iter().map(|item| {
                                html!(
                                    <div class="json-entry inline">
                                        <dt>{&item.name}</dt>
                                        <dd>{render_json(&item.value)}</dd>
                                    </div>
                                )
                            }) }
                        </dl>
                    </span>
                )
            } else {
                html!(
                    <dl class="snapshot-values">
                        { for items.iter().map(|item| {
                            let class = if is_json_inline(&item.value) {
                                "json-entry inline"
                            } else {
                                "json-entry"
                            };
                            html!(
                                <div class={class}>
                                    <dt>{&item.name}</dt>
                                    <dd>{render_json(&item.value)}</dd>
                                </div>
                            )
                        }) }
                    </dl>
                )
            }
        }
        Markup::Stack(items) => {
            html!(
                <>
                    { for items.iter().map(|item| markup_to_html(item, test_start)) }
                </>
            )
        }
        Markup::Join(items) => {
            fn flatten_joins(items: &[Markup]) -> Vec<&Markup> {
                let mut result = Vec::new();
                for item in items {
                    if let Markup::Join(nested) = item {
                        result.extend(flatten_joins(nested));
                    } else {
                        result.push(item);
                    }
                }
                result
            }

            let flattened = flatten_joins(items);
            let mut result = Vec::new();
            let mut pending_spans = Vec::new();
            let mut next_separator_has_comma = false;
            let mut previous_non_comma_index = None;

            let flush_pending =
                |pending: &mut Vec<Html>, result: &mut Vec<Html>| {
                    if !pending.is_empty() {
                        if !result.is_empty() {
                            result.push(html!({ "\n\n" }));
                        }
                        result.push(html!(<p>{ for pending.drain(..) }</p>));
                    }
                };

            for (i, item) in flattened.iter().enumerate() {
                if matches!(item, Markup::Comma) {
                    next_separator_has_comma = true;
                    continue;
                }

                let current_inline = is_inline(item);

                if let Some(previous_index) = previous_non_comma_index {
                    let previous_inline = is_inline(flattened[previous_index]);

                    match (previous_inline, current_inline) {
                        (true, true) => {
                            let separator = if next_separator_has_comma {
                                ", "
                            } else {
                                " "
                            };
                            pending_spans.push(html!({ separator }));
                        }
                        (true, false) => {
                            pending_spans.push(html!({ ":" }));
                            flush_pending(&mut pending_spans, &mut result);
                        }
                        (false, false) => {
                            if !result.is_empty() {
                                result.push(html!({ "\n\n" }));
                            }
                        }
                        (false, true) => {}
                    }
                    next_separator_has_comma = false;
                }

                previous_non_comma_index = Some(i);

                if current_inline {
                    pending_spans.push(markup_to_html(item, test_start));
                } else {
                    result.push(markup_to_html(item, test_start));
                }
            }

            flush_pending(&mut pending_spans, &mut result);

            html!(<>{ for result }</>)
        }
        Markup::Comma => html!(),
    }
}

fn inline_to_html(inline: &Inline, test_start: SystemTime) -> Html {
    match inline {
        Inline::Text(text) => html!({ text }),
        Inline::Code(code) => html!(<code>{code}</code>),
        Inline::Time(time) => {
            html!(<time>{format_time(time, test_start)}</time>)
        }
        Inline::Keyword(keyword) => {
            html!(<span class="keyword">{keyword}</span>)
        }
    }
}

fn format_time(time: &SystemTime, test_start: SystemTime) -> String {
    format_duration(
        time.duration_since(test_start)
            .expect("timestamp millisecond conversion failed"),
    )
}

fn render_json(value: &serde_json::Value) -> Html {
    match value {
        serde_json::Value::Array(items) if items.is_empty() => {
            html!(<code class="json-literal">{"[]"}</code>)
        }
        serde_json::Value::Array(items) => {
            html!(
                <ul class="json-array">
                    { for items.iter().map(|item| html!(<li>{render_json(item)}</li>)) }
                </ul>
            )
        }
        serde_json::Value::Object(map) if map.is_empty() => {
            html!(<code class="json-literal">{"{}"}</code>)
        }
        serde_json::Value::Object(map) => {
            html!(
                <dl class="json-object">
                    { for map.iter().map(|(key, val)| {
                        let class = if is_json_inline(val) {
                            "json-entry inline"
                        } else {
                            "json-entry"
                        };
                        html!(
                            <div class={class}>
                                <dt>{key}</dt>
                                <dd>{render_json(val)}</dd>
                            </div>
                        )
                    }) }
                </dl>
            )
        }
        serde_json::Value::String(s) if is_printable(s) => {
            html!(<span class="json-string">{s}</span>)
        }
        serde_json::Value::String(s) => {
            let literal = serde_json::Value::String(s.clone()).to_string();
            html!(
                <code class="json-literal" title={s.clone()}>
                    {literal}
                </code>
            )
        }
        other => {
            html!(<code class="json-literal">{other.to_string()}</code>)
        }
    }
}

fn is_printable(s: &str) -> bool {
    s.chars().all(|c| !c.is_control() || c == '\n' || c == '\t')
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
