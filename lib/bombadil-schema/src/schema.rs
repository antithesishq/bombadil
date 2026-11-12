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

/// Identifies an action template by a 64-bit hash, written as 16
/// lowercase hex digits (JSON numbers cannot hold 64 bits).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ActionKey(pub u64);

impl std::fmt::Display for ActionKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

impl std::str::FromStr for ActionKey {
    type Err = &'static str;

    fn from_str(digits: &str) -> Result<Self, Self::Err> {
        if digits.len() != 16
            || !digits
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        {
            return Err("an action key is 16 hex digits");
        }
        u64::from_str_radix(digits, 16)
            .map(ActionKey)
            .map_err(|_| "an action key is 16 hex digits")
    }
}

impl Serialize for ActionKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for ActionKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct Digits;

        impl serde::de::Visitor<'_> for Digits {
            type Value = ActionKey;

            fn expecting(
                &self,
                f: &mut std::fmt::Formatter<'_>,
            ) -> std::fmt::Result {
                f.write_str("16 hex digits")
            }

            fn visit_str<E: serde::de::Error>(
                self,
                digits: &str,
            ) -> Result<ActionKey, E> {
                digits.parse().map_err(E::custom)
            }
        }

        deserializer.deserialize_str(Digits)
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Snapshot {
    /// `u64` rather than `usize`, so a trace reads the same on 32-bit
    /// targets (the inspect page is wasm32).
    pub index: u64,
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

#[cfg(test)]
mod tests {
    use super::ActionKey;
    use hegel::{TestCase, generators::integers};
    use serde_json as json;

    #[hegel::test]
    fn action_key_round_trips_as_16_hex_digits(tc: TestCase) {
        let key = ActionKey(tc.draw(integers::<u64>()));
        let written = json::to_value(key).unwrap();
        assert_eq!(written, json::json!(format!("{:016x}", key.0)));
        assert_eq!(json::from_value::<ActionKey>(written).unwrap(), key);
    }

    #[hegel::test]
    fn action_key_of_another_length_is_rejected(tc: TestCase) {
        let digits = format!("{:032x}", tc.draw(integers::<u128>()));
        let length = tc.draw(integers::<usize>().max_value(31));
        let length = if length < 16 { length } else { length + 1 };
        assert!(digits[..length].parse::<ActionKey>().is_err());
    }

    #[hegel::test]
    fn action_key_with_another_character_is_rejected(tc: TestCase) {
        let others: Vec<u8> = (0..128u8)
            .filter(|byte| !matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
            .collect();
        let mut digits =
            format!("{:016x}", tc.draw(integers::<u64>())).into_bytes();
        let position = tc.draw(integers::<usize>().max_value(15));
        digits[position] =
            others[tc.draw(integers::<usize>().max_value(others.len() - 1))];
        let digits = String::from_utf8(digits).unwrap();
        assert!(digits.parse::<ActionKey>().is_err(), "{digits:?}");
    }
}
