use std::ops::{Index, IndexMut};

use crate::schema::{ActionKey, TraceEntry};
use serde::{Deserialize, Serialize};
use small_string::SmallString;

pub type TerminalTraceEntry = TraceEntry<TerminalAction, TerminalStateSummary>;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum TerminalAction {
    TypeText { text: String },
    Resize { size: TerminalSize },
    Click { row: u16, column: u16 },
    ScrollUp {},
    ScrollDown {},
}

/// Per-state metadata in a terminal trace. The screen contents (grid,
/// scrollback, modes) are in the ghostty snapshot stored next to it.
/// No `Eq`: the spec probabilities in `actions_available` are `f64`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TerminalStateSummary {
    pub scroll_offset: u32,
    pub cursor: TerminalCursor,
    pub exit_status: Option<ProcessExitStatus>,
    /// Keys of the action templates available in this state, with their
    /// spec probabilities (see `bombadil::model::StepModel`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actions_available: Option<Vec<(ActionKey, f64)>>,
    /// Key of the action template that led to this state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action_taken: Option<ActionKey>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProcessExitStatus {
    pub signal: Option<String>,
    pub code: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TerminalCursor {
    pub position: TerminalCursorPosition,
    pub visible: bool,
    pub blinking: bool,
    pub visual_style: TerminalCursorVisualStyle,
    pub color: TerminalColor,
}

#[derive(Debug, Copy, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TerminalCursorPosition {
    pub column: u16,
    pub row: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TerminalCursorVisualStyle {
    Bar,
    Block,
    Underline,
    BlockHollow,
    Unknown,
}

#[derive(Debug, Copy, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct TerminalSize<U16 = u16> {
    pub columns: U16,
    pub rows: U16,
}

impl TerminalSize {
    pub fn cell_count(&self) -> u32 {
        self.columns as u32 * self.rows as u32
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TerminalGrid {
    cells: Vec<TerminalCell>,
    pub size: TerminalSize,
}

impl TerminalGrid {
    pub fn with_size(size: TerminalSize) -> TerminalGrid {
        TerminalGrid {
            cells: vec![
                TerminalCell::Empty {
                    style: TerminalStyle::default()
                };
                size.cell_count() as usize
            ],
            size,
        }
    }

    pub fn from_cells(
        size: TerminalSize,
        cells: Vec<TerminalCell>,
    ) -> TerminalGrid {
        let expected = usize::from(size.columns) * usize::from(size.rows);
        assert!(
            cells.len() == expected,
            "cannot create grid of size ({}, {}) from {} cells",
            size.rows,
            size.columns,
            cells.len()
        );
        TerminalGrid { cells, size }
    }
}

impl Index<(u16, u16)> for TerminalGrid {
    type Output = TerminalCell;

    fn index(&self, (row, column): (u16, u16)) -> &Self::Output {
        assert!(
            row < self.size.rows && column < self.size.columns,
            "cannot index into ({}, {}) in grid of size ({}, {})",
            row,
            column,
            self.size.rows,
            self.size.columns
        );
        self.cells
            .get((row * self.size.columns + column) as usize)
            .expect("grid index out of bounds")
    }
}

impl IndexMut<(u16, u16)> for TerminalGrid {
    fn index_mut(&mut self, (row, column): (u16, u16)) -> &mut Self::Output {
        assert!(
            row < self.size.rows && column < self.size.columns,
            "cannot index_mut into ({}, {}) in grid of size ({}, {})",
            row,
            column,
            self.size.rows,
            self.size.columns
        );
        self.cells
            .get_mut((row * self.size.columns + column) as usize)
            .unwrap_or_else(|| {
                panic!(
                    "grid index {:?} out of bounds [0, {})",
                    (row, column),
                    self.size.cell_count()
                )
            })
    }
}

impl IntoIterator for TerminalGrid {
    type Item = TerminalCell;

    type IntoIter = std::vec::IntoIter<TerminalCell>;

    fn into_iter(self) -> Self::IntoIter {
        self.cells.into_iter()
    }
}

impl<'a> IntoIterator for &'a TerminalGrid {
    type Item = &'a TerminalCell;

    type IntoIter = std::slice::Iter<'a, TerminalCell>;

    fn into_iter(self) -> Self::IntoIter {
        self.cells.iter()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TerminalCell {
    Occupied {
        contents: SmallString,
        wide: bool,
        style: TerminalStyle,
    },
    Empty {
        style: TerminalStyle,
    },
    Continuation {
        style: TerminalStyle,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TerminalStyle {
    pub foreground_color: TerminalColor,
    pub background_color: TerminalColor,
    pub underline_color: TerminalColor,
    pub underline: TerminalUnderline,
    pub attributes: TerminalAttributes,
}

impl Default for TerminalStyle {
    fn default() -> TerminalStyle {
        TerminalStyle {
            foreground_color: TerminalColor::None,
            background_color: TerminalColor::None,
            underline_color: TerminalColor::None,
            underline: TerminalUnderline::None,
            attributes: TerminalAttributes::empty(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TerminalColor {
    None,
    Palette(u8),
    RGB { r: u8, g: u8, b: u8 },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ANSIColor {
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
    Default,
    BrightBlack,
    BrightRed,
    BrightGreen,
    BrightYellow,
    BrightBlue,
    BrightMagenta,
    BrightCyan,
    BrightWhite,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TerminalUnderline {
    None,
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TerminalAttributes(pub u8);

bitflags::bitflags! {
    impl TerminalAttributes: u8 {
        const BOLD          = 0b00000001;
        const ITALIC        = 0b00000010;
        const BLINK         = 0b00000100;
        const INVERSE       = 0b00001000;
        const STRIKETHROUGH = 0b00010000;
        const DIM           = 0b00100000;
        const INVISIBLE     = 0b01000000;
        const OVERLINE      = 0b10000000;
    }
}

#[cfg(test)]
mod tests {
    use super::{ActionKey, TerminalStateSummary};
    use hegel::{
        TestCase,
        generators::{floats, integers, optional, vecs},
        tuples,
    };
    use serde_json as json;

    /// A trace from before the step model was recorded has neither field.
    #[hegel::test]
    fn state_summary_step_model_is_optional(tc: TestCase) {
        let actions_available: Option<Vec<(ActionKey, f64)>> = tc
            .draw(optional(vecs(tuples!(
                integers::<u64>(),
                floats::<f64>().min_value(0.0).max_value(1.0)
            ))))
            .map(|actions| {
                actions
                    .into_iter()
                    .map(|(key, probability)| (ActionKey(key), probability))
                    .collect()
            });
        let action_taken = tc.draw(optional(integers::<u64>())).map(ActionKey);
        let mut written = json::json!({
            "scroll_offset": 0,
            "cursor": {
                "position": { "column": 0, "row": 0 },
                "visible": true,
                "blinking": false,
                "visual_style": "Block",
                "color": "None",
            },
            "exit_status": null,
        });
        if let Some(actions_available) = &actions_available {
            written["actions_available"] = json::json!(actions_available);
        }
        if let Some(action_taken) = &action_taken {
            written["action_taken"] = json::json!(action_taken);
        }
        let summary: TerminalStateSummary =
            json::from_value(written.clone()).unwrap();
        assert_eq!(summary.actions_available, actions_available);
        assert_eq!(summary.action_taken, action_taken);
        let rewritten = json::to_value(&summary).unwrap();
        for field in ["actions_available", "action_taken"] {
            assert_eq!(rewritten.get(field), written.get(field));
        }
    }
}
