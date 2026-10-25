//! Messages of the external driver protocol, version
//! [`PROTOCOL_VERSION`]. Each message is one line of JSON on the driver's
//! stdin (from Bombadil) or stdout (from the driver). See `PROTOCOL.md`.

use serde::{Deserialize, Serialize};
use serde_json as json;

pub const PROTOCOL_VERSION: u32 = 0;

/// Sent by the driver on its stdout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DriverMessage {
    /// First message of every session.
    Hello { protocol: u32, name: String },
    /// The app has settled into a new state: once after `hello`, and once
    /// after each `apply`.
    State { state: json::Value },
    /// The driver can't continue the session.
    Error { message: String },
}

/// Sent by Bombadil on the driver's stdin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BombadilMessage {
    /// Perform a concrete action, then report the resulting `state`.
    Apply { action: json::Value },
    /// End the session and exit.
    Terminate,
}
