use std::collections::VecDeque;

use bombadil_schema::Time;
use bombadil_schema::markup::{Layout, Markup, Node};
use owo_colors::OwoColorize;
use serde_json::Value;
use willow_tree::NodeId;

enum Frame {
    Snapshots {
        remaining_ids: VecDeque<NodeId>,
        child_results: Vec<(String, Layout)>,
    },
    Join {
        remaining_ids: VecDeque<NodeId>,
        output: String,
        layout_previous: Option<Layout>,
        comma_pending: bool,
        layout_all: Layout,
    },
}

pub fn markup_to_styled(tree: &Markup, test_start: Time) -> String {
    let mut stack: Vec<Frame> = Vec::new();

    let root = tree.root();
    match root.value() {
        Node::Snapshots => stack.push(Frame::Snapshots {
            remaining_ids: root.children().iter().cloned().collect(),
            child_results: Vec::new(),
        }),
        Node::Join => stack.push(Frame::Join {
            remaining_ids: root.children().iter().cloned().collect(),
            output: String::new(),
            layout_previous: None,
            comma_pending: false,
            layout_all: Layout::Inline,
        }),
        Node::Comma => panic!("root node cannot be a Comma"),
        leaf => {
            let (result, _) = build_leaf(leaf, test_start);
            return result;
        }
    }

    loop {
        let frame = stack.last_mut().expect("expected a frame on the stack");

        let next_id = match frame {
            Frame::Snapshots {
                remaining_ids: remaining,
                ..
            }
            | Frame::Join {
                remaining_ids: remaining,
                ..
            } => remaining.pop_front(),
        };

        match next_id {
            Some(id) => {
                let node = &tree[id];
                match node.value() {
                    Node::Comma => {
                        if let Frame::Join { comma_pending, .. } = frame {
                            *comma_pending = true;
                        }
                    }
                    Node::Join => {
                        if let Frame::Join {
                            remaining_ids: remaining,
                            ..
                        } = frame
                        {
                            let mut children: VecDeque<NodeId> =
                                node.children().iter().cloned().collect();
                            children.append(remaining);
                            *remaining = children;
                        }
                    }
                    Node::Snapshots => {
                        let children =
                            node.children().iter().cloned().collect();
                        stack.push(Frame::Snapshots {
                            remaining_ids: children,
                            child_results: Vec::new(),
                        });
                    }
                    leaf => {
                        let (result, inline) = build_leaf(leaf, test_start);
                        feed(frame, result, inline);
                    }
                }
            }
            None => {
                let frame_finished =
                    stack.pop().expect("no finished frame on stack");
                let (result, layout) = finalize(frame_finished);

                match stack.last_mut() {
                    Some(parent) => feed(parent, result, layout),
                    None => return result,
                }
            }
        }
    }
}

fn build_leaf(node: &Node, test_start: Time) -> (String, Layout) {
    match node {
        Node::Text(text) => (text.clone(), Layout::Inline),
        Node::Code(code) => (maybe_italic(code.to_string()), Layout::Inline),
        Node::Time(time) => {
            let elapsed = std::time::Duration::from_micros(
                time.as_micros().saturating_sub(test_start.as_micros()),
            );
            let formatted = bombadil_schema::duration::format_duration(
                elapsed,
                bombadil_schema::duration::FormatDurationOptions {
                    include_millis: true,
                },
            );
            (maybe_bold(formatted), Layout::Inline)
        }
        Node::Keyword(keyword) => (keyword.clone(), Layout::Inline),
        Node::CodeBlock(code) => {
            (maybe_italic(code.to_string()), Layout::Block)
        }
        Node::SnapshotMarkup { name, value } => {
            let mut s = String::new();
            s.push_str(name);
            s.push_str(" = ");
            render_json_value(&mut s, value, 0);
            (s, Layout::for_json(value))
        }
        Node::Comma | Node::Join | Node::Snapshots => {
            unreachable!("branch/comma nodes must be handled before build_leaf")
        }
    }
}

