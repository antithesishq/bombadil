use std::time::SystemTime;

use crate::markup::{Inline, Markup};

pub fn markup_to_text(markup: &Markup, test_start: SystemTime) -> String {
    let mut output = String::new();
    render_markup(&mut output, markup, test_start);
    output
}

fn render_markup(output: &mut String, markup: &Markup, test_start: SystemTime) {
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
                output
                    .push_str(&serde_json::to_string(&snapshot.value).unwrap());
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

fn render_join(output: &mut String, items: &[Markup], test_start: SystemTime) {
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

fn render_inline(output: &mut String, inline: &Inline, test_start: SystemTime) {
    match inline {
        Inline::Text(text) => output.push_str(text),
        Inline::Code(code) => output.push_str(code),
        Inline::Time(time) => {
            output.push_str(&format_duration(*time, test_start));
        }
        Inline::Keyword(keyword) => output.push_str(keyword),
    }
}

fn format_duration(time: SystemTime, test_start: SystemTime) -> String {
    let duration = time
        .duration_since(test_start)
        .unwrap_or(std::time::Duration::ZERO);

    let total_seconds = duration.as_secs();
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;

    format!("{:02}:{:02}", minutes, seconds)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use crate::schema::{
        EventuallyViolation, Formula, PropertyViolation, Snapshot, Violation,
    };

    use super::*;

    fn thunk(s: &str) -> Formula {
        Formula::Thunk {
            function: s.to_string(),
            negated: false,
        }
    }

    const TEST_START: SystemTime = SystemTime::UNIX_EPOCH;

    fn time_at(seconds: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
    }

    fn render_violation(violation: &PropertyViolation) -> String {
        let markup = crate::markup::render_violation(violation);
        markup_to_text(&markup, TEST_START)
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
                            }],
                        }),
                    }),
                }),
            },
        };

        insta::assert_snapshot!(render_violation(&violation));
    }
}
