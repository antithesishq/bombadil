use bombadil_schema::Time;
use bombadil_schema::markup::{Inline, Markup};
use yew::prelude::*;

use crate::duration::{FormatDurationOptions, format_duration};

pub use bombadil_schema::markup::render_violation;

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

pub fn markup_to_html(markup: &Markup, test_start: Time) -> Html {
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

fn inline_to_html(inline: &Inline, test_start: Time) -> Html {
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

fn format_time(time: &Time, test_start: Time) -> String {
    format_duration(
        time.duration_since(test_start)
            .expect("timestamp microsecond conversion failed"),
        FormatDurationOptions::default(),
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
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_by_key(|(key, _)| *key);
            html!(
                <dl class="json-object">
                    { for entries.into_iter().map(|(key, val)| {
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
