use std::cmp::max;
use std::path::PathBuf;
use std::time::UNIX_EPOCH;

use crate::browser::actions::{available_actions, BrowserAction, Timeout};
use crate::browser::random;
use crate::instrumentation::EDGE_MAP_SIZE;
use crate::proxy::Proxy;
use crate::state_machine::{self, StateMachine};
use ::url::Url;
use anyhow::{bail, Result};
use log::{debug, error, info};
use serde::Serialize;
use serde_json as json;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tokio::time::timeout;

use crate::browser::state::{BrowserState, ConsoleEntryLevel, Exception};
use crate::browser::{Browser, BrowserOptions};

pub struct RunnerOptions {
    pub exit_on_violation: bool,
    pub states_directory: PathBuf,
}

#[derive(Clone, Serialize)]
pub struct TraceEntry {
    hash_previous: Option<u64>,
    hash_current: Option<u64>,
    action: Option<BrowserAction>,
    screenshot_path: PathBuf,
}

pub async fn run(
    origin: &Url,
    runner_options: &RunnerOptions,
    browser: &mut Browser,
) -> Result<()> {
    let mut rng = rand::rng();
    let mut last_action: Option<BrowserAction> = None;
    let mut last_action_timeout = Timeout::from_secs(1);
    let mut edges = [0u8; EDGE_MAP_SIZE];
    let mut hash_previous: Option<u64> = None;

    let mut trace_file = File::options()
        .append(true)
        .create(true)
        .open(runner_options.states_directory.join("trace.jsonl"))
        .await?;
    let screenshots_dir_path =
        runner_options.states_directory.join("screenshots");
    tokio::fs::create_dir_all(&screenshots_dir_path).await?;

    loop {
        match timeout(last_action_timeout.to_duration(), browser.next_event())
            .await
        {
            Ok(Some(event)) => match event {
                state_machine::Event::StateChanged(state) => {
                    // very basic check until we have spec language and all that
                    match check_page_ok(&state).await {
                        Ok(_) => {}
                        Err(error) => {
                            if runner_options.exit_on_violation {
                                bail!("violation: {}", error);
                            } else {
                                error!("violation: {}", error);
                            }
                        }
                    }

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
                    info!("edge delta: +{}/-{}", added, removed);

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
                    info!("total hits: {}", hits_total);
                    info!(
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

                    let screenshot_path = screenshots_dir_path.join(
                        state
                            .screenshot_path
                            .file_name()
                            .expect("screenshot must have a file name"),
                    );
                    log::info!(
                        "copying {:?} to {:?}",
                        &state.screenshot_path,
                        screenshot_path,
                    );
                    tokio::fs::copy(&state.screenshot_path, &screenshot_path)
                        .await?;

                    let entry = TraceEntry {
                        hash_previous,
                        hash_current: state.transition_hash,
                        action: last_action,
                        screenshot_path,
                    };

                    trace_file
                        .write(json::to_string(&entry)?.as_bytes())
                        .await?;
                    trace_file.write_u8(b'\n').await?;

                    if state.transition_hash.is_some() {
                        log::info!(
                            "got new transition hash: {:?}",
                            state.transition_hash
                        );
                    };
                    hash_previous = state.transition_hash;

                    let actions = available_actions(origin, &state).await?;
                    let action = random::pick_action(&mut rng, actions);

                    match action {
                        (action, timeout) => {
                            info!("picked action: {:?}", action);
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
                debug!("timed out");
                browser.request_state().await;
            }
        }
    }
}

pub async fn run_test(
    origin: Url,
    runner_options: &RunnerOptions,
    browser_options: &BrowserOptions,
) -> Result<()> {
    info!("testing {}", &origin);
    info!("storing states in {:?}", runner_options.states_directory);

    let proxy = Proxy::spawn(0).await?;

    let mut browser_options = browser_options.clone();
    browser_options.proxy = Some(format!("http://127.0.0.1:{}", proxy.port));

    let mut browser = Browser::new(origin.clone(), &browser_options).await?;

    browser.initiate().await?;
    let result = run(&origin, runner_options, &mut browser).await;
    browser.terminate().await?;

    proxy.stop();

    result
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
