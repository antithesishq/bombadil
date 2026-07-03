use std::time::Duration;

use crate::schema::{
    EventuallyViolation, Formula, PropertyViolation, Snapshot, Time, Violation,
};
use serde_json::Value;
use willow_tree::{NodeId, Tree};

#[derive(Debug, Clone)]
pub enum Node {
    // Leaves
    Text(String),
    Code(String),
    Time(Time),
    Keyword(String),
    CodeBlock(String),
    SnapshotMarkup {
        name: String,
        value: serde_json::Value,
    },
    Comma,

    // Branches
    Snapshots,
    Join,
}

pub type Markup = Tree<Node>;

pub fn render_violation(violation: &PropertyViolation) -> Markup {
    let mut tree = Tree::new(Node::Join);

    enum Work<'a> {
        Violation {
            violation: &'a Violation,
            time: Time,
            parent_id: NodeId,
        },
        Node {
            node: Node,
            parent_id: NodeId,
        },
    }

    let mut stack = vec![Work::Violation {
        violation: &violation.violation,
        time: get_violation_time(&violation.violation),
        parent_id: tree.root_id(),
    }];

    while let Some(work) = stack.pop() {
        match work {
            Work::Violation {
                violation,
                time,
                parent_id,
            } => {
                match violation {
                    Violation::False {
                        snapshots,
                        condition,
                        ..
                    } => {
                        if snapshots.is_empty() {
                            render_code(
                                &mut tree,
                                parent_id,
                                format!("!({condition})"),
                            );
                        } else {
                            render_snapshots(
                                &mut tree, parent_id, snapshots, time,
                            );
                        }
                    }
                    Violation::Eventually { subformula, reason } => {
                        match reason {
                            EventuallyViolation::TimedOut(time) => {
                                let join_id =
                                    tree.insert(Node::Join, parent_id);
                                render_formula(&mut tree, join_id, subformula);
                                tree.insert(
                                    Node::Text("was never true before".into()),
                                    join_id,
                                );
                                tree.insert(Node::Time(*time), join_id);
                            }
                            EventuallyViolation::TestEnded => {
                                let join_id =
                                    tree.insert(Node::Join, parent_id);
                                render_formula(&mut tree, join_id, subformula);
                                tree.insert(
                                    Node::Keyword("was never true".into()),
                                    join_id,
                                );
                            }
                        }
                    }
                    Violation::Always {
                        violation,
                        subformula,
                        start,
                        end: None,
                        time,
                    } => {
                        let join_id = tree.insert(Node::Join, parent_id);
                        tree.insert(Node::Text("as of".into()), join_id);
                        tree.insert(Node::Time(*start), join_id);
                        tree.insert(Node::Comma, join_id);
                        tree.insert(
                            Node::Text(
                                "it should always be the case that".into(),
                            ),
                            join_id,
                        );
                        render_formula(&mut tree, join_id, subformula);
                        tree.insert(Node::Comma, join_id);
                        tree.insert(Node::Text("however".into()), join_id);
                        stack.push(Work::Violation {
                            violation,
                            time: *time,
                            parent_id: join_id,
                        });
                    }
                    // TODO: collapse this case with the one above?
                    Violation::Always {
                        violation,
                        subformula,
                        start,
                        end: Some(end),
                        time,
                    } => {
                        let join_id = tree.insert(Node::Join, parent_id);
                        tree.insert(Node::Text("as of".into()), join_id);
                        tree.insert(Node::Time(*start), join_id);
                        tree.insert(Node::Text("and until".into()), join_id);
                        tree.insert(Node::Time(*end), join_id);
                        tree.insert(Node::Comma, join_id);
                        tree.insert(
                            Node::Text(
                                "it should always be the case that".into(),
                            ),
                            join_id,
                        );
                        render_formula(&mut tree, join_id, subformula);
                        tree.insert(Node::Comma, join_id);
                        tree.insert(Node::Text("however".into()), join_id);
                        stack.push(Work::Violation {
                            violation,
                            time: *time,
                            parent_id: join_id,
                        });
                    }
                    Violation::And { left, right } => {
                        let join_id = tree.insert(Node::Join, parent_id);
                        stack.push(Work::Violation {
                            violation: right,
                            time,
                            parent_id: join_id,
                        });
                        stack.push(Work::Node {
                            node: Node::Keyword("and".into()),
                            parent_id: join_id,
                        });
                        stack.push(Work::Violation {
                            violation: left,
                            time,
                            parent_id: join_id,
                        });
                    }
                    Violation::Or { left, right } => {
                        let join_id = tree.insert(Node::Join, parent_id);
                        stack.push(Work::Violation {
                            violation: right,
                            time,
                            parent_id: join_id,
                        });
                        stack.push(Work::Node {
                            node: Node::Keyword("and".into()),
                            parent_id: join_id,
                        });
                        stack.push(Work::Violation {
                            violation: left,
                            time,
                            parent_id: join_id,
                        });
                    }
                    Violation::Implies {
                        left,
                        right,
                        antecedent_snapshots,
                    } => {
                        let join_id = tree.insert(Node::Join, parent_id);
                        // Use the consequent's time as "current" for grouping snapshots
                        let implies_time = get_violation_time(right);
                        if !antecedent_snapshots.is_empty() {
                            render_snapshots(
                                &mut tree,
                                join_id,
                                antecedent_snapshots,
                                implies_time,
                            );
                            tree.insert(Node::Comma, join_id);
                            tree.insert(
                                Node::Text(
                                    "failing the implication because".into(),
                                ),
                                join_id,
                            );

                            stack.push(Work::Violation {
                                violation: right,
                                time: implies_time,
                                parent_id: join_id,
                            });
                        } else {
                            render_formula(&mut tree, join_id, left);
                            tree.insert(
                                Node::Keyword("implies".into()),
                                join_id,
                            );
                            stack.push(Work::Violation {
                                violation: right,
                                time: implies_time,
                                parent_id: join_id,
                            });
                        }
                    }
                }
            }
            Work::Node { node, parent_id } => {
                tree.insert(node, parent_id);
            }
        }
    }

    tree
}

