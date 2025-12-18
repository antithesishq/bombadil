use std::time::UNIX_EPOCH;

use crate::browser::actions::{available_actions, Timeout};
use crate::browser::actions::{hegel, random};
use crate::state_machine::{self, StateMachine};
use ::url::Url;
use anyhow::bail;
use log::{debug, info};
use serde_json as json;
use tokio::time::timeout;

use crate::browser::state::{BrowserState, ConsoleEntryLevel};
use crate::browser::{Browser, BrowserOptions};

pub struct TestOptions {
    pub hegel: bool,
}

pub async fn run(
    browser: &mut Browser,
    test_options: TestOptions,
) -> anyhow::Result<()> {
    let mut rng = rand::rng();
    let mut last_action_timeout = Timeout::from_secs(1);
    loop {
        match timeout(last_action_timeout.to_duration(), browser.next_event())
            .await
        {
            Ok(Some(event)) => match event {
                state_machine::Event::StateChanged(state) => {
                    // very basic check until we have spec language and all that
                    check_page_ok(&state).await?;

                    let actions = available_actions(&state).await?;
                    let action = if test_options.hegel {
                        hegel::pick_action(actions)
                    } else {
                        random::pick_action(&mut rng, actions)
                    };
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
    test_options: TestOptions,
    browser_options: BrowserOptions,
) -> anyhow::Result<()> {
    info!("testing {}", &origin);
    let mut browser = Browser::new(origin, browser_options).await?;

    browser.initiate().await?;
    let result = run(&mut browser, test_options).await;
    browser.terminate().await?;

    result
}

async fn check_page_ok(state: &BrowserState) -> anyhow::Result<()> {
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
        let formatted = match exception {
            json::Value::String(s) => s.clone(),
            other => json::to_string_pretty(other)?,
        };
        bail!("uncaught exception: {}", formatted)
    }

    Ok(())
}
