use std::{
    ops::{Index, IndexMut},
    time::{Duration, SystemTime},
};

use serde::{Deserialize, Serialize};
use small_string::SmallString;

/// Time represented as microseconds since UNIX_EPOCH.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Time(u64);

impl Serialize for Time {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Time {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        u64::deserialize(deserializer).map(Time)
    }
}

impl std::ops::Add<Duration> for Time {
    type Output = Self;
    fn add(self, rhs: Duration) -> Self {
        let duration_micros = rhs.as_micros() as u64;
        Time(self.0.wrapping_add(duration_micros))
    }
}

impl Time {
    pub fn from_system_time(time: SystemTime) -> Self {
        let micros = time
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros() as u64;
        Time(micros)
    }

    pub fn to_system_time(self) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_micros(self.0)
    }

    pub fn as_micros(self) -> u64 {
        self.0
    }

    pub fn checked_add(self, duration: Duration) -> Option<Self> {
        let duration_micros = duration.as_micros();
        if duration_micros > u64::MAX as u128 {
            return None;
        }
        self.0.checked_add(duration_micros as u64).map(Time)
    }

    pub fn duration_since(self, earlier: Time) -> Result<Duration, Duration> {
        if self.0 >= earlier.0 {
            Ok(Duration::from_micros(self.0 - earlier.0))
        } else {
            Err(Duration::from_micros(earlier.0 - self.0))
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TraceEntry<A, S> {
    pub timestamp: Time,
    pub action: Option<A>,
    pub state: S,
    pub snapshots: Vec<Snapshot>,
    pub violations: Vec<PropertyViolation>,
}

pub type BrowserTraceEntry = TraceEntry<BrowserAction, BrowserStateSummary>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BrowserStateSummary {
    pub url: String,
    pub hash_previous: Option<u64>,
    pub hash_current: Option<u64>,
    pub screenshot: String,
    pub resources: Resources,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Resources {
    pub js_heap_used: u64,
    pub js_heap_total: u64,
    pub dom_nodes: u64,
    pub documents: u64,
    pub js_event_listeners: u64,
    pub layout_objects: u64,
    pub timestamp: f64,
    pub thread_time: f64,
    pub task_duration: f64,
    pub script_duration: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum BrowserAction {
    Back,
    Forward,
    Click {
        name: String,
        content: Option<String>,
        point: Point,
    },
    DoubleClick {
        name: String,
        content: Option<String>,
        point: Point,
        delay_millis: u64,
    },
    TypeText {
        text: String,
        delay_millis: u64,
    },
    PressKey {
        code: u8,
    },
    ScrollUp {
        origin: Point,
        distance: f64,
    },
    ScrollDown {
        origin: Point,
        distance: f64,
    },
    Reload,
    Wait,
    SetFileInputFiles {
        selector: String,
        files: Vec<String>,
    },
    MouseDrag {
        from: Point,
        to: Point,
        steps: u8,
        delay_millis: u64,
    },
    SetViewport {
        width: u16,
        height: u16,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TerminalStateSummary {
    pub grid: TerminalGrid,
    pub scrollback: TerminalGrid,
    pub scroll_offset: u32,
    pub cursor: TerminalCursor,
    pub exit_status: Option<ProcessExitStatus>,
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

#[derive(Debug, Copy, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TerminalSize {
    pub columns: u16,
    pub rows: u16,
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
pub struct TerminalAttributes(u8);

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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Snapshot {
    pub index: usize,
    pub name: Option<String>,
    pub value: serde_json::Value,
    pub time: Time,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PropertyViolation {
    pub name: String,
    pub violation: Violation,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Violation {
    False {
        time: Time,
        condition: String,
        snapshots: Vec<Snapshot>,
    },
    Eventually {
        subformula: Box<Formula>,
        reason: EventuallyViolation,
    },
    Always {
        violation: Box<Violation>,
        subformula: Box<Formula>,
        start: Time,
        end: Option<Time>,
        time: Time,
    },
    And {
        left: Box<Violation>,
        right: Box<Violation>,
    },
    Or {
        left: Box<Violation>,
        right: Box<Violation>,
    },
    Implies {
        left: Formula,
        right: Box<Violation>,
        antecedent_snapshots: Vec<Snapshot>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum EventuallyViolation {
    TimedOut(Time),
    TestEnded,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Formula {
    Pure { value: bool, pretty: String },
    Thunk { function: String, negated: bool },
    And(Box<Formula>, Box<Formula>),
    Or(Box<Formula>, Box<Formula>),
    Implies(Box<Formula>, Box<Formula>),
    Next(Box<Formula>),
    Always(Box<Formula>, Option<Duration>),
    Eventually(Box<Formula>, Option<Duration>),
}
