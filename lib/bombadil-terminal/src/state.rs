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
}

impl RunState for TerminalState {
    fn timestamp(&self) -> bombadil_schema::Time {
        Time::from_system_time(self.timestamp)
    }
}
