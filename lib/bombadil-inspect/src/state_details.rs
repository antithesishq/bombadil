use std::rc::Rc;
use std::time::SystemTime;

use bombadil_inspect_api::EventuallyViolation;
use bombadil_inspect_api::Formula;
use bombadil_inspect_api::PropertyViolation;
use bombadil_inspect_api::Snapshot;
use bombadil_inspect_api::TraceEntry;
use bombadil_inspect_api::Violation;
use serde_json as json;
use yew::component;
use yew::prelude::*;

use crate::container_size::use_container_size;
use crate::duration::format_duration;

#[derive(PartialEq, Properties)]
pub struct StateDetailsProps {
    pub entry: Rc<TraceEntry>,
    pub trace: Rc<[TraceEntry]>,
    pub test_start: SystemTime,
}

#[component]
pub fn StateDetails(props: &StateDetailsProps) -> Html {
    let (container_ref, container_size) = use_container_size();
    html!(
        <>
            <details open={true} ref={container_ref} class={if props.entry.violations.is_empty() {""} else {"has-violations"}}>
                {
                    if !props.entry.violations.is_empty() && let Some((width, height)) = container_size {
                        html!(
                            <svg class="background" xmlns="http://www.w3.org/2000/svg">
                                <rect width={width.to_string()} height={height.to_string()} fill="url(#violation)" />
                            </svg>
                        )
                    } else {
                        html!()
                    }
                }
                <summary>
                {format!("Violations ({})", props.entry.violations.len())}
                </summary>
                <ol class="numbered">
                {
                    props
                        .entry
                        .violations
                        .iter()
                        .map(|violation| html!(<li>{render_violation(violation, props.test_start, &props.trace)}</li>))
                        .collect::<Html>()
                }
                </ol>
            </details>
            <details open={true}>
                <summary>{"Snapshots"}</summary>
                <dl class="snapshots">
                {
                    props
                        .entry
                        .snapshots
                        .iter()
                        .map(|snapshot| html!(
                            <>
                                <dt>{snapshot.name.as_deref().unwrap_or("<unnamed>")}</dt>
                                <dd>{render_json(&snapshot.value)}</dd>
                            </>
                        ))
                        .collect::<Html>()
                }
                </dl>
            </details>
        </>
    )
}

fn render_violation(
    violation: &PropertyViolation,
    test_start: SystemTime,
    trace: &[TraceEntry],
) -> Html {
    html!(
        <div class="violation">
            <div class="violation-name">{&violation.name}{":"}</div>
            {render_violation_inner(&violation.violation, test_start, trace)}
        </div>
    )
}

fn render_violation_inner(
    violation: &Violation,
    test_start: SystemTime,
    trace: &[TraceEntry],
) -> Html {
    match violation {
        Violation::False {
            snapshot_references,
            condition,
            ..
        } => {
            if snapshot_references.is_empty() {
                html!(<pre><code>{condition}</code></pre>)
            } else {
                render_snapshot_values(snapshot_references, trace)
            }
        }
        Violation::Eventually { subformula, reason } => {
            let reason_text = match reason {
                EventuallyViolation::TimedOut(time) => {
                    format!(
                        "(timed out at {})",
                        format_time(time, test_start),
                    )
                }
                EventuallyViolation::TestEnded => {
                    "(never occurred)".to_string()
                }
            };
            html!(
                <>
                    {render_formula(subformula)}
                    <span>{reason_text}</span>
                </>
            )
        }
        Violation::Always {
            violation,
            subformula,
            start,
            end: None,
            time,
        } => {
            html!(
                <>
                    <span>{
                        format!(
                            "as of {}, it should always \
                             be the case that",
                            format_time(start, test_start),
                        )
                    }</span>
                    {render_formula(subformula)}
                    <span>{
                        format!("but at {}:", format_time(time, test_start))
                    }</span>
                    {render_violation_inner(violation, test_start, trace)}
                </>
            )
        }
        Violation::Always {
            violation,
            subformula,
            start,
            end: Some(end),
            time,
        } => {
            html!(
                <>
                    <span>{
                        format!(
                            "as of {} and until {}, it \
                             should always be the case that",
                            format_time(start, test_start),
                            format_time(end, test_start),
                        )
                    }</span>
                    {render_formula(subformula)}
                    <span>{
                        format!("but at {}:", format_time(time, test_start))
                    }</span>
                    {render_violation_inner(violation, test_start, trace)}
                </>
            )
        }
        Violation::And { left, right } => {
            html!(
                <>
                    {render_violation_inner(left, test_start, trace)}
                    <span class="keyword">{"and"}</span>
                    {render_violation_inner(right, test_start, trace)}
                </>
            )
        }
        Violation::Or { left, right } => {
            html!(
                <>
                    {render_violation_inner(left, test_start, trace)}
                    <span class="keyword">{"or"}</span>
                    {render_violation_inner(right, test_start, trace)}
                </>
            )
        }
        Violation::Implies {
            left,
            right,
            antecedent_snapshot_references,
        } => {
            html!(
                <>
                    <span>
                        {render_formula(left)}
                        <span class="keyword">{" implies"}</span>
                        {
                            if !antecedent_snapshot_references.is_empty() {
                                html!(
                                    <span class="antecedent-context">
                                        {" (was true"}
                                        {render_snapshot_inline(
                                            antecedent_snapshot_references,
                                            trace,
                                        )}
                                        {")"}
                                    </span>
                                )
                            } else {
                                html!()
                            }
                        }
                        {":"}
                    </span>
                    {render_violation_inner(right, test_start, trace)}
                </>
            )
        }
    }
}

