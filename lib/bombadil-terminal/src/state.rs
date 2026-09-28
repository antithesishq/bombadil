use std::sync::Arc;
use std::time::SystemTime;

use bombadil::driver::RunState;
use bombadil_schema::{
    Time,
    terminal::{ProcessExitStatus, TerminalCursor, TerminalGrid},
};
use serde::Serialize;

use crate::driver::TerminalAction;

#[derive(Clone, Debug, Serialize)]
pub struct TerminalState {
    pub timestamp: SystemTime,
    pub grid: TerminalGrid,
    pub scrollback: TerminalGrid,
    pub scroll_offset: u32,
    pub cursor: TerminalCursor,
    pub exit_status: Option<ProcessExitStatus>,
    pub last_action: Option<TerminalAction>,
    /// Ghostty's binary snapshot of the full terminal (screens, modes,
    /// scrollback, unfinished VT input), or `None` if encoding failed.
    /// Shared rather than cloned so handing the state to the trace
    /// writer does not copy it.
    #[serde(skip)]
    pub terminal_snapshot: Option<Arc<Vec<u8>>>,
}

impl RunState for TerminalState {
    fn timestamp(&self) -> bombadil_schema::Time {
        Time::from_system_time(self.timestamp)
    }
}
