use crate::cookie::BrowserCookie;
use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;

use anyhow::anyhow;
use cdp_protocol::cdp::browser_protocol::emulation;

#[derive(Debug, Clone)]
pub struct Emulation {
    pub width: u16,
    pub height: u16,
    pub device_scale_factor: f64,
}

#[derive(Debug, Clone)]
pub struct BrowserOptions {
    pub emulation: Emulation,
    pub create_target: bool,
    pub instrumentation: crate::instrumentation::InstrumentationConfig,
    pub downloads_directory: PathBuf,
    pub grant_permissions: Vec<String>,
    pub extra_headers: HashMap<String, String>,
    pub cookies: Vec<BrowserCookie>,
    pub virtual_time_policy: Option<VirtualTimePolicy>,
}

#[derive(Debug, Clone)]
pub enum VirtualTimePolicy {
    Advance,
    Pause,
    PauseIfNetworkFetchesPending,
}

impl FromStr for VirtualTimePolicy {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "Advance" => Ok(VirtualTimePolicy::Advance),
            "Pause" => Ok(VirtualTimePolicy::Pause),
            "PauseIfNetworkFetchesPending" => {
                Ok(VirtualTimePolicy::PauseIfNetworkFetchesPending)
            }
            s => Err(anyhow!("invalid virtual time policy: {s}")),
        }
    }
}

impl From<&VirtualTimePolicy> for emulation::VirtualTimePolicy {
    fn from(value: &VirtualTimePolicy) -> Self {
        match value {
            VirtualTimePolicy::Advance => Self::Advance,
            VirtualTimePolicy::Pause => Self::Pause,
            VirtualTimePolicy::PauseIfNetworkFetchesPending => {
                Self::PauseIfNetworkFetchesPending
            }
        }
    }
}
