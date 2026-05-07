use std::{borrow::Cow, path::Path, time::SystemTime};

use bombadil_schema::Time;
use serde::Serialize;
use url::Url;

use crate::{
    browser::{actions::BrowserAction, state::Resources},
    specification::{convert::ToSchema, domain::Snapshot},
};

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

#[derive(Debug, Clone, Serialize)]
pub struct PropertyViolation {
    pub name: String,
    pub violation: bombadil_schema::Violation,
}

impl ToSchema<bombadil_schema::PropertyViolation> for PropertyViolation {
    fn to_schema(&self) -> bombadil_schema::PropertyViolation {
        bombadil_schema::PropertyViolation {
            name: self.name.clone(),
            violation: self.violation.clone(),
        }
    }
}

impl<'a> ToSchema<bombadil_schema::TraceEntry> for TraceEntry<'a> {
    fn to_schema(&self) -> bombadil_schema::TraceEntry {
        bombadil_schema::TraceEntry {
            timestamp: Time::from_system_time(self.timestamp),
            url: self.url.to_string(),
            hash_previous: self.hash_previous,
            hash_current: self.hash_current,
            action: self.action.as_ref().map(|a| a.to_api()),
            screenshot: self.screenshot.to_string_lossy().to_string(),
            snapshots: self.snapshots.iter().map(|s| s.to_schema()).collect(),
            violations: self.violations.iter().map(|v| v.to_schema()).collect(),
            resources: self.resources.to_api(),
        }
    }
}
