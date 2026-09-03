use std::cmp::max;
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::Result;
use bombadil::driver::{DriverEvent, InterfaceDriver};
use bombadil::specification::domain::Snapshot;
use bombadil_schema::Time;
use serde::Deserialize;
use serde_json as json;
use url::Url;

use crate::browser::actions::BrowserAction;
use crate::browser::actions::BrowserActionTemplate;
use crate::browser::state::{BrowserState, Coverage};
use crate::browser::{Browser, BrowserEvent, BrowserOptions};
use crate::chromium;
use crate::chromium::Chromium;
use crate::instrumentation::js::EDGE_MAP_SIZE;

pub enum DebuggerOptions {
    External {
        remote_debugger: Url,
    },
    Managed {
        launch_options: chromium::LaunchOptions,
    },
}

pub struct BrowserDriver {
    // Heap-allocated so the 64 KB edge map doesn't blow the stack.
    edges: Vec<u8>,
    coverage_map_offset: usize,
    browser: Browser,
}

impl BrowserDriver {
    pub fn launch(
        origin: Url,
        browser_options: BrowserOptions,
        debugger_options: DebuggerOptions,
        specification_bundle: String,
    ) -> Result<Self> {
        let coverage_map_offset = antithesis_fuzzer::init_coverage_module(
            EDGE_MAP_SIZE,
            "bombadil.tsv",
        );

        let chromium = match debugger_options {
            DebuggerOptions::External { remote_debugger } => {
                Chromium::connect(remote_debugger)?
            }
            DebuggerOptions::Managed { launch_options } => {
                Chromium::launch(launch_options)?
            }
        };
        let browser = Browser::new(origin, browser_options, chromium)?;
        browser.ensure_script_evaluated(&specification_bundle)?;

        Ok(Self {
            edges: vec![0u8; EDGE_MAP_SIZE],
            coverage_map_offset,
            browser,
        })
    }
}

impl InterfaceDriver for BrowserDriver {
    type Action = BrowserAction;
    type ActionTemplate = BrowserActionTemplate;
    type State = BrowserState;

    fn initiate(&mut self) -> Result<()> {
        self.browser.initiate()
    }

    fn terminate(self) -> Result<()> {
        self.browser.terminate()
    }

    fn next_event(&mut self) -> Option<DriverEvent<BrowserState>> {
        match self.browser.next_event() {
            Some(BrowserEvent::StateChanged(state)) => {
                for (index, bucket) in &state.coverage.edges_new {
                    let index = *index as usize;
                    // Report coverage changes to Antithesis.
                    if self.edges[index] == 0 {
                        assert!(
                            self.coverage_map_offset
                                < (usize::MAX - EDGE_MAP_SIZE),
                            "offset + index overflows usize"
                        );
                        antithesis_fuzzer::notify_coverage(
                            self.coverage_map_offset + index,
                        );
                    }
                    // Update main edge coverage map.
                    self.edges[index] = max(self.edges[index], *bucket);
                }
                log_coverage_stats_increment(&state.coverage);
                log_coverage_stats_total(&self.edges);
                // Then forward the event.
                Some(DriverEvent::StateChanged(Arc::new(state)))
            }
            Some(BrowserEvent::Error(error)) => Some(DriverEvent::Error(error)),
            None => None,
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

    // Ensure __bombadilRequire is available (wait for bundle script to execute
    // after reload/navigation). Use async/await to avoid blocking the event loop.
    state
        .evaluate_function_call::<json::Value>(
            r#"
            async () => {
                const start = Date.now();
                const timeout = 5000;
                while (typeof globalThis.__bombadilRequire !== 'function') {
                    if (Date.now() - start > timeout) {
                        throw new Error('__bombadilRequire not available after ' + timeout + 'ms');
                    }
                    await new Promise(resolve => setTimeout(resolve, 10));
                }
                return true;
            }
            "#,
            vec![],
        )
        ?;

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
        })
        .collect();

    Ok(results)
}

fn log_coverage_stats_increment(coverage: &Coverage) {
    if log::log_enabled!(log::Level::Debug) {
        let (added, removed) = coverage.edges_new.iter().fold(
            (0usize, 0usize),
            |(added, removed), (_, bucket)| {
                if *bucket > 0 {
                    (added + 1, removed)
                } else {
                    (added, removed + 1)
                }
            },
        );
        log::debug!("edge delta: +{}/-{}", added, removed);
    }
}

fn log_coverage_stats_total(edges: &[u8]) {
    if log::log_enabled!(log::Level::Debug) {
        let mut buckets = [0u64; 8];
        let mut hits_total: u64 = 0;
        for bucket in edges {
            if *bucket > 0 {
                buckets[*bucket as usize - 1] += 1;
                hits_total += 1;
            }
        }
        log::debug!("total hits: {}", hits_total);
        log::debug!(
            "total edges (max bucket): {:04} {:04} {:04} {:04} {:04} {:04} {:04} {:04}",
            buckets[0],
            buckets[1],
            buckets[2],
            buckets[3],
            buckets[4],
            buckets[5],
            buckets[6],
            buckets[7],
        );
    }
}
