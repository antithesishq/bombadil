use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceEntry {
    pub timestamp: SystemTime,
    pub url: String,
    pub hash_previous: Option<u64>,
    pub hash_current: Option<u64>,
    pub action: Option<BrowserAction>,
    pub screenshot: String,
    pub snapshots: Vec<Snapshot>,
    pub violations: Vec<PropertyViolation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub name: Option<String>,
    pub value: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PropertyViolation {
    pub name: String,
    pub violation: Violation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Violation {
    False {
        time: SystemTime,
        condition: String,
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
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum EventuallyViolation {
    TimedOut(SystemTime),
    TestEnded,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
