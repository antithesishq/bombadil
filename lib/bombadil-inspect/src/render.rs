use std::collections::VecDeque;

use bombadil_schema::Time;
use bombadil_schema::markup::{Layout, Markup, Node};
use serde_json::Value;
use willow_tree::NodeId;
use yew::prelude::*;

use crate::duration::{FormatDurationOptions, format_duration};

pub use bombadil_schema::markup::render_violation;

enum Frame {
    Snapshots {
        remaining_ids: VecDeque<NodeId>,
        child_results: Vec<(Html, Layout)>,
    },
    Join {
        remaining_ids: VecDeque<NodeId>,
        html_result: Vec<Html>,
        html_pending_inline: Vec<Html>,
        comma_pending: bool,
        layout_previous: Option<Layout>,
        layout_all: Layout,
    },
}

pub fn markup_to_html(tree: &Markup, test_start: Time) -> Html {
    let mut stack: Vec<Frame> = Vec::new();

    let root = tree.root();
    match root.value() {
        Node::Snapshots => stack.push(Frame::Snapshots {
            remaining_ids: root.children().iter().cloned().collect(),
            child_results: Vec::new(),
        }),
        Node::Join => stack.push(Frame::Join {
            remaining_ids: root.children().iter().cloned().collect(),
            html_result: Vec::new(),
            html_pending_inline: Vec::new(),
            comma_pending: false,
            layout_previous: None,
            layout_all: Layout::Inline,
        }),
        Node::Comma => panic!("root node cannot be a Comma"),
        leaf => {
            let (html, _) = build_leaf(leaf, test_start);
            return html;
        }
    }

    loop {
        let frame = stack.last_mut().expect("expected a frame on the stack");

        let next_id = match frame {
            Frame::Snapshots { remaining_ids, .. }
            | Frame::Join { remaining_ids, .. } => remaining_ids.pop_front(),
        };

        match next_id {
            Some(id) => {
                let node = &tree[id];

                match node.value() {
                    Node::Comma => {
                        if let Frame::Join { comma_pending, .. } = frame {
                            *comma_pending = true;
                        }
                        // Comma has no meaning inside Snapshots, ignore.
                    }
                    Node::Join => {
                        // Splice this Join's children in front of whatever's in the
                        // current frame's queue.
                        if let Frame::Join { remaining_ids, .. } = frame {
                            let mut children: VecDeque<NodeId> =
                                node.children().iter().cloned().collect();
                            children.append(remaining_ids);
                            *remaining_ids = children;
                        } else {
                            panic!(
                                "unexpected Join node under Snapshots frame",
                            );
                        }
                    }
                    Node::Snapshots => {
                        stack.push(Frame::Snapshots {
                            remaining_ids: node
                                .children()
                                .iter()
                                .cloned()
                                .collect(),
                            child_results: Vec::new(),
                        });
                    }
                    leaf => {
                        let (html, inline) = build_leaf(leaf, test_start);
                        feed(frame, html, inline);
                    }
                }
            }
            None => {
                let finished = stack.pop().unwrap();
                let (html, inline) = finalize(finished);

                match stack.last_mut() {
                    Some(parent) => feed(parent, html, inline),
                    None => return html,
                }
            }
        }
    }
}

fn build_leaf(node: &Node, test_start: Time) -> (Html, Layout) {
    match node {
        Node::Text(text) => (html!({ text }), Layout::Inline),
        Node::Code(code) => (html!(<code>{code}</code>), Layout::Inline),
        Node::Time(time) => (
            html!(<time>{format_time(time, test_start)}</time>),
            Layout::Inline,
        ),
        Node::Keyword(keyword) => (
            html!(<span class="keyword">{keyword}</span>),
            Layout::Inline,
        ),
        Node::CodeBlock(code) => {
            (html!(<pre><code>{code}</code></pre>), Layout::Block)
        }
        Node::SnapshotMarkup { name, value } => {
            let layout = Layout::for_json(value);
            let class = match layout {
                Layout::Inline => "json-entry inline",
                Layout::Block => "json-entry",
            };
            let html = html!(
                <div class={class}>
                    <dt>{name}</dt>
                    <dd>{render_json(value)}</dd>
                </div>
            );
            (html, layout)
        }
        Node::Comma | Node::Join | Node::Snapshots => {
            unreachable!("branch/comma nodes must be handled before build_leaf")
        }
    }
}