fn feed(frame: &mut Frame, result_new: String, layout: Layout) {
    match frame {
        Frame::Snapshots {
            child_results: built,
            ..
        } => built.push((result_new, layout)),
        Frame::Join {
            output,
            layout_previous,
            comma_pending,
            layout_all,
            ..
        } => {
            *layout_all = layout_all.join(layout);

            if let Some(layout_previous) = *layout_previous {
                let separator = match (layout_previous, layout) {
                    (Layout::Inline, Layout::Inline) => {
                        if *comma_pending {
                            ", "
                        } else {
                            " "
                        }
                    }
                    (Layout::Inline, Layout::Block) => ":\n\n",
                    (Layout::Block, _) => "\n\n",
                };
                output.push_str(separator);
                *comma_pending = false;
            }

            output.push_str(&result_new);
            *layout_previous = Some(layout);
        }
    }
}

fn finalize(frame: Frame) -> (String, Layout) {
    match frame {
        Frame::Snapshots { child_results, .. } => {
            let layout_all = child_results
                .iter()
                .fold(Layout::Inline, |acc, (_, layout)| acc.join(*layout));
            let mut result = String::new();
            for (index, (item, _)) in child_results.iter().enumerate() {
                if index > 0 {
                    result.push_str(if layout_all == Layout::Inline {
                        ", "
                    } else {
                        "\n"
                    });
                }
                result.push_str(item);
            }
            (result, layout_all)
        }
        Frame::Join {
            output, layout_all, ..
        } => (output, layout_all),
    }
}

fn render_json_value(output: &mut String, value: &Value, indent: usize) {
    enum Work<'a> {
        Value { value: &'a Value, indent: usize },
        Literal(String),
    }

    let mut stack = vec![Work::Value { value, indent }];

    while let Some(work) = stack.pop() {
        match work {
            Work::Literal(s) => output.push_str(&s),
            Work::Value { value, indent } => match value {
                Value::Null => output.push_str(&maybe_blue("null".to_string())),
                Value::Bool(b) => output.push_str(&maybe_blue(b.to_string())),
                Value::Number(n) => output.push_str(&maybe_blue(n.to_string())),
                Value::String(s) => {
                    if is_simple_string(s) {
                        output.push_str(&maybe_blue(s.to_string()));
                    } else {
                        output.push_str(&maybe_blue(
                            serde_json::to_string(s).expect(
                                "couldn't serialize JSON string as string",
                            ),
                        ));
                    }
                }
                Value::Array(items) if items.is_empty() => {
                    output.push_str(&maybe_blue("[]".to_string()))
                }
                Value::Array(items) => {
                    let indent_str = "  ".repeat(indent + 1);
                    for item in items.iter().rev() {
                        stack.push(Work::Value {
                            value: item,
                            indent: indent + 1,
                        });
                        stack.push(Work::Literal(format!("\n{indent_str}- ")));
                    }
                }
                Value::Object(map) if map.is_empty() => {
                    output.push_str(&maybe_blue("{}".to_string()))
                }
                Value::Object(map) => {
                    let mut entries: Vec<_> = map.iter().collect();
                    entries.sort_by_key(|(key, _)| *key);

                    let indent_str = "  ".repeat(indent + 1);
                    for (key, val) in entries.into_iter().rev() {
                        stack.push(Work::Value {
                            value: val,
                            indent: indent + 1,
                        });
                        stack.push(Work::Literal(format!(
                            "\n{indent_str}{key}: "
                        )));
                    }
                }
            },
        }
    }
}

