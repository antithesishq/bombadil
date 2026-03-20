use std::rc::Rc;
use std::time::SystemTime;

use bombadil_inspect_api::EventuallyViolation;
use bombadil_inspect_api::Formula;
use bombadil_inspect_api::PropertyViolation;
use bombadil_inspect_api::TraceEntry;
use bombadil_inspect_api::Violation;
use serde_json as json;
use yew::component;
use yew::prelude::*;

use crate::container_size::use_container_size;
use crate::duration::format_duration;
use crate::svg::ViolationPattern;

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
                                <ViolationPattern />
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
            <details open={true}>
                <summary>{"Snapshots"}</summary>
                <table>
                {
                    props
                        .entry
                        .snapshots
                        .iter()
                        .map(|snapshot| html!(<tr><th>{snapshot.name.clone()}</th><td>{json::to_string_pretty(&snapshot.value).unwrap_or("invalid json".into())}</td></tr>))
                        .collect::<Html>()
                }
                </table>
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
            <strong>{&violation.name}{": "}</strong>
            {render_violation_inner(&violation.violation, test_start)}
        </div>
    )
}

fn render_violation_inner(
    violation: &Violation,
    test_start: SystemTime,
) -> Html {
    match violation {
        Violation::False { time: _, condition } => {
            html!(<pre><code>{format!("!({})", condition)}</code></pre>)
        }
        Violation::Eventually { subformula, reason } => {
            let reason_text = match reason {
                EventuallyViolation::TimedOut(time) => {
                    format!("timed out at {}: ", format_time(time, test_start))
                }
                EventuallyViolation::TestEnded => {
                    "failed at test end: ".to_string()
                }
            };
            html!(
                <div class="violation-eventually">
                    <span>{reason_text}</span>
                    {render_formula(subformula)}
                </div>
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
                <span class="violation-always">
                    <span>{
                        format!(
                            "as of {}, it should always \
                             be the case that",
                            format_time(start, test_start),
                        )
                    }</span>
                    {render_formula(subformula)}
                    <span>{
                        format!("but at {}", format_time(time, test_start))
                    }</span>
                    {render_violation_inner(violation, test_start)}
                </span>
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
                <span class="violation-always">
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
                        format!("but at {}", format_time(time, test_start))
                    }</span>
                    {render_violation_inner(violation, test_start)}
                </span>
            )
        }
        Violation::And { left, right } => {
            html!(
                <div class="violation-and">
                    {render_violation_inner(left, test_start)}
                    <span class="keyword">{"and"}</span>
                    {render_violation_inner(right, test_start)}
                </div>
            )
        }
        Violation::Or { left, right } => {
            html!(
                <div class="violation-or">
                    {render_violation_inner(left, test_start)}
                    <span class="keyword">{"or"}</span>
                    {render_violation_inner(right, test_start)}
                </div>
            )
        }
        Violation::Implies { left, right } => {
            html!(
                <div class="violation-implies">
                    {render_violation_inner(right, test_start)}
                    <span class="keyword">{"since"}</span>
                    {render_formula(left)}
                </div>
            )
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
