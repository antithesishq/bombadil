use std::{borrow::Cow, path::Path, time::SystemTime};

use serde::Serialize;
use url::Url;

use crate::{
    browser::{actions::BrowserAction, state::Resources},
    specification::{ltl, render, verifier::Snapshot},
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
    pub violation: ltl::Violation<render::PrettyFunction>,
}
