use bombadil_schema::Time;
use bombadil_schema::markup::{Inline, Markup};
use owo_colors::OwoColorize;

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

pub fn markup_to_styled(markup: &Markup, test_start: Time) -> String {
    let mut output = String::new();
    render_markup(&mut output, markup, test_start);
    output
}

fn render_markup(output: &mut String, markup: &Markup, test_start: Time) {
    match markup {
        Markup::Span(inlines) => {
            for inline in inlines {
                render_inline(output, inline, test_start);
            }
        }
        Markup::CodeBlock(code) => {
            output.push_str(&maybe_italic(code.to_string()));
        }
        Markup::Snapshots(snapshots) => {
            let all_inline =
                snapshots.iter().all(|item| is_json_inline(&item.value));
            for (index, snapshot) in snapshots.iter().enumerate() {
                if index > 0 {
                    let separator = if all_inline { ", " } else { "\n" };
                    output.push_str(separator);
                }
                output.push_str(&snapshot.name);
                output.push_str(" = ");
                render_json_value(output, &snapshot.value, 0);
            }
        }
        Markup::Stack(items) => {
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    output.push_str("\n\n");
                }
                render_markup(output, item, test_start);
            }
        }
        Markup::Join(items) => {
            render_join(output, items, test_start);
        }
        Markup::Comma => {}
    }
}

fn render_join(output: &mut String, items: &[Markup], test_start: Time) {
    let items = flatten_joins(items);

    let mut previous_non_comma_index: Option<usize> = None;
    let mut next_separator_has_comma = false;

    for (index, item) in items.iter().enumerate() {
        if matches!(item, Markup::Comma) {
            next_separator_has_comma = true;
            continue;
        }

        if let Some(previous_index) = previous_non_comma_index {
            let previous_inline = is_inline(&items[previous_index]);
            let current_inline = is_inline(item);

            let separator = match (previous_inline, current_inline) {
                (true, true) => {
                    if next_separator_has_comma {
                        ", "
                    } else {
                        " "
                    }
                }
                (true, false) => ":\n\n",
                (false, _) => "\n\n",
            };

            output.push_str(separator);
            next_separator_has_comma = false;
        }

        render_markup(output, item, test_start);
        previous_non_comma_index = Some(index);
    }
}

fn flatten_joins(items: &[Markup]) -> Vec<Markup> {
    let mut result = Vec::new();
    for item in items {
        if let Markup::Join(nested_items) = item {
            result.extend(flatten_joins(nested_items));
        } else {
            result.push(item.clone());
        }
    }
    result
}

fn is_json_inline(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Array(items) => items.is_empty(),
        serde_json::Value::Object(map) => map.is_empty(),
        _ => true,
    }
}

