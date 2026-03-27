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
                <ol>
                {
                    props
                        .entry
                        .violations
                        .iter()
                        .map(|violation| html!(<li>{render_violation(violation, props.test_start)}</li>))
                        .collect::<Html>()
                }
                </ol>
            </details>
            <details>
                <summary>{"Snapshots"}</summary>
                <dl class="snapshots">
                {
                    {
                        let options = JsonRenderOptions {
                            literal_strings: true,
                        };
                        props
                            .entry
                            .snapshots
                            .iter()
                            .map(|snapshot| {
                                let class =
                                    if is_json_inline(&snapshot.value) {
                                        "json-entry inline"
                                    } else {
                                        "json-entry"
                                    };
                                html!(
                                    <div class={class}>
                                        <dt>{snapshot.name.as_deref().unwrap_or("<unnamed>")}</dt>
                                        <dd>{render_json(&snapshot.value, options)}</dd>
                                    </div>
                                )
                            })
                            .collect::<Html>()
                    }
                }
                </dl>
            </details>
        </>
    )
}

fn render_violation(
    violation: &PropertyViolation,
    test_start: SystemTime,
) -> Html {
    html!(
        <div class="violation">
            <div class="violation-name">{&violation.name}{":"}</div>
            {render_violation_inner(&violation.violation, test_start)}
        </div>
    )
}

fn render_violation_inner(
    violation: &Violation,
    test_start: SystemTime,
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
                let options = JsonRenderOptions {
                    literal_strings: false,
                };
                render_snapshot_values(snapshot_references, options)
            }
        }
        Violation::Eventually { subformula, reason } => {
            let reason_html = match reason {
                EventuallyViolation::TimedOut(time) => {
                    html!(
                        <>
                            {"(which timed out at "}
                            <time>{format_time(time, test_start)}</time>
                            {")"}
                        </>
                    )
                }
                EventuallyViolation::TestEnded => {
                    html!({ "(which never occurred)" })
                }
            };
            html!(
                <>
                    <span>
                        <span class="keyword">{"eventually "}</span>
                        {render_formula(subformula)}
                    </span>
                    <span>{reason_html}</span>
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
                    <span>
                        {"as of "}
                        <time>{format_time(start, test_start)}</time>
                        {", it should always be the case that:"}
                    </span>
                    {render_formula(subformula)}
                    <span>
                        {"but at "}
                        <time>{format_time(time, test_start)}</time>
                        {":"}
                    </span>
                    {render_violation_inner(violation, test_start)}
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
                    <span>
                        {"as of "}
                        <time>{format_time(start, test_start)}</time>
                        {" and until "}
                        <time>{format_time(end, test_start)}</time>
                        {", it should always be the case that:"}
                    </span>
                    {render_formula(subformula)}
                    <span>
                        {"but at "}
                        <time>{format_time(time, test_start)}</time>
                        {":"}
                    </span>
                    {render_violation_inner(violation, test_start)}
                </>
            )
        }
        Violation::And { left, right } => {
            html!(
                <>
                    {render_violation_inner(left, test_start)}
                    <span class="keyword">{"and"}</span>
                    {render_violation_inner(right, test_start)}
                </>
            )
        }
        Violation::Or { left, right } => {
            html!(
                <>
                    {render_violation_inner(left, test_start)}
                    <span class="keyword">{"or"}</span>
                    {render_violation_inner(right, test_start)}
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
                        {
                            if !antecedent_snapshot_references.is_empty() {
                                html!(
                                    <>
                                        {render_snapshot_inline(
                                            antecedent_snapshot_references,
                                            JsonRenderOptions {
                                                literal_strings: false,
                                            },
                                        )}
                                        {", implying:"}
                                    </>
                                )
                            } else {
                                html!(
                                    <>
                                        {render_formula(left)}
                                        <span class="keyword">{" implies:"}</span>
                                    </>
                                )
                            }
                        }
                    </span>
                    {render_violation_inner(right, test_start)}
                </>
            )
        }
    }
}

fn render_snapshot_values(
    references: &[Snapshot],
    options: JsonRenderOptions,
) -> Html {
    let items = collect_snapshot_items(references, options);
    html!(
        <dl class="snapshot-values">
            { for items.into_iter() }
        </dl>
    )
}

