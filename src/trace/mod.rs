use std::{path::PathBuf, time::SystemTime};

use serde::Serialize;
use url::Url;

use crate::{browser::actions::BrowserAction, specification::ltl};

pub mod writer;

#[derive(Debug, Clone, Serialize)]
pub struct TraceEntry {
    pub timestamp: SystemTime,
    pub url: Url,
    pub hash_previous: Option<u64>,
    pub hash_current: Option<u64>,
    pub action: Option<BrowserAction>,
    pub screenshot: PathBuf,
    pub violations: Vec<PropertyViolation>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PropertyViolation {
    pub name: String,
    pub violation: ltl::Violation<ltl::PrettyFunction>,
}