fn render_json_value(
    output: &mut String,
    value: &serde_json::Value,
    indent: usize,
) {
    match value {
        serde_json::Value::Null => {
            output.push_str(&maybe_blue("null".to_string()))
        }
        serde_json::Value::Bool(b) => {
            output.push_str(&maybe_blue(b.to_string()))
        }
        serde_json::Value::Number(n) => {
            output.push_str(&maybe_blue(n.to_string()))
        }
        serde_json::Value::String(s) => {
            if is_simple_string(s) {
                output.push_str(&maybe_blue(s.to_string()));
            } else {
                output.push_str(&maybe_blue(serde_json::to_string(s).unwrap()));
            }
        }
        serde_json::Value::Array(items) if items.is_empty() => {
            output.push_str(&maybe_blue("[]".to_string()))
        }
        serde_json::Value::Array(items) => {
            let indent_str = "  ".repeat(indent + 1);
            for item in items {
                output.push('\n');
                output.push_str(&indent_str);
                output.push_str("- ");
                render_json_value(output, item, indent + 1);
            }
        }
        serde_json::Value::Object(map) if map.is_empty() => {
            output.push_str(&maybe_blue("{}".to_string()))
        }
        serde_json::Value::Object(map) => {
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_by_key(|(key, _)| *key);

            let indent_str = "  ".repeat(indent + 1);
            for (key, val) in entries {
                output.push('\n');
                output.push_str(&indent_str);
                output.push_str(key);
                output.push_str(": ");
                render_json_value(output, val, indent + 1);
            }
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

    let first = s.chars().next().unwrap();
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

fn render_inline(output: &mut String, inline: &Inline, test_start: Time) {
    match inline {
        Inline::Text(text) => output.push_str(text),
        Inline::Code(code) => output.push_str(&maybe_italic(code.to_string())),
        Inline::Time(time) => {
            let elapsed = std::time::Duration::from_micros(
                time.as_micros().saturating_sub(test_start.as_micros()),
            );
            let formatted = bombadil_schema::duration::format_duration(
                elapsed,
                bombadil_schema::duration::FormatDurationOptions {
                    include_millis: true,
                },
            );
            output.push_str(&maybe_bold(formatted));
        }
        Inline::Keyword(keyword) => output.push_str(keyword),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bombadil_schema::{
        EventuallyViolation, Formula, PropertyViolation, Snapshot, Time,
        UntilViolation, Violation,
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
                    Box::new(thunk(
                        "notificationCount.current === notificationCount.at(start)",
                    )),
                    Some(Duration::from_secs(10)),
                )),
                start: time_at(0),
                end: None,
                time: time_at(120),
                violation: Box::new(Violation::Always {
                    subformula: Box::new(thunk(
                        "notificationCount.current === notificationCount.at(start)",
                    )),
                    start: time_at(120),
                    end: Some(time_at(130)),
                    time: time_at(125),
                    violation: Box::new(Violation::False {
                        time: time_at(125),
                        condition:
                            "notificationCount.current === notificationCount.at(start)"
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
    fn test_until_with_prior_and_current_snapshots() {
        let violation = PropertyViolation {
            name: "loadingUntilReady".to_string(),
            violation: Violation::Until {
                left: Box::new(thunk("loading === true")),
                right: Box::new(thunk("ready === true")),
                start: time_at(0),
                end: None,
                reason: UntilViolation::Left(Box::new(Violation::False {
                    time: time_at(120),
                    condition: "loading === true".into(),
                    snapshots: vec![
                        Snapshot {
                            index: 0,
                            name: Some("loading".into()),
                            value: serde_json::json!(false),
                            time: time_at(120),
                        },
                        Snapshot {
                            index: 0,
                            name: Some("loading".into()),
                            value: serde_json::json!(true),
                            time: time_at(60),
                        },
                    ],
                })),
            },
        };

        insta::assert_snapshot!(render_violation(&violation));
    }

    #[test]
    fn test_until_timed_out_with_snapshots() {
        let violation = PropertyViolation {
            name: "loadingUntilReady".to_string(),
            violation: Violation::Until {
                left: Box::new(thunk("loading === true")),
                right: Box::new(thunk("ready === true")),
                start: time_at(60),
                end: Some(time_at(65)),
                reason: UntilViolation::TimedOut {
                    time: time_at(65),
                    snapshots: vec![Snapshot {
                        index: 0,
                        name: Some("loading".into()),
                        value: serde_json::json!(true),
                        time: time_at(60),
                    }],
                },
            },
        };

        insta::assert_snapshot!(render_violation(&violation));
    }

    #[test]
    fn test_until_test_ended_with_snapshots() {
        let violation = PropertyViolation {
            name: "loadingUntilReady".to_string(),
            violation: Violation::Until {
                left: Box::new(thunk("loading === true")),
                right: Box::new(thunk("ready === true")),
                start: time_at(30),
                end: None,
                reason: UntilViolation::TestEnded {
                    snapshots: vec![Snapshot {
                        index: 0,
                        name: Some("loading".into()),
                        value: serde_json::json!(true),
                        time: time_at(120),
                    }],
                },
            },
        };

        insta::assert_snapshot!(render_violation(&violation));
    }

    #[test]
    fn test_release_with_prior_and_current_snapshots() {
        let violation = PropertyViolation {
            name: "readyReleasesLoading".to_string(),
            violation: Violation::Release {
                left: Box::new(thunk("ready === true")),
                right: Box::new(thunk("loading === true")),
                start: time_at(10),
                end: Some(time_at(20)),
                violation: Box::new(Violation::False {
                    time: time_at(15),
                    condition: "loading === true".into(),
                    snapshots: vec![
                        Snapshot {
                            index: 0,
                            name: Some("loading".into()),
                            value: serde_json::json!(false),
                            time: time_at(15),
                        },
                        Snapshot {
                            index: 0,
                            name: Some("loading".into()),
                            value: serde_json::json!(true),
                            time: time_at(12),
                        },
                    ],
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
