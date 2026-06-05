use std::sync::Arc;

use anyhow::{Result, ensure};
use bombadil::driver::FromGeneratedAction;
use bombadil_schema::{TerminalCell, TerminalGrid, TerminalSize};
use serde::{Deserialize, Serialize};
use serde_json as json;

use crate::{driver::TerminalAction, state::TerminalState};

impl FromGeneratedAction for TerminalAction {
    fn from_generated(value: json::Value) -> Result<Self> {
        let js_action: JsTerminalAction = json::from_value(value)?;
        js_action.try_into()
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JsTerminalState {
    pub grid: JsTerminalGrid,
    pub scrollback: JsTerminalGrid,
    pub scroll_offset: u32,
    pub terminated: bool,
    pub last_action: Option<JsTerminalAction>,
}

impl JsTerminalState {
    pub fn from_state(value: Arc<TerminalState>) -> Self {
        JsTerminalState {
            grid: JsTerminalGrid::from_grid(&value.grid),
            scrollback: JsTerminalGrid::from_grid(&value.scrollback),
            scroll_offset: value.scroll_offset,
            terminated: value.terminated,
            last_action: value.last_action.clone().map(Into::into),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JsTerminalGrid {
    rows: Vec<Vec<TerminalCell>>,
    size: JsTerminalSize,
}

impl JsTerminalGrid {
    pub fn from_grid(value: &TerminalGrid) -> Self {
        let mut rows = Vec::with_capacity(value.size.rows as usize);
        for row_index in 0..value.size.rows {
            let mut row = Vec::with_capacity(value.size.columns as usize);
            for column_index in 0..value.size.columns {
                row.push(value[(row_index, column_index)].clone())
            }
            rows.push(row);
        }

        JsTerminalGrid {
            rows,
            size: value.size.into(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum JsTerminalAction {
    #[serde(rename_all = "camelCase")]
    TypeText {
        text: String,
    },
    #[serde(rename_all = "camelCase")]
    PressKey {
        code: f64,
    },
    #[serde(rename_all = "camelCase")]
    Resize {
        size: JsTerminalSize,
    },
    ScrollUp {},
    ScrollDown {},
}

impl TryFrom<JsTerminalAction> for TerminalAction {
    type Error = anyhow::Error;
    fn try_from(value: JsTerminalAction) -> Result<Self> {
        match value {
            JsTerminalAction::TypeText { text } => {
                Ok(TerminalAction::TypeText { text })
            }
            JsTerminalAction::PressKey { code } => {
                ensure!(code.is_normal(), "key code must be a normal number");
                Ok(TerminalAction::PressKey { code: code as u32 })
            }
            JsTerminalAction::Resize { size } => Ok(TerminalAction::Resize {
                size: size.try_into()?,
            }),
            JsTerminalAction::ScrollUp {} => Ok(TerminalAction::ScrollUp {}),
            JsTerminalAction::ScrollDown {} => {
                Ok(TerminalAction::ScrollDown {})
            }
        }
    }
}

impl From<TerminalAction> for JsTerminalAction {
    fn from(value: TerminalAction) -> Self {
        match value {
            TerminalAction::TypeText { text } => {
                JsTerminalAction::TypeText { text }
            }
            TerminalAction::PressKey { code } => {
                JsTerminalAction::PressKey { code: code as f64 }
            }
            TerminalAction::Resize { size } => {
                JsTerminalAction::Resize { size: size.into() }
            }
            TerminalAction::ScrollUp {} => todo!(),
            TerminalAction::ScrollDown {} => todo!(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JsTerminalSize {
    rows: f64,
    columns: f64,
}

impl From<TerminalSize> for JsTerminalSize {
    fn from(value: TerminalSize) -> Self {
        JsTerminalSize {
            rows: value.rows as f64,
            columns: value.columns as f64,
        }
    }
}

impl TryFrom<JsTerminalSize> for TerminalSize {
    type Error = anyhow::Error;
    fn try_from(value: JsTerminalSize) -> Result<Self> {
        ensure!(value.rows.is_normal(), "rows must be a normal number");
        ensure!(value.columns.is_normal(), "columns must be a normal number");
        Ok(TerminalSize {
            rows: value.rows as u16,
            columns: value.columns as u16,
        })
    }
}
