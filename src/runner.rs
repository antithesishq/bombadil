use std::cmp::max;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use crate::browser::actions::{available_actions, BrowserAction, Timeout};
use crate::browser::random;
use crate::instrumentation::EDGE_MAP_SIZE;
use crate::proxy::Proxy;
use crate::state_machine::{self, StateMachine};
use ::url::Url;
use anyhow::{bail, Result};
use serde::Serialize;
use serde_json as json;
use tokio::spawn;
use tokio::sync::broadcast;
use tokio::time::timeout;

use crate::browser::state::{BrowserState, ConsoleEntryLevel, Exception};
use crate::browser::{Browser, BrowserOptions};

#[derive(Debug, Clone, Serialize)]
pub struct TraceEntry {
    pub url: Url,
    pub hash_previous: Option<u64>,
    pub hash_current: Option<u64>,
    pub action: Option<BrowserAction>,
    pub screenshot_path: PathBuf,
}

#[derive(Debug, Clone)]
pub enum RunEvent {
    NewTraceEntry {
        entry: TraceEntry,
        violation: Arc<Option<anyhow::Error>>,
    },
    Error(Arc<anyhow::Error>),
}

pub async fn run(
    origin: Url,
    browser: &mut Browser,
    sender: broadcast::Sender<RunEvent>,
) -> Result<()> {
    let mut last_action: Option<BrowserAction> = None;
    let mut last_action_timeout = Timeout::from_secs(1);
    let mut edges = [0u8; EDGE_MAP_SIZE];
    let mut hash_previous: Option<u64> = None;

    loop {
        match timeout(last_action_timeout.to_duration(), browser.next_event())
            .await
        {
            Ok(Some(event)) => match event {
                state_machine::Event::StateChanged(state) => {
                    // very basic check until we have spec language and all that
                    let violation = check_page_ok(&state).await.err();

                    let (added, removed) =
                        state.coverage.edges_new.iter().fold(
                            (0usize, 0usize),
                            |(added, removed), (_, bucket)| {
                                if *bucket > 0 {
                                    (added + 1, removed)
                                } else {
                                    (added, removed + 1)
                                }
                            },
                        );
                    log::info!("edge delta: +{}/-{}", added, removed);

                    // Update global edges.
                    for (index, bucket) in &state.coverage.edges_new {
                        edges[*index as usize] =
                            max(edges[*index as usize], *bucket);
                    }

                    let mut buckets = [0u64; 8];
                    let mut hits_total: u64 = 0;
                    for bucket in edges {
                        if bucket > 0 {
                            buckets[bucket as usize - 1] += 1;
                            hits_total += 1;
                        }
                    }
                    log::info!("total hits: {}", hits_total);
                    log::info!(
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

                    let entry = TraceEntry {
                        url: state.url.clone(),
                        hash_previous,
                        hash_current: state.transition_hash,
                        action: last_action,
                        screenshot_path: state.screenshot_path.clone(),
                    };

                    sender
                        .send(RunEvent::NewTraceEntry {
                            entry: entry.clone(),
                            violation: Arc::new(violation),
                        })
                        .expect("send failed");
                    hash_previous = state.transition_hash;

                    let actions = available_actions(&origin, &state).await?;

                    let action = {
                        let mut rng = rand::rng();
                        random::pick_action(&mut rng, actions)
                    };

                    match action {
                        (action, timeout) => {
                            log::info!("picked action: {:?}", action);
                            browser.apply(action.clone()).await?;
                            last_action = Some(action);
                            last_action_timeout = timeout;
                        }
                    }
                }
                state_machine::Event::Error(error) => {
                    bail!("state machine error: {}", error)
                }
            },
            Ok(None) => {
                bail!("browser closed")
            }
            Err(_) => {
                log::debug!("timed out");
                browser.request_state().await;
            }
        }
    }
}

pub async fn run_test(
    origin: Url,
    browser_options: &BrowserOptions,
) -> Result<broadcast::Receiver<RunEvent>> {
    log::info!("testing {}", &origin);

    let (sender, receiver) = broadcast::channel(16);
    let proxy = Proxy::spawn(0).await?;

    let mut browser_options = browser_options.clone();
    browser_options.proxy = Some(format!("http://127.0.0.1:{}", proxy.port));

    let mut browser = Browser::new(origin.clone(), &browser_options).await?;
    browser.initiate().await?;

    spawn(async move {
        let result = run(origin, &mut browser, sender.clone()).await;
        // Try to gracefully shutdown first.
        if let Err(err) = browser.terminate().await {
            log::warn!("browser didn't close successfully: {}", err)
        };
        proxy.stop();
        // Then send the error.
        if let Err(err) = result {
            sender
                .send(RunEvent::Error(Arc::new(err)))
                .expect("send error failed");
        }
    });

    Ok(receiver)
}

async fn check_page_ok(state: &BrowserState) -> Result<()> {
    let status: Option<u16> = state.evaluate_function_call(
                        "() => window.performance.getEntriesByType('navigation')[0]?.responseStatus", vec![]
                    ).await?;
    if let Some(status) = status
        && status >= 400
    {
        bail!(
            "expected 2xx or 3xx but got {} at {} ({})",
            status,
            state.title,
            state.url
        );
    }

    for entry in &state.console_entries {
        match entry.level {
            ConsoleEntryLevel::Error => bail!(
                "console.error at {}: {:?}",
                entry.timestamp.duration_since(UNIX_EPOCH)?.as_micros(),
                entry.args
            ),
            _ => {}
        }
    }

    if let Some(exception) = &state.exception {
        fn formatted(value: &json::Value) -> Result<String> {
            match value {
                json::Value::String(s) => Ok(s.clone()),
                other => json::to_string_pretty(other).map_err(Into::into),
            }
        }
        match exception {
            Exception::UncaughtException(value) => {
                bail!("uncaught exception: {}", formatted(value)?)
            }
            Exception::UnhandledPromiseRejection(value) => {
                bail!("unhandled promise rejection: {}", formatted(value)?)
            }
        }
    }

    Ok(())
}
