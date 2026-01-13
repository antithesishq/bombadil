use std::time::UNIX_EPOCH;

use crate::browser::actions::{available_actions, Timeout};
use crate::browser::random;
use crate::instrumentation::EDGE_MAP_SIZE;
use crate::proxy::Proxy;
use crate::state_machine::{self, StateMachine};
use ::url::Url;
use anyhow::{bail, Result};
use bit_set::BitSet;
use log::{debug, error, info};
use serde_json as json;
use tokio::time::timeout;

use crate::browser::state::{BrowserState, ConsoleEntryLevel, Exception};
use crate::browser::{Browser, BrowserOptions};

pub struct RunnerOptions {
    pub exit_on_violation: bool,
}

pub async fn run(
    origin: &Url,
    runner_options: &RunnerOptions,
    browser: &mut Browser,
) -> Result<()> {
    let mut rng = rand::rng();
    let mut last_action_timeout = Timeout::from_secs(1);
    let mut edges = BitSet::with_capacity(EDGE_MAP_SIZE);
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

                    for (index, bucket) in &state.coverage.edges_new {
                        if *bucket > 0 {
                            edges.insert(*index as usize);
                        }
                    }
                    info!("total edges: {}", edges.len());

                    let actions = available_actions(origin, &state).await?;
                    let action = random::pick_action(&mut rng, actions);

                    match action {
                        (action, timeout) => {
                            info!("picked action: {:?}", action);
                            browser.apply(action.clone()).await?;
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
