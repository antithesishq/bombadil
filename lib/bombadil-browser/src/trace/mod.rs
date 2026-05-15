use std::{borrow::Cow, path::Path, time::SystemTime};

use bombadil_schema::Time;
use serde::Serialize;
use url::Url;

pub use bombadil::runner::PropertyViolation;

use crate::{
    browser::{actions::BrowserAction, state::Resources},
    convert::ToSchema,
};
use bombadil::specification::domain::Snapshot;

pub mod writer;

#[derive(Debug, Clone, Serialize)]
pub struct TraceEntry<'a> {
    pub timestamp: SystemTime,
    pub url: Cow<'a, Url>,
    pub hash_previous: Option<u64>,
    pub hash_current: Option<u64>,
    pub action: Option<Cow<'a, BrowserAction>>,
    pub screenshot: Cow<'a, Path>,
    pub snapshots: Cow<'a, [Snapshot]>,
    pub violations: Cow<'a, [PropertyViolation]>,
    pub resources: Cow<'a, Resources>,
}

impl<'a> ToSchema<bombadil_schema::BrowserTraceEntry> for TraceEntry<'a> {
    fn to_schema(&self) -> bombadil_schema::BrowserTraceEntry {
        bombadil_schema::TraceEntry {
            timestamp: Time::from_system_time(self.timestamp),
            action: self.action.as_ref().map(|a| a.to_schema()),
            state: bombadil_schema::BrowserStateSummary {
                url: self.url.to_string(),
                hash_previous: self.hash_previous,
                hash_current: self.hash_current,
                screenshot: self.screenshot.to_string_lossy().to_string(),
                resources: self.resources.to_schema(),
            },
            snapshots: self.snapshots.iter().map(|s| s.to_schema()).collect(),
            violations: self.violations.iter().map(|v| v.to_schema()).collect(),
        }
    }
}