fn get_violation_time(violation: &Violation) -> Time {
    let mut current = violation;
    loop {
        match current {
            Violation::False { time, .. } => return *time,
            Violation::Always { time, .. } => return *time,
            Violation::Eventually { reason, .. } => {
                return match reason {
                    EventuallyViolation::TimedOut(time) => *time,
                    EventuallyViolation::TestEnded => Time::from_system_time(
                        std::time::SystemTime::UNIX_EPOCH,
                    ),
                };
            }
            Violation::Implies { right, .. } => {
                current = right.as_ref();
            }
            Violation::And { left, .. } => {
                current = left.as_ref();
            }
            Violation::Or { left, .. } => {
                current = left.as_ref();
            }
        }
    }
}

fn render_code(tree: &mut Markup, parent_id: NodeId, code: String) {
    let node = if code.contains("\n") {
        Node::CodeBlock(code)
    } else {
        Node::Code(code)
    };
    tree.insert(node, parent_id);
}

fn render_snapshots(
    tree: &mut Markup,
    parent_id: NodeId,
    snapshots: &[Snapshot],
    current_time: Time,
) {
    use std::collections::BTreeMap;

    let (current, other): (Vec<_>, Vec<_>) =
        snapshots.iter().partition(|s| s.time == current_time);

    let mut by_time: BTreeMap<Time, Vec<&Snapshot>> = BTreeMap::new();
    for snapshot in &other {
        by_time.entry(snapshot.time).or_default().push(snapshot);
    }

    let join_id = tree.insert(Node::Join, parent_id);
    let mut has_snapshots = false;

    if !current.is_empty() {
        tree.insert(Node::Text("at".into()), join_id);
        tree.insert(Node::Time(current_time), join_id);
        tree.insert(Node::Comma, join_id);
        {
            let snapshots_id = tree.insert(Node::Snapshots, join_id);
            for snapshot in current {
                tree.insert(
                    Node::SnapshotMarkup {
                        name: snapshot_name(snapshot),
                        value: snapshot.value.clone(),
                    },
                    snapshots_id,
                );
            }
        }
        has_snapshots = true;
    }

    for (time, snapshots) in by_time.iter().rev() {
        if has_snapshots {
            tree.insert(Node::Comma, join_id);
            tree.insert(Node::Text("and".into()), join_id);
        }
        tree.insert(Node::Text("from the prior state at".into()), join_id);
        tree.insert(Node::Time(*time), join_id);
        tree.insert(Node::Comma, join_id);
        {
            let snapshots_id = tree.insert(Node::Snapshots, join_id);
            for snapshot in snapshots {
                tree.insert(
                    Node::SnapshotMarkup {
                        name: snapshot_name(snapshot),
                        value: snapshot.value.clone(),
                    },
                    snapshots_id,
                );
            }
        }
        has_snapshots = true;
    }
}

fn snapshot_name(snapshot: &Snapshot) -> String {
    snapshot
        .name
        .as_deref()
        .map(String::from)
        .unwrap_or_else(|| format!("extractors[{}]", snapshot.index))
}