fn feed(frame: &mut Frame, html: Html, layout: Layout) {
    match frame {
        Frame::Snapshots { child_results, .. } => {
            child_results.push((html, layout))
        }
        Frame::Join {
            html_result,
            html_pending_inline,
            comma_pending,
            layout_previous,
            layout_all,
            ..
        } => {
            *layout_all = layout_all.join(layout);

            if let Some(layout_previous) = *layout_previous {
                match (layout_previous, layout) {
                    (Layout::Inline, Layout::Inline) => {
                        let separator = if *comma_pending { ", " } else { " " };
                        html_pending_inline.push(html!({ separator }));
                    }
                    (Layout::Inline, Layout::Block) => {
                        html_pending_inline.push(html!({ ":" }));
                        flush_pending(html_pending_inline, html_result);
                    }
                    (Layout::Block, Layout::Block) => {
                        if !html_result.is_empty() {
                            html_result.push(html!({ "\n\n" }));
                        }
                    }
                    (Layout::Block, Layout::Inline) => {}
                }
                *comma_pending = false;
            }

            *layout_previous = Some(layout);

            if layout == Layout::Inline {
                html_pending_inline.push(html);
            } else {
                html_result.push(html);
            }
        }
    }
}

fn finalize(frame: Frame) -> (Html, Layout) {
    match frame {
        Frame::Snapshots { child_results, .. } => {
            let layout_all = child_results
                .iter()
                .fold(Layout::Inline, |acc, (_, layout)| acc.join(*layout));
            let html = match layout_all {
                Layout::Inline => html!(
                    <span class="snapshot-inline">
                        <dl class="snapshot-values inline">
                            { for child_results.iter().map(|(html, _)| html.clone()) }
                        </dl>
                    </span>
                ),
                Layout::Block => html!(
                    <dl class="snapshot-values">
                        { for child_results.iter().map(|(html, _)| html.clone()) }
                    </dl>
                ),
            };
            (html, layout_all)
        }
        Frame::Join {
            html_result: mut result,
            html_pending_inline: mut pending,
            layout_all,
            ..
        } => {
            flush_pending(&mut pending, &mut result);
            (html!(<>{ for result }</>), layout_all)
        }
    }
}

fn flush_pending(pending: &mut Vec<Html>, result: &mut Vec<Html>) {
    if !pending.is_empty() {
        if !result.is_empty() {
            result.push(html!({ "\n\n" }));
        }
        result.push(html!(<p>{ for pending.drain(..) }</p>));
    }
}

fn format_time(time: &Time, test_start: Time) -> String {
    format_duration(
        time.duration_since(test_start)
            .expect("timestamp microsecond conversion failed"),
        FormatDurationOptions::default(),
    )
}

fn render_json(value: &Value) -> Html {
    match value {
        Value::Array(items) if items.is_empty() => {
            html!(<code class="json-literal">{"[]"}</code>)
        }
        Value::Array(items) => {
            html!(
                <ul class="json-array">
                    { for items.iter().map(|item| html!(<li>{render_json(item)}</li>)) }
                </ul>
            )
        }
        Value::Object(map) if map.is_empty() => {
            html!(<code class="json-literal">{"{}"}</code>)
        }
        Value::Object(map) => {
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_by_key(|(key, _)| *key);
            html!(
                <dl class="json-object">
                    { for entries.iter().map(|(key, value)| {
                        let class = match Layout::for_json(value) {
                            Layout::Inline => "json-entry inline",
                            Layout::Block => "json-entry",
                        };
                        html!(
                            <div class={class}>
                                <dt>{key}</dt>
                                <dd>{render_json(value)}</dd>
                            </div>
                        )
                    }) }
                </dl>
            )
        }
        Value::String(s) if is_printable(s) => {
            html!(<span class="json-string">{s}</span>)
        }
        Value::String(s) => {
            let literal = Value::String(s.clone()).to_string();
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
