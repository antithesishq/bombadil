use std::time::UNIX_EPOCH;

use crate::browser::actions::{available_actions, Timeout};
#[cfg(feature = "hegel")]
use crate::browser::hegel;
#[cfg(not(feature = "hegel"))]
use crate::browser::random;
use crate::state_machine::{self, StateMachine};
use ::url::Url;
use anyhow::{bail, Result};
use log::{debug, info};
use serde_json as json;
use tokio::time::timeout;

use crate::browser::state::{BrowserState, ConsoleEntryLevel, Exception};
use crate::browser::{Browser, BrowserOptions};

pub async fn run(origin: &Url, browser: &mut Browser) -> Result<()> {
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

                    let actions = available_actions(origin, &state).await?;

                    #[cfg(feature = "hegel")]
                    let action = hegel::pick_action(actions);

                    #[cfg(not(feature = "hegel"))]
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
    browser_options: BrowserOptions,
) -> Result<()> {
    info!("testing {}", &origin);
    let mut browser = Browser::new(origin.clone(), browser_options).await?;

    browser.initiate().await?;
    let result = run(&origin, &mut browser).await;
    browser.terminate().await?;

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