pub fn format_bound(duration: Duration) -> String {
    let milliseconds = duration.as_millis();

    if milliseconds == 0 {
        return "0 milliseconds".to_string();
    }

    if milliseconds.is_multiple_of(60_000) {
        let minutes = milliseconds / 60_000;
        if minutes == 1 {
            "1 minute".to_string()
        } else {
            format!("{} minutes", minutes)
        }
    } else if milliseconds.is_multiple_of(1_000) {
        let seconds = milliseconds / 1_000;
        if seconds == 1 {
            "1 second".to_string()
        } else {
            format!("{} seconds", seconds)
        }
    } else if milliseconds == 1 {
        "1 millisecond".to_string()
    } else {
        format!("{} milliseconds", milliseconds)
    }
}

fn render_formula(tree: &mut Markup, parent_id: NodeId, formula: &Formula) {
    enum Work<'a> {
        Formula {
            formula: &'a Formula,
            parent_id: NodeId,
        },
        Node {
            node: Node,
            parent_id: NodeId,
        },
    }
    let mut stack = vec![Work::Formula { formula, parent_id }];

    while let Some(work) = stack.pop() {
        match work {
            Work::Formula { formula, parent_id } => match formula {
                Formula::Pure { value: _, pretty } => {
                    render_code(tree, parent_id, pretty.clone());
                }
                Formula::Thunk {
                    function,
                    negated: true,
                } => {
                    render_code(tree, parent_id, format!("not({})", function));
                }
                Formula::Thunk {
                    function,
                    negated: false,
                } => {
                    render_code(tree, parent_id, function.clone());
                }
                Formula::And(left, right) => {
                    let join_id = tree.insert(Node::Join, parent_id);
                    stack.push(Work::Formula {
                        formula: right,
                        parent_id: join_id,
                    });
                    stack.push(Work::Node {
                        node: Node::Keyword("and".into()),
                        parent_id: join_id,
                    });
                    stack.push(Work::Formula {
                        formula: left,
                        parent_id: join_id,
                    });
                }
                Formula::Or(left, right) => {
                    let join_id = tree.insert(Node::Join, parent_id);
                    stack.push(Work::Formula {
                        formula: right,
                        parent_id: join_id,
                    });
                    stack.push(Work::Node {
                        node: Node::Keyword("or".into()),
                        parent_id: join_id,
                    });
                    stack.push(Work::Formula {
                        formula: left,
                        parent_id: join_id,
                    });
                }
                Formula::Implies(left, right) => {
                    let join_id = tree.insert(Node::Join, parent_id);
                    tree.insert(Node::Keyword("if".into()), join_id);
                    stack.push(Work::Formula {
                        formula: right,
                        parent_id: join_id,
                    });
                    stack.push(Work::Node {
                        node: Node::Keyword("then".into()),
                        parent_id: join_id,
                    });
                    stack.push(Work::Formula {
                        formula: left,
                        parent_id: join_id,
                    });
                }
                Formula::Next(formula) => {
                    let join_id = tree.insert(Node::Join, parent_id);
                    tree.insert(Node::Keyword("next".into()), join_id);
                    stack.push(Work::Formula {
                        formula,
                        parent_id: join_id,
                    });
                }
                Formula::Always(formula, None) => {
                    let join_id = tree.insert(Node::Join, parent_id);
                    tree.insert(Node::Keyword("always".into()), join_id);
                    stack.push(Work::Formula {
                        formula,
                        parent_id: join_id,
                    });
                }
                Formula::Always(formula, Some(bound)) => {
                    let join_id = tree.insert(Node::Join, parent_id);
                    tree.insert(
                        Node::Text(format!("for {}", format_bound(*bound))),
                        join_id,
                    );
                    stack.push(Work::Formula {
                        formula,
                        parent_id: join_id,
                    });
                }
                Formula::Eventually(formula, None) => {
                    let join_id = tree.insert(Node::Join, parent_id);
                    tree.insert(Node::Keyword("eventually".into()), join_id);
                    stack.push(Work::Formula {
                        formula,
                        parent_id: join_id,
                    });
                }
                Formula::Eventually(formula, Some(bound)) => {
                    let join_id = tree.insert(Node::Join, parent_id);
                    tree.insert(Node::Text("within".into()), join_id);
                    tree.insert(Node::Text(format_bound(*bound)), join_id);
                    stack.push(Work::Formula {
                        formula,
                        parent_id: join_id,
                    });
                }
            },
            Work::Node { node, parent_id } => {
                tree.insert(node, parent_id);
            }
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Layout {
    Inline,
    Block,
}

impl Layout {
    pub fn join(self, other: Layout) -> Layout {
        match (self, other) {
            (Layout::Inline, Layout::Inline) => Layout::Inline,
            _ => Layout::Block,
        }
    }

    pub fn for_json(value: &Value) -> Self {
        match value {
            Value::Array(items) if !items.is_empty() => Layout::Block,
            Value::Object(map) if !map.is_empty() => Layout::Block,
            _ => Layout::Inline,
        }
    }
}
