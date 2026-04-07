use crate::markup::{Inline, Markup};
use crate::schema::Time;

pub fn markup_to_text(markup: &Markup, test_start: Time) -> String {
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
            output.push_str(code);
        }
        Markup::Snapshots(snapshots) => {
            for (index, snapshot) in snapshots.iter().enumerate() {
                if index > 0 {
                    output.push_str(", ");
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
        Markup::Comma => {
            // Commas are handled by Join logic, never rendered directly
        }
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
        serde_json::Value::Null => output.push_str("null"),
        serde_json::Value::Bool(b) => output.push_str(&b.to_string()),
        serde_json::Value::Number(n) => output.push_str(&n.to_string()),
        serde_json::Value::String(s) => {
            if is_simple_string(s) {
                output.push_str(s);
            } else {
                output.push_str(&serde_json::to_string(s).unwrap());
            }
        }
        serde_json::Value::Array(items) if items.is_empty() => {
            output.push_str("[]")
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
            output.push_str("{}")
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
        Inline::Code(code) => output.push_str(code),
        Inline::Time(time) => {
            output.push_str(&format_duration(*time, test_start));
        }
        Inline::Keyword(keyword) => output.push_str(keyword),
    }
}

fn format_duration(time: Time, test_start: Time) -> String {
    let micros_since_start =
        time.as_micros().saturating_sub(test_start.as_micros());
    let total_seconds = micros_since_start / 1_000_000;
    let millis = (micros_since_start % 1_000_000) / 1_000;
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;

    format!("{:02}:{:02}.{:03}", minutes, seconds, millis)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::schema::{
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
            std::time::SystemTime::UNIX_EPOCH
                + Duration::from_secs(seconds),
        )
    }

    fn render_violation(violation: &PropertyViolation) -> String {
        let markup = crate::markup::render_violation(violation);
        markup_to_text(&markup, test_start())
    }

    #[test]
    fn test_invariant_violation() {
        // always(() => count.current <= 5)
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
        // always((() => x > 10).implies(eventually(() => y == 20)))
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
        // always(errorMessage !== null implies eventually(errorMessage === null).within(5 seconds))
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
        // always(unchanged.or(increment).or(decrement))
        // where unchanged = now(() => next(() => counterValue.current === current))
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
        // always(notificationCount === notificationCount.at(start)).for(10 seconds)
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
                        condition: "notificationCount.current === notificationCount.at(start)"
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
        // Snapshot with nested objects and arrays
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
        // State machine property: always(eventually(ready) or always(disabled))
        // Models: "either we eventually become ready, or we stay disabled forever"
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
}
