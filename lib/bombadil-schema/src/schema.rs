use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TraceEntry {
    pub timestamp: SystemTime,
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
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PropertyViolation {
    pub name: String,
    pub violation: Violation,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Violation {
    False {
        time: SystemTime,
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
        start: SystemTime,
        end: Option<SystemTime>,
        time: SystemTime,
    },
    Until {
        left: Box<Formula>,
        right: Box<Formula>,
        bound: Option<Duration>,
        reason: UntilViolation,
    },
    Release {
        left: Box<Formula>,
        right: Box<Formula>,
        bound: Option<Duration>,
        violation: Box<Violation>,
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
    TimedOut(SystemTime),
    TestEnded,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum UntilViolation {
    Left(Box<Violation>),
    TimedOut(SystemTime),
    TestEnded,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Formula {
    Pure { value: bool, pretty: String },
    Thunk { function: String, negated: bool },
    And(Box<Formula>, Box<Formula>),
    Or(Box<Formula>, Box<Formula>),
    Implies(Box<Formula>, Box<Formula>),
    Until(Box<Formula>, Box<Formula>, Option<Duration>),
    Release(Box<Formula>, Box<Formula>, Option<Duration>),
    Next(Box<Formula>),
    Always(Box<Formula>, Option<Duration>),
    Eventually(Box<Formula>, Option<Duration>),
}