fn render_snapshot_values(
    references: &[(usize, Vec<usize>)],
    trace: &[TraceEntry],
) -> Html {
    let items = collect_snapshot_items(references, trace);
    html!(
        <dl class="snapshot-values">
            { for items.into_iter() }
        </dl>
    )
}

fn render_snapshot_inline(
    references: &[(usize, Vec<usize>)],
    trace: &[TraceEntry],
) -> Html {
    let items = collect_snapshot_items(references, trace);
    if items.is_empty() {
        return html!();
    }
    html!(
        <span class="snapshot-inline">
            {" with "}
            <dl class="snapshot-values inline">
                { for items.into_iter() }
            </dl>
        </span>
    )
}

fn collect_snapshot_items(
    references: &[(usize, Vec<usize>)],
    trace: &[TraceEntry],
) -> Vec<Html> {
    let mut items = Vec::new();
    for (state_index, extractor_indices) in references {
        if let Some(entry) = trace.get(*state_index) {
            for &extractor_index in extractor_indices {
                if let Some(snapshot) = entry.snapshots.get(extractor_index) {
                    let name = snapshot_name(snapshot, extractor_index);
                    items.push(html!(
                        <>
                            <dt>{name}</dt>
                            <dd>{render_json(&snapshot.value)}</dd>
                        </>
                    ));
                }
            }
        }
    }
    items
}

fn snapshot_name(snapshot: &Snapshot, extractor_index: usize) -> String {
    snapshot
        .name
        .as_deref()
        .map(String::from)
        .unwrap_or_else(|| format!("extractor[{}]", extractor_index))
}

fn is_printable(s: &str) -> bool {
    s.chars().all(|c| !c.is_control() || c == '\n' || c == '\t')
}

fn render_json(value: &json::Value) -> Html {
    match value {
        json::Value::Array(items) => {
            html!(
                <ul class="json-array">
                    { for items.iter().map(|item| html!(<li>{render_json(item)}</li>)) }
                </ul>
            )
        }
        json::Value::Object(map) => {
            html!(
                <dl class="json-object">
                    { for map.iter().map(|(key, val)| html!(
                        <>
                            <dt>{key}</dt>
                            <dd>{render_json(val)}</dd>
                        </>
                    )) }
                </dl>
            )
        }
        json::Value::String(s) if is_printable(s) => {
            html!(<span class="json-string">{s}</span>)
        }
        other => {
            html!(<code class="json-literal">{other.to_string()}</code>)
        }
    }
}

fn render_formula(formula: &Formula) -> Html {
    match formula {
        Formula::Pure { value: _, pretty } => {
            html!(<code>{pretty}</code>)
        }
        Formula::Thunk {
            function,
            negated: true,
        } => {
            html!(<pre><code>{format!("not({})", function)}</code></pre>)
        }
        Formula::Thunk {
            function,
            negated: false,
        } => {
            html!(<pre><code>{function}</code></pre>)
        }
        Formula::And(left, right) => {
            html!(
                <span class="formula-and">
                    {render_formula(left)}
                    <code>{".and("}</code>
                    {render_formula(right)}
                    <code>{")"}</code>
                </span>
            )
        }
        Formula::Or(left, right) => {
            html!(
                <span class="formula-or">
                    {render_formula(left)}
                    <code>{".or("}</code>
                    {render_formula(right)}
                    <code>{")"}</code>
                </span>
            )
        }
        Formula::Implies(left, right) => {
            html!(
                <span class="formula-implies">
                    {render_formula(left)}
                    <code>{".implies("}</code>
                    {render_formula(right)}
                    <code>{")"}</code>
                </span>
            )
        }
        Formula::Next(formula) => {
            html!(
                <span class="formula-next">
                    <code>{"next("}</code>
                    {render_formula(formula)}
                    <code>{")"}</code>
                </span>
            )
        }
        Formula::Always(formula, None) => {
            html!(
                <span class="formula-always">
                    <code>{"always("}</code>
                    {render_formula(formula)}
                    <code>{")"}</code>
                </span>
            )
        }
        Formula::Always(formula, Some(bound)) => {
            html!(
                <span class="formula-always">
                    <code>{"always("}</code>
                    {render_formula(formula)}
                    <code>{
                        format!(
                            ").within({}, \"milliseconds\")",
                            bound.as_millis(),
                        )
                    }</code>
                </span>
            )
        }
        Formula::Eventually(formula, None) => {
            html!(
                <span class="formula-eventually">
                    <code>{"eventually("}</code>
                    {render_formula(formula)}
                    <code>{")"}</code>
                </span>
            )
        }
        Formula::Eventually(formula, Some(bound)) => {
            html!(
                <span class="formula-eventually">
                    <code>{"eventually("}</code>
                    {render_formula(formula)}
                    <code>{
                        format!(
                            ").within({}, \"milliseconds\")",
                            bound.as_millis(),
                        )
                    }</code>
                </span>
            )
        }
    }
}

fn format_time(time: &SystemTime, test_start: SystemTime) -> String {
    format_duration(
        time.duration_since(test_start)
            .expect("timestamp millisecond conversion failed"),
    )
}
