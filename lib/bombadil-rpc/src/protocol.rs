//! Messages of the external driver protocol, version
//! [`PROTOCOL_VERSION`]. Each message is one line of JSON on the driver's
//! stdin (from Bombadil) or stdout (from the driver). See `PROTOCOL.md`.

use serde::{Deserialize, Serialize};
use serde_json as json;

pub const PROTOCOL_VERSION: u32 = 0;

pub const STALL_TIMEOUT_DEFAULT_MS: u64 = 30_000;

/// Sent by the driver on its stdout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DriverMessage {
    /// First message of every session.
    Hello {
        protocol: u32,
        name: String,
        /// How long Bombadil waits for a `state` before considering the
        /// driver stalled. Defaults to [`STALL_TIMEOUT_DEFAULT_MS`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stall_timeout_ms: Option<u64>,
    },
    /// The app's state: once after `hello`, and once after each `apply`.
    State {
        state: json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        screenshot_png: Option<String>,
        /// `false` if the driver gave up waiting for the app to settle and
        /// reports the state as it was.
        #[serde(default = "settled_default", skip_serializing_if = "is_true")]
        settled: bool,
        /// Why the driver didn't perform the action it was sent, if it
        /// didn't. Only valid after an `apply`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        not_applied: Option<String>,
        /// When the driver read the state, in microseconds since the Unix
        /// epoch. Defaults to when Bombadil receives it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timestamp_us: Option<u64>,
    },
    /// The driver can't continue the session.
    Error { message: String },
}

fn settled_default() -> bool {
    true
}

fn is_true(value: &bool) -> bool {
    *value
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
