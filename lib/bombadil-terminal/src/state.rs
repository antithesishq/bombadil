use std::time::SystemTime;

use serde::Serialize;

use crate::{
    driver::{Size, TerminalAction},
    small_string::SmallString,
};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalState {
    #[serde(skip)]
    pub timestamp: SystemTime,
    pub size: Size,
    pub rows: Vec<String>,
    pub scrollback: Vec<String>,
    pub scroll_offset: u32,
    pub terminated: bool,
    pub last_action: Option<TerminalAction>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Grid {
    cells: Vec<Cell>,
    pub size: Size,
}

#[derive(Clone, Debug, Serialize)]
pub struct Cell {
    contents: SmallString,
}
