use std::rc::Rc;

use bombadil_schema::browser;
use bombadil_schema::{PropertyViolation, Snapshot, Time};
use gloo_timers::callback::Timeout;
use serde::Serialize;
use serde_json as json;
use wasm_bindgen_futures::{JsFuture, spawn_local};
use yew::component;
use yew::prelude::*;

use crate::container_size::use_container_size;
use crate::render::{markup_to_html, render_violation};

#[derive(Serialize)]
struct CopiedStateDetails<'a> {
    violations: &'a [PropertyViolation],
    snapshots: &'a [Snapshot],
}

#[derive(PartialEq, Properties)]
pub struct StateDetailsProps {
    pub entry: Rc<browser::BrowserTraceEntry>,
    pub test_start: Time,
}

#[component]
pub fn StateDetails(props: &StateDetailsProps) -> Html {
    let (container_ref, container_size) = use_container_size();
    let copy_status = use_state(|| None::<bool>);
    let copy_text = json::to_string_pretty(&CopiedStateDetails {
        violations: &props.entry.violations,
        snapshots: &props.entry.snapshots,
    })
    .expect("violations and snapshots should serialize");
    let copy_details = {
        let copy_status = copy_status.clone();
        Callback::from(move |_: MouseEvent| {
            let copy_status = copy_status.clone();
            let promise = web_sys::window()
                .expect("window should exist")
                .navigator()
                .clipboard()
                .write_text(&copy_text);
            spawn_local(async move {
                copy_status.set(Some(JsFuture::from(promise).await.is_ok()));
                let copy_status = copy_status.clone();
                Timeout::new(2_000, move || copy_status.set(None)).forget();
            });
        })
    };
    let copy_label = match *copy_status {
        None => "Copy violations and snapshots",
        Some(true) => "Copied violations and snapshots",
        Some(false) => "Failed to copy violations and snapshots",
    };

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
                {if props.entry.violations.is_empty() {
                    html!()
                } else {
                    html!(<button
                        type="button"
                        class="copy-details"
                        onclick={copy_details}
                        aria-label={copy_label}
                        aria-live="polite"
                        title={copy_label}
                    >
                        {if *copy_status == Some(true) {
                            html!(<svg viewBox="0 0 16 16" aria-hidden="true">
                                <path d="m3 8.5 3 3 7-7" />
                            </svg>)
                        } else {
                            html!(<svg viewBox="0 0 16 16" aria-hidden="true">
                                <rect x="5.5" y="5.5" width="8" height="8" rx="1" />
                                <path d="M10.5 5.5V3.5a1 1 0 0 0-1-1h-6a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2" />
                            </svg>)
                        }}
                    </button>)
                }}
                <ol>
                {
                    props
                        .entry
                        .violations
                        .iter()
                        .map(|violation| {
                            let markup = render_violation(violation);
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
