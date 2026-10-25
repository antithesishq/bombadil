use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use bombadil::driver::RunId;
use bombadil::{
    driver::{DriverEvent, InterfaceDriver, InterfaceSession},
    specification::domain::Snapshot,
};
use bombadil_schema::Time;
use serde::Deserialize;
use serde_json as json;
use url::Url;

use crate::browser::actions::BrowserAction;
use crate::browser::actions::BrowserActionTemplate;
use crate::browser::state::BrowserState;
use crate::browser::{Browser, BrowserEvent, NAVIGATION_TIMEOUT};
use crate::browser_options::BrowserOptions;
use crate::chromium;
use crate::chromium::Chromium;
use crate::instrumentation::{BlockRegistry, InstrumentationConfig};

/// Slots of the coverage module registered with Antithesis. A block reports
/// as the slot of its ID modulo this.
const COVERAGE_MAP_SIZE: usize = 64 * 1024;

pub enum DebuggerOptions {
    External {
        remote_debugger: Url,
    },
    Managed {
        launch_options: chromium::LaunchOptions,
    },
}

pub struct BrowserDriver {
    pub origin: Url,
    pub browser_options: BrowserOptions,
    pub debugger_options: DebuggerOptions,
    pub specification_bundle: Arc<str>,
    /// Shared by all sessions.
    pub blocks: BlockRegistry,
}

impl InterfaceDriver for BrowserDriver {
    type Session = BrowserSession;

    fn new_session(&self, _: RunId) -> Result<Self::Session> {
        let coverage = if self.browser_options.instrumentation
            == InstrumentationConfig::none()
        {
            None
        } else {
            // TODO: write coverage file with real data
            let coverage_map_offset = antithesis_fuzzer::init_coverage_module(
                COVERAGE_MAP_SIZE,
                "bombadil.tsv",
            );
            assert!(
                coverage_map_offset <= usize::MAX - COVERAGE_MAP_SIZE,
                "offset + slot overflows usize"
            );
            Some(BrowserSessionCoverage {
                reported: vec![false; COVERAGE_MAP_SIZE],
                coverage_map_offset,
            })
        };

        let chromium = match &self.debugger_options {
            DebuggerOptions::External { remote_debugger } => {
                Chromium::connect(remote_debugger)?
            }
            DebuggerOptions::Managed { launch_options } => {
                Chromium::launch(launch_options)?
            }
        };
        let mut browser = Browser::new(
            &self.origin,
            &self.browser_options,
            &chromium,
            &self.blocks,
        )?;
        browser.ensure_script_evaluated(&self.specification_bundle)?;

        browser.initiate()?;

        Ok(BrowserSession {
            _chromium: chromium,
            browser,
            specification_bundle: self.specification_bundle.clone(),
            coverage,
        })
    }
}

pub struct BrowserSessionCoverage {
    /// Per slot, whether it has been reported to Antithesis.
    reported: Vec<bool>,
    coverage_map_offset: usize,
}

pub struct BrowserSession {
    // We need to own this throughout the lifecycle, becausing
    // dropping a managed Chromium value terminates the spawned
    // browser process.
    _chromium: Chromium,
    coverage: Option<BrowserSessionCoverage>,
    browser: Browser,
    specification_bundle: Arc<str>,
}

impl InterfaceSession for BrowserSession {
    type Action = BrowserAction;
    type ActionTemplate = BrowserActionTemplate;
    type State = BrowserState;

    const STALL_TIMEOUT: Duration = NAVIGATION_TIMEOUT.saturating_mul(2);

    fn terminate(&mut self) -> Result<()> {
        self.browser.terminate()
    }

    fn next_event(
        &mut self,
        timeout: Duration,
    ) -> Option<DriverEvent<BrowserState>> {
        match self.browser.next_event(timeout) {
            Ok(BrowserEvent::StateChanged(state)) => {
                if let Some(coverage) = &mut self.coverage {
                    for block in &state.coverage.branches {
                        let slot = *block as usize % COVERAGE_MAP_SIZE;
                        if !coverage.reported[slot] {
                            coverage.reported[slot] = true;
                            antithesis_fuzzer::notify_coverage(
                                coverage.coverage_map_offset + slot,
                            );
                        }
                    }
                }
                // Then forward the event.
                Some(DriverEvent::StateChanged(Arc::new(state)))
            }
            Ok(BrowserEvent::Error(error)) => Some(DriverEvent::Error(error)),
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                Some(DriverEvent::Timeout)
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => None,
        }
    }

    fn apply(
        &mut self,
        action: BrowserAction,
        state: Arc<BrowserState>,
    ) -> Result<()> {
        self.browser.apply(action, state)
    }

    fn extract_snapshots(
        &mut self,
        state: Arc<BrowserState>,
        last_action: Option<&BrowserAction>,
    ) -> Result<Vec<Snapshot>> {
        if !await_bundle_defined(&state).context("failed to run extractors")? {
            log::warn!("specification bundle not defined, reevaluating...");
            let bundle: &str = &self.specification_bundle;
            state.evaluate_script(bundle)?;
        }
        run_extractors(state, last_action)
    }

    fn state_timestamp(state: &BrowserState) -> SystemTime {
        state.timestamp
    }
}

#[derive(Debug, Clone, Deserialize)]
struct PartialSnapshot {
    index: usize,
    name: Option<String>,
    value: json::Value,
    excluded: bool,
}

fn run_extractors(
    state: Arc<BrowserState>,
    last_action: Option<&BrowserAction>,
) -> Result<Vec<Snapshot>> {
    let console_entries: Vec<json::Value> = state
        .console_entries
        .iter()
        .map(|entry| {
            json::json!({
                "timestamp": entry.timestamp,
                "level": format!("{:?}", entry.level).to_ascii_lowercase(),
                "args": entry.args,
            })
        })
        .collect();

    let state_partial = json::json!({
        "errors": {
            "uncaughtExceptions": &state.exceptions,
        },
        "console": console_entries,
        "navigationHistory": &state.navigation_history,
        "lastAction": json::to_value(last_action)?,
        "resources": &state.resources,
    });

    let partial_snapshots: Vec<PartialSnapshot> = state
            .evaluate_function_call(
                "(state) => __bombadilRequire('@antithesishq/bombadil').runtime.runExtractors({ ...state, document, window })",
                vec![state_partial.clone()]
            )
            ?;

    let time = Time::from_system_time(state.timestamp);
    let results: Vec<Snapshot> = partial_snapshots
        .into_iter()
        .map(|partial| Snapshot {
            index: partial.index,
            name: partial.name,
            value: partial.value,
            time,
            excluded: partial.excluded,
        })
        .collect();

    Ok(results)
}

/// Ensure __bombadilRequire is available (wait for bundle script to execute
/// after reload/navigation).
fn await_bundle_defined(state: &BrowserState) -> Result<bool> {
    for n in 0..5 {
        let defined = state.evaluate_function_call::<bool>(
            r#"() => typeof globalThis.__bombadilRequire === 'function'"#,
            vec![],
        )?;
        if defined {
            return Ok(true);
        } else {
            thread::sleep(Duration::from_millis(200 * n as u64));
        }
    }
    Ok(false)
}
