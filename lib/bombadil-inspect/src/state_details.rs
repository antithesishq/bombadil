use std::rc::Rc;
use std::time::SystemTime;

use bombadil_inspect_api::TraceEntry;
use serde_json as json;
use yew::component;
use yew::prelude::*;

use crate::container_size::use_container_size;
use crate::render::{markup_to_html, render_violation};

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
                        .map(|violation| {
                            let markup = render_violation(violation, props.test_start);
                            html!(<li>
                                <div class="violation-entry">
                                    <div class="violation-name">{&violation.name}{":"}</div>
                                    {markup_to_html(&markup, props.test_start)}
                                </div>
                            </li>)
                        })
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
