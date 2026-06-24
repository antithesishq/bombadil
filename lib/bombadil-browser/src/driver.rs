use std::cmp::max;
use std::sync::Arc;
use std::sync::mpsc as std_mpsc;
use std::time::SystemTime;

use anyhow::{Result, anyhow};
use bombadil::driver::{DriverEvent, InterfaceDriver};
use bombadil::specification::domain::Snapshot;
use bombadil_schema::Time;
use serde::Deserialize;
use serde_json as json;
use tokio::sync::mpsc::{
    UnboundedReceiver, UnboundedSender, unbounded_channel,
};
use url::Url;

use crate::browser::actions::BrowserAction;
use crate::browser::actions::BrowserActionTemplate;
use crate::browser::state::{BrowserState, Coverage};
use crate::browser::{Browser, BrowserEvent, BrowserOptions, DebuggerOptions};
use crate::instrumentation::js::EDGE_MAP_SIZE;

/// Commands sent from the synchronous [`BrowserDriver`] to the asynchronous
/// browser worker thread.
enum BrowserCommand {
    Initiate {
        reply: std_mpsc::Sender<Result<()>>,
    },
    NextEvent {
        reply: std_mpsc::Sender<Option<DriverEvent<BrowserState>>>,
    },
    Apply {
        action: BrowserAction,
        reply: std_mpsc::Sender<Result<()>>,
    },
    ExtractSnapshots {
        state: Arc<BrowserState>,
        last_action: Option<BrowserAction>,
        reply: std_mpsc::Sender<Result<Vec<Snapshot>>>,
    },
    Terminate {
        reply: std_mpsc::Sender<Result<()>>,
    },
}

pub struct BrowserDriver {
    command_send: UnboundedSender<BrowserCommand>,
    worker: Option<std::thread::JoinHandle<()>>,
    // Heap-allocated so the 64 KB edge map doesn't blow the stack.
    edges: Vec<u8>,
}

impl BrowserDriver {
    pub fn launch(
        origin: Url,
        browser_options: BrowserOptions,
        debugger_options: DebuggerOptions,
        specification_bundle: String,
    ) -> Result<Self> {
        let (command_send, command_receive) = unbounded_channel();
        let (ready_send, ready_receive) = std_mpsc::channel();

        let worker = std::thread::Builder::new()
            .name("bombadil-browser-worker".to_string())
            .spawn(move || {
                run_browser_worker(
                    origin,
                    browser_options,
                    debugger_options,
                    specification_bundle,
                    command_receive,
                    ready_send,
                );
            })?;

        ready_receive
            .recv()
            .map_err(|_| anyhow!("browser worker died before ready"))??;

        Ok(Self {
            command_send,
            worker: Some(worker),
            edges: vec![0u8; EDGE_MAP_SIZE],
        })
    }
}

impl InterfaceDriver for BrowserDriver {
    type Action = BrowserAction;
    type ActionTemplate = BrowserActionTemplate;
    type State = BrowserState;

    fn initiate(&mut self) -> Result<()> {
        let (reply_send, reply_receive) = std_mpsc::channel();
        self.command_send
            .send(BrowserCommand::Initiate { reply: reply_send })
            .map_err(|_| anyhow!("browser worker gone"))?;
        reply_receive
            .recv()
            .map_err(|_| anyhow!("browser worker gone"))?
    }

    fn terminate(mut self) -> Result<()> {
        let (reply_send, reply_receive) = std_mpsc::channel();
        self.command_send
            .send(BrowserCommand::Terminate { reply: reply_send })
            .map_err(|_| anyhow!("browser worker gone"))?;
        let result = reply_receive
            .recv()
            .map_err(|_| anyhow!("browser worker gone"))?;
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        result
    }

    fn next_event(&mut self) -> Option<DriverEvent<BrowserState>> {
        let (reply_send, reply_receive) = std_mpsc::channel();
        if self
            .command_send
            .send(BrowserCommand::NextEvent { reply: reply_send })
            .is_err()
        {
            return None;
        }
        match reply_receive.recv().ok().flatten() {
            Some(DriverEvent::StateChanged(state)) => {
                // Main edge coverage map.
                for (index, bucket) in &state.coverage.edges_new {
                    self.edges[*index as usize] =
                        max(self.edges[*index as usize], *bucket);
                }
                log_coverage_stats_increment(&state.coverage);
                log_coverage_stats_total(&self.edges);
                // Then forward the event.
                Some(DriverEvent::StateChanged(state))
            }
            other => other,
        }
    }