fn render_snapshot_inline(
    references: &[Snapshot],
    options: JsonRenderOptions,
) -> Html {
    let items = collect_snapshot_items(references, options);
    if items.is_empty() {
        return html!();
    }
    html!(
        <span class="snapshot-inline">
            <dl class="snapshot-values inline">
                { for items.into_iter() }
            </dl>
        </span>
    )
}

fn collect_snapshot_items(
    references: &[Snapshot],
    options: JsonRenderOptions,
) -> Vec<Html> {
    let mut items = Vec::new();
    for (i, snapshot) in references.iter().enumerate() {
        let name = snapshot_name(snapshot, i);
        let class = if is_json_inline(&snapshot.value) {
            "json-entry inline"
        } else {
            "json-entry"
        };
        items.push(html!(
            <div class={class}>
                <dt>{name}</dt>
                <dd>{render_json(&snapshot.value, options)}</dd>
            </div>
        ));
    }
    items
}

fn snapshot_name(snapshot: &Snapshot, index: usize) -> String {
    snapshot
        .name
        .as_deref()
        .map(String::from)
        .unwrap_or_else(|| format!("extractor[{}]", index))
}

#[derive(Clone, Copy)]
struct JsonRenderOptions {
    literal_strings: bool,
}

fn is_printable(s: &str) -> bool {
    s.chars().all(|c| !c.is_control() || c == '\n' || c == '\t')
}

fn is_json_inline(value: &json::Value) -> bool {
    match value {
        json::Value::Array(items) => items.is_empty(),
        json::Value::Object(map) => map.is_empty(),
        _ => true,
    }
}

fn render_json(value: &json::Value, options: JsonRenderOptions) -> Html {
    match value {
        json::Value::Array(items) if items.is_empty() => {
            html!(<code class="json-literal">{"[]"}</code>)
        }
        json::Value::Array(items) => {
            html!(
                <ul class="json-array">
                    { for items.iter().map(|item| html!(<li>{render_json(item, options)}</li>)) }
                </ul>
            )
        }
        json::Value::Object(map) if map.is_empty() => {
            html!(<code class="json-literal">{"{}"}</code>)
        }
        json::Value::Object(map) => {
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
                                <dd>{render_json(val, options)}</dd>
                            </div>
                        )
                    }) }
                </dl>
            )
        }
        json::Value::String(s)
            if !options.literal_strings && is_printable(s) =>
        {
            html!(<span class="json-string">{s}</span>)
        }
        json::Value::String(s) => {
            let literal = json::Value::String(s.clone()).to_string();
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
                    <span class="keyword">{" and "}</span>
                    {render_formula(right)}
                </span>
            )
        }
        Formula::Or(left, right) => {
            html!(
                <span class="formula-or">
                    {render_formula(left)}
                    <span class="keyword">{" or "}</span>
                    {render_formula(right)}
                </span>
            )
        }
        Formula::Implies(left, right) => {
            html!(
                <span class="formula-implies">
                    {render_formula(left)}
                    <span class="keyword">{" implies "}</span>
                    {render_formula(right)}
                </span>
            )
        }
        Formula::Next(formula) => {
            html!(
                <span class="formula-next">
                    <span class="keyword">{"next "}</span>
                    {render_formula(formula)}
                </span>
            )
        }
        Formula::Always(formula, None) => {
            html!(
                <span class="formula-always">
                    <span class="keyword">{"always "}</span>
                    {render_formula(formula)}
                </span>
            )
        }
        Formula::Always(formula, Some(bound)) => {
            html!(
                <span class="formula-always">
                    <span class="keyword">{"always "}</span>
                    {render_formula(formula)}
                    <span class="keyword">{
                        format!(" within {}ms", bound.as_millis())
                    }</span>
                </span>
            )
        }
        Formula::Eventually(formula, None) => {
            html!(
                <span class="formula-eventually">
                    <span class="keyword">{"eventually "}</span>
                    {render_formula(formula)}
                </span>
            )
        }
        Formula::Eventually(formula, Some(bound)) => {
            html!(
                <span class="formula-eventually">
                    <span class="keyword">{"eventually "}</span>
                    {render_formula(formula)}
                    <span class="keyword">{
                        format!(" within {}ms", bound.as_millis())
                    }</span>
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
