use std::time::SystemTime;

use bombadil_schema::TerminalGrid;
use serde::Serialize;

use crate::driver::TerminalAction;

#[derive(Clone, Debug, Serialize)]
pub struct TerminalState {
    pub timestamp: SystemTime,
    pub grid: TerminalGrid,
    pub scrollback: TerminalGrid,
    pub scroll_offset: u32,
    pub terminated: bool,
    pub last_action: Option<TerminalAction>,
}
