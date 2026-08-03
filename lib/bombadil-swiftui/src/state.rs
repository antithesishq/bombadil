use std::time::SystemTime;

use bombadil_schema::swiftui::{ProcessExitStatus, SwiftUINode};

#[derive(Debug)]
pub struct SwiftUIState {
    pub timestamp: SystemTime,
    /// Accessibility tree reported by the agent; `None` once the app
    /// has exited.
    pub root: Option<SwiftUINode>,
    pub exit_status: Option<ProcessExitStatus>,
}

impl SwiftUIState {
    /// Total number of nodes in the tree, for progress reporting.
    pub fn node_count(&self) -> usize {
        self.root.as_ref().map_or(0, |root| root.iter().count())
    }
}