    fn apply(&mut self, action: BrowserAction) -> Result<()> {
        let (reply_send, reply_receive) = std_mpsc::channel();
        self.command_send
            .send(BrowserCommand::Apply {
                action,
                reply: reply_send,
            })
            .map_err(|_| anyhow!("browser worker gone"))?;
        reply_receive
            .recv()
            .map_err(|_| anyhow!("browser worker gone"))?
    }

    fn extract_snapshots(
        &mut self,
        state: Arc<BrowserState>,
        last_action: Option<&BrowserAction>,
    ) -> Result<Vec<Snapshot>> {
        let (reply_send, reply_receive) = std_mpsc::channel();
        self.command_send
            .send(BrowserCommand::ExtractSnapshots {
                state,
                last_action: last_action.cloned(),
                reply: reply_send,
            })
            .map_err(|_| anyhow!("browser worker gone"))?;
        reply_receive
            .recv()
            .map_err(|_| anyhow!("browser worker gone"))?
    }

    fn state_timestamp(state: &BrowserState) -> SystemTime {
        state.timestamp
    }
}

fn run_browser_worker(
    origin: Url,
    browser_options: BrowserOptions,
    debugger_options: DebuggerOptions,
    specification_bundle: String,
    mut command_receive: UnboundedReceiver<BrowserCommand>,
    ready_send: std_mpsc::Sender<Result<()>>,
) {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = ready_send.send(Err(error.into()));
            return;
        }
    };

    runtime.block_on(async move {
        let mut browser =
            match Browser::new(origin, browser_options, debugger_options).await
            {
                Ok(browser) => browser,
                Err(error) => {
                    let _ = ready_send.send(Err(error));
                    return;
                }
            };

        if let Err(error) =
            browser.ensure_script_evaluated(&specification_bundle).await
        {
            let _ = ready_send.send(Err(error));
            return;
        }

        if ready_send.send(Ok(())).is_err() {
            // Driver was dropped before we finished setup.
            return;
        }

        // `terminate` consumes the browser, so we carry the reply out of the
        // loop and call it once afterwards.
        let mut terminate_reply = None;
        while let Some(command) = command_receive.recv().await {
            match command {
                BrowserCommand::Initiate { reply } => {
                    let _ = reply.send(browser.initiate().await);
                }
                BrowserCommand::NextEvent { reply } => {
                    let event = match browser.next_event().await {
                        Some(BrowserEvent::StateChanged(state)) => {
                            Some(DriverEvent::StateChanged(state))
                        }
                        Some(BrowserEvent::Error(error)) => {
                            Some(DriverEvent::Error(error))
                        }
                        None => None,
                    };
                    let _ = reply.send(event);
                }
                BrowserCommand::Apply { action, reply } => {
                    let _ = reply.send(browser.apply(action));
                }
                BrowserCommand::ExtractSnapshots {
                    state,
                    last_action,
                    reply,
                } => {
                    let result =
                        run_extractors(state, last_action.as_ref()).await;
                    let _ = reply.send(result);
                }
                BrowserCommand::Terminate { reply } => {
                    terminate_reply = Some(reply);
                    break;
                }
            }
        }

        if let Some(reply) = terminate_reply {
            let _ = reply.send(browser.terminate().await);
        }
    });
}

#[derive(Debug, Clone, Deserialize)]
struct PartialSnapshot {
    index: usize,
    name: Option<String>,
    value: json::Value,
}

async fn run_extractors(
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
        .await?;

    let partial_snapshots: Vec<PartialSnapshot> = state
            .evaluate_function_call(
                "(state) => __bombadilRequire('@antithesishq/bombadil').runtime.runExtractors({ ...state, document, window })",
                vec![state_partial.clone()],
            )
            .await?;

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
