use crate::schema::{Rect, TraceEntry};
use serde::{Deserialize, Serialize};

pub use crate::schema::ProcessExitStatus;

pub type SwiftUITraceEntry = TraceEntry<SwiftUIAction, SwiftUIStateSummary>;

/// A node in the accessibility tree reported by the in-app agent. The
/// root node represents the application; its children are windows.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SwiftUINode {
    /// Accessibility role, e.g. `Button`, `TextField`, `StaticText`.
    pub role: String,
    /// The `accessibilityIdentifier` set by the app, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identifier: Option<String>,
    /// Accessibility label (usually the visible text).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Accessibility value, rendered as a string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// Frame in screen coordinates (points, origin top-left).
    pub frame: Rect,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub selected: bool,
    #[serde(default)]
    pub focused: bool,
    #[serde(default)]
    pub children: Vec<SwiftUINode>,
}

fn default_true() -> bool {
    true
}

impl SwiftUINode {
    /// Iterate over this node and all descendants, depth-first.
    pub fn iter(&self) -> impl Iterator<Item = &SwiftUINode> {
        let mut stack = vec![self];
        std::iter::from_fn(move || {
            let node = stack.pop()?;
            stack.extend(node.children.iter().rev());
            Some(node)
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SwiftUIStateSummary {
    pub root: Option<SwiftUINode>,
    pub exit_status: Option<ProcessExitStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SwiftUIAction {
    Tap { x: f64, y: f64 },
    TypeText { text: String },
    PressKey { key: String },
    ScrollUp { x: f64, y: f64, distance: f64 },
    ScrollDown { x: f64, y: f64, distance: f64 },
}
