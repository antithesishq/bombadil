use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

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
pub struct TraceEntry {
    pub timestamp: Time,
    pub url: String,
    pub hash_previous: Option<u64>,
    pub hash_current: Option<u64>,
    pub action: Option<BrowserAction>,
    pub screenshot: String,
    pub snapshots: Vec<Snapshot>,
    pub violations: Vec<PropertyViolation>,
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