fn is_simple_string(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    if s.chars().any(|c| c.is_control()) {
        return false;
    }
    match s {
        "true" | "false" | "True" | "False" | "TRUE" | "FALSE" | "yes"
        | "no" | "Yes" | "No" | "YES" | "NO" | "null" | "Null" | "NULL"
        | "~" => return false,
        _ => {}
    }
    let first = s.chars().next().expect("first char on empty string");
    if matches!(
        first,
        '[' | ']'
            | '{'
            | '}'
            | ','
            | '&'
            | '*'
            | '#'
            | '?'
            | '|'
            | '-'
            | '<'
            | '>'
            | '='
            | '!'
            | '%'
            | '@'
            | '`'
            | '\''
            | '"'
            | ':'
    ) {
        return false;
    }
    if s.contains(": ") {
        return false;
    }
    if s.contains(" #") {
        return false;
    }
    true
}

pub fn supports_color() -> bool {
    supports_color::on(supports_color::Stream::Stdout).is_some()
}

pub fn maybe_blue(s: String) -> String {
    if supports_color() {
        s.blue().to_string()
    } else {
        s
    }
}
pub fn maybe_bold(s: String) -> String {
    if supports_color() {
        s.bold().to_string()
    } else {
        s
    }
}
pub fn maybe_italic(s: String) -> String {
    if supports_color() {
        s.italic().to_string()
    } else {
        s
    }
}
pub fn maybe_dimmed(s: String) -> String {
    if supports_color() {
        s.dimmed().to_string()
    } else {
        s
    }
}
pub fn maybe_red(s: String) -> String {
    if supports_color() {
        s.red().to_string()
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bombadil_schema::{
        EventuallyViolation, Formula, PropertyViolation, Snapshot, Time,
        Violation,
    };

    use super::*;

    fn thunk(s: &str) -> Formula {
        Formula::Thunk {
            function: s.to_string(),
            negated: false,
        }
    }

    fn test_start() -> Time {
        Time::from_system_time(std::time::SystemTime::UNIX_EPOCH)
    }

    fn time_at(seconds: u64) -> Time {
        Time::from_system_time(
            std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(seconds),
        )
    }

    fn render_violation(violation: &PropertyViolation) -> String {
        let markup = bombadil_schema::markup::render_violation(violation);
        markup_to_styled(&markup, test_start())
    }

    #[test]
    fn test_invariant_violation() {
        let violation = PropertyViolation {
            name: "maxCount".to_string(),
            violation: Violation::Always {
                subformula: Box::new(thunk("count.current <= 5")),
                start: time_at(0),
                end: None,
                time: time_at(305),
                violation: Box::new(Violation::False {
                    time: time_at(305),
                    condition: "count.current <= 5".into(),
                    snapshots: vec![Snapshot {
                        index: 0,
                        name: Some("count".into()),
                        value: serde_json::json!(6),
                        time: time_at(305),
                    }],
                }),
            },
        };

        insta::assert_snapshot!(render_violation(&violation));
    }

    #[test]
    fn test_always_implies_eventually() {
        let violation = PropertyViolation {
            name: "implicationProperty".to_string(),
            violation: Violation::Always {
                subformula: Box::new(Formula::Implies(
                    Box::new(thunk("x > 10")),
                    Box::new(Formula::Eventually(
                        Box::new(thunk("y == 20")),
                        None,
                    )),
                )),
                start: time_at(60),
                end: None,
                time: time_at(120),
                violation: Box::new(Violation::Implies {
                    left: thunk("x > 10"),
                    right: Box::new(Violation::Eventually {
                        subformula: Box::new(thunk("y == 20")),
                        reason: EventuallyViolation::TestEnded,
                    }),
                    antecedent_snapshots: vec![Snapshot {
                        index: 0,
                        name: Some("x".into()),
                        value: serde_json::json!(11),
                        time: time_at(120),
                    }],
                }),
            },
        };

        insta::assert_snapshot!(render_violation(&violation));
    }

    #[test]
    fn test_bounded_eventually() {
        let violation = PropertyViolation {
            name: "errorDisappears".to_string(),
            violation: Violation::Always {
                subformula: Box::new(Formula::Implies(
                    Box::new(thunk("errorMessage !== null")),
                    Box::new(Formula::Eventually(
                        Box::new(thunk("errorMessage === null")),
                        Some(Duration::from_secs(5)),
                    )),
                )),
                start: time_at(0),
                end: None,
                time: time_at(60),
                violation: Box::new(Violation::Implies {
                    left: thunk("errorMessage !== null"),
                    right: Box::new(Violation::Eventually {
                        subformula: Box::new(thunk("errorMessage === null")),
                        reason: EventuallyViolation::TimedOut(time_at(65)),
                    }),
                    antecedent_snapshots: vec![Snapshot {
                        index: 0,
                        name: Some("errorMessage".into()),
                        value: serde_json::json!("Error: Failed to load"),
                        time: time_at(60),
                    }],
                }),
            },
        };

        insta::assert_snapshot!(render_violation(&violation));
    }

    #[test]
    fn test_next_violation() {
        let violation = PropertyViolation {
            name: "counterStateMachine".to_string(),
            violation: Violation::Always {
                subformula: Box::new(Formula::Or(
                    Box::new(Formula::Or(
                        Box::new(Formula::Next(Box::new(thunk(
                            "counterValue.current === 5",
                        )))),
                        Box::new(Formula::Next(Box::new(thunk(
                            "counterValue.current === 6",
                        )))),
                    )),
                    Box::new(Formula::Next(Box::new(thunk(
                        "counterValue.current === 4",
                    )))),
                )),
                start: time_at(0),
                end: None,
                time: time_at(30),
                violation: Box::new(Violation::Or {
                    left: Box::new(Violation::Or {
                        left: Box::new(Violation::False {
                            time: time_at(31),
                            condition: "counterValue.current === 5".into(),
                            snapshots: vec![Snapshot {
                                index: 0,
                                name: Some("counterValue".into()),
                                value: serde_json::json!(10),
                                time: time_at(31),
                            }],
                        }),
                        right: Box::new(Violation::False {
                            time: time_at(31),
                            condition: "counterValue.current === 6".into(),
                            snapshots: vec![],
                        }),
                    }),
                    right: Box::new(Violation::False {
                        time: time_at(31),
                        condition: "counterValue.current === 4".into(),
                        snapshots: vec![],
                    }),
                }),
            },
        };

        insta::assert_snapshot!(render_violation(&violation));
    }

    #[test]
    fn test_bounded_always() {
        let violation = PropertyViolation {
            name: "constantNotificationCount".to_string(),
            violation: Violation::Always {
                subformula: Box::new(Formula::Always(
                    Box::new(thunk("notificationCount.current === initial")),
                    Some(Duration::from_secs(10)),
                )),
                start: time_at(0),
                end: None,
                time: time_at(120),
                violation: Box::new(Violation::Always {
                    subformula: Box::new(thunk(
                        "notificationCount.current === initial",
                    )),
                    start: time_at(120),
                    end: Some(time_at(130)),
                    time: time_at(125),
                    violation: Box::new(Violation::False {
                        time: time_at(125),
                        condition: "notificationCount.current === initial"
                            .into(),
                        snapshots: vec![Snapshot {
                            index: 0,
                            name: Some("notificationCount".into()),
                            value: serde_json::json!(3),
                            time: time_at(125),
                        }],
                    }),
                }),
            },
        };

        insta::assert_snapshot!(render_violation(&violation));
    }

    #[test]
    fn test_complex_snapshots() {
        let violation = PropertyViolation {
            name: "userDataValid".to_string(),
            violation: Violation::Always {
                subformula: Box::new(thunk("user.isValid()")),
                start: time_at(0),
                end: None,
                time: time_at(60),
                violation: Box::new(Violation::False {
                    time: time_at(60),
                    condition: "user.isValid()".into(),
                    snapshots: vec![Snapshot {
                        index: 0,
                        name: Some("user".into()),
                        value: serde_json::json!({
                            "name": "Alice",
                            "age": 30,
                            "tags": ["premium", "verified"],
                            "address": {
                                "city": "San Francisco",
                                "zip": "94102"
                            }
                        }),
                        time: time_at(60),
                    }],
                }),
            },
        };

        insta::assert_snapshot!(render_violation(&violation));
    }

    #[test]
    fn test_or_with_temporal_operators() {
        let violation = PropertyViolation {
            name: "stateMachine".to_string(),
            violation: Violation::Always {
                subformula: Box::new(Formula::Or(
                    Box::new(Formula::Eventually(
                        Box::new(thunk("state === 'ready'")),
                        Some(Duration::from_secs(30)),
                    )),
                    Box::new(Formula::Always(
                        Box::new(thunk("state === 'disabled'")),
                        None,
                    )),
                )),
                start: time_at(0),
                end: None,
                time: time_at(10),
                violation: Box::new(Violation::Or {
                    left: Box::new(Violation::Eventually {
                        subformula: Box::new(thunk("state === 'ready'")),
                        reason: EventuallyViolation::TimedOut(time_at(40)),
                    }),
                    right: Box::new(Violation::Always {
                        subformula: Box::new(thunk("state === 'disabled'")),
                        start: time_at(10),
                        end: None,
                        time: time_at(15),
                        violation: Box::new(Violation::False {
                            time: time_at(15),
                            condition: "state === 'disabled'".into(),
                            snapshots: vec![Snapshot {
                                index: 0,
                                name: Some("state".into()),
                                value: serde_json::json!("pending"),
                                time: time_at(15),
                            }],
                        }),
                    }),
                }),
            },
        };

        insta::assert_snapshot!(render_violation(&violation));
    }

    #[test]
    fn test_snapshot_separator_logic() {
        let all_inline = PropertyViolation {
            name: "allInlineSnapshots".to_string(),
            violation: Violation::Always {
                subformula: Box::new(thunk("condition")),
                start: time_at(0),
                end: None,
                time: time_at(10),
                violation: Box::new(Violation::False {
                    time: time_at(10),
                    condition: "condition".into(),
                    snapshots: vec![
                        Snapshot {
                            index: 0,
                            name: Some("foo".into()),
                            value: serde_json::json!(1),
                            time: time_at(10),
                        },
                        Snapshot {
                            index: 1,
                            name: Some("bar".into()),
                            value: serde_json::json!(2),
                            time: time_at(10),
                        },
                        Snapshot {
                            index: 2,
                            name: Some("baz".into()),
                            value: serde_json::json!("test"),
                            time: time_at(10),
                        },
                    ],
                }),
            },
        };

        let mixed = PropertyViolation {
            name: "mixedSnapshots".to_string(),
            violation: Violation::Always {
                subformula: Box::new(thunk("condition")),
                start: time_at(0),
                end: None,
                time: time_at(20),
                violation: Box::new(Violation::False {
                    time: time_at(20),
                    condition: "condition".into(),
                    snapshots: vec![
                        Snapshot {
                            index: 0,
                            name: Some("selectedFilter".into()),
                            value: serde_json::json!("Active"),
                            time: time_at(20),
                        },
                        Snapshot {
                            index: 1,
                            name: Some("newTodoInput".into()),
                            value: serde_json::json!({
                                "active": false,
                                "pendingText": "b",
                                "rect": {}
                            }),
                            time: time_at(20),
                        },
                        Snapshot {
                            index: 2,
                            name: Some("availableFilters".into()),
                            value: serde_json::json!([
                                "All",
                                "Active",
                                "Completed"
                            ]),
                            time: time_at(20),
                        },
                    ],
                }),
            },
        };

        insta::assert_snapshot!("all_inline", render_violation(&all_inline));
        insta::assert_snapshot!("mixed", render_violation(&mixed));
    }
}
