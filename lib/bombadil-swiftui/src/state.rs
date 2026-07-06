use std::time::SystemTime;

use bombadil_schema::swiftui::{ProcessExitStatus, SwiftUINode};
use serde::Serialize;

use crate::driver::SwiftUIAction;

#[derive(Clone, Debug, Serialize)]
pub struct SwiftUIState {
    pub timestamp: SystemTime,
    /// Accessibility tree reported by the agent; `None` once the app
    /// has exited (or before the first state arrives).
    pub root: Option<SwiftUINode>,
    pub exit_status: Option<ProcessExitStatus>,
    pub last_action: Option<SwiftUIAction>,
}

impl SwiftUIState {
    /// Total number of nodes in the tree, for progress reporting.
    pub fn node_count(&self) -> usize {
        self.root.as_ref().map_or(0, |root| root.iter().count())
    }
}
