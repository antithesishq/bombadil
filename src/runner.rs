use crate::browser::actions::BrowserAction;
use crate::browser::{BrowserEvent, BrowserOptions};
use crate::instrumentation::js::EDGE_MAP_SIZE;
use crate::specification::verifier::Specification;
use crate::specification::worker::{PropertyValue, VerifierWorker};
use crate::trace::PropertyViolation;
use ::url::Url;
use serde_json as json;
use std::cmp::max;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, oneshot};
use tokio::{select, spawn};

use crate::browser::state::{BrowserState, Coverage};
use crate::browser::{Browser, DebuggerOptions};
use crate::url::is_within_domain;

pub struct RunnerOptions {
    pub stop_on_violation: bool,
}

#[derive(Debug, Clone)]
pub enum RunEvent {
    NewState {
        state: BrowserState,
        last_action: Option<BrowserAction>,
        violations: Vec<PropertyViolation>,
    },
}

pub struct Runner {
    origin: Url,
    options: RunnerOptions,
    browser: Browser,
    verifier: Arc<VerifierWorker>,
    events: broadcast::Sender<RunEvent>,
    shutdown_sender: oneshot::Sender<()>,
    shutdown_receiver: oneshot::Receiver<()>,
    done_sender: oneshot::Sender<anyhow::Result<()>>,
    done_receiver: oneshot::Receiver<anyhow::Result<()>>,
}

impl Runner {
    pub async fn new(
        origin: Url,
        specification: Specification,
        options: RunnerOptions,
        browser_options: BrowserOptions,
        debugger_options: DebuggerOptions,
    ) -> anyhow::Result<Self> {
        let (events, _) = broadcast::channel(16);
        let (done_sender, done_receiver) = oneshot::channel();
        let (shutdown_sender, shutdown_receiver) = oneshot::channel();

        let verifier = VerifierWorker::start(specification).await?;

        let browser =
            Browser::new(origin.clone(), browser_options, debugger_options)
                .await?;

        Ok(Runner {
            origin,
            options,
            browser,
            verifier,
            events,
            shutdown_sender,
            shutdown_receiver,
            done_sender,
            done_receiver,
        })
    }

    pub fn start(self) -> RunEvents {
        let Runner {
            origin,
            options,
            mut browser,
            verifier,
            events,
            shutdown_sender,
            shutdown_receiver,
            done_sender,
            done_receiver,
        } = self;

        log::info!("starting test of {}", origin);
        let events_receiver = events.subscribe();

        spawn(async move {
            let run = async || {
                browser.initiate().await?;
                log::debug!("browser initiated");
                Runner::run_test(
                    &origin,
                    options,
                    &mut browser,
                    verifier,
                    events,
                    shutdown_receiver,
                )
                .await
            };
            let result = run().await;
            log::debug!("test finished");

            browser
                .terminate()
                .await
                .expect("browser failed to terminate");

            done_sender
                .send(result)
                .expect("couldn't send runner completion")
        });

        RunEvents {
            events: events_receiver,
            done: done_receiver,
            shutdown: shutdown_sender,
        }
    }

    async fn run_test(
        origin: &Url,
        options: RunnerOptions,
        browser: &mut Browser,
        verifier: Arc<VerifierWorker>,
        events: broadcast::Sender<RunEvent>,
        mut shutdown: oneshot::Receiver<()>,
    ) -> anyhow::Result<()> {
        let mut last_action: Option<BrowserAction> = None;
        let mut edges = [0u8; EDGE_MAP_SIZE];

        let extractors = verifier.extractors().await?;

        loop {
            let verifier = verifier.clone();
            select! {
                _ = &mut shutdown => {
                    return Ok(())
                },
                event = browser.next_event() => match event {
                    Some(event) => match event {
                        BrowserEvent::StateChanged(state) => {
                            // Step formulas and collect violations.
                            let snapshots = run_extractors(&state, &extractors, &last_action).await?;
                            let step_result = verifier.step::<crate::specification::js::JsAction>(snapshots, state.timestamp).await?;

                            // Convert JsAction tree to BrowserAction tree
                            let action_tree = step_result.actions.try_map(&mut |js_action| {
                                js_action.to_browser_action()
                            })?;

                            let mut violations = Vec::with_capacity(step_result.properties.len());
                            let mut all_properties_definite = true;
                            for (name, value) in step_result.properties {
                                match value {
                                    PropertyValue::False(violation) => {
                                        violations.push(PropertyViolation{ name, violation });
                                    }
                                    PropertyValue::Residual => {
                                        all_properties_definite = false;
                                    }
                                    PropertyValue::True => {
                                        // Property is satisfied
                                    }
                                }
                            }
                            let has_violations = !violations.is_empty();

                            // Make sure we stay within origin.
                            let action_tree = if !is_within_domain(&state.url, origin) {
                                action_tree.filter(&|a| matches!(a, BrowserAction::Back))
                            } else {
                                action_tree
                            };

                            // Update global edges.
                            for (index, bucket) in &state.coverage.edges_new {
                                edges[*index as usize] =
                                    max(edges[*index as usize], *bucket);
                            }
                            log_coverage_stats_increment(&state.coverage);
                            log_coverage_stats_total(&edges);

                            events.send(RunEvent::NewState {
                                state,
                                last_action,
                                violations,
                            })?;
                            if has_violations && options.stop_on_violation {
                                return Ok(())
                            }
                            if all_properties_definite {
                                log::info!("all properties are definite, stopping");
                                return Ok(())
                            }

                            let action_tree = action_tree.prune()
                                .ok_or_else(|| anyhow::anyhow!("no actions available"))?;

                            let action = action_tree.pick(&mut rand::rng())?.clone();
                            let timeout = action_timeout(&action);
                            log::info!("picked action: {:?}", action);
                            browser.apply(action.clone(), timeout)?;
                            last_action = Some(action);
                        }
                        BrowserEvent::Error(error) => {
                            anyhow::bail!("state machine error: {}", error)
                        }
                    },
                    None => {
                        anyhow::bail!("browser closed")
                    }
                }
            }
        }
    }
}

pub struct RunEvents {
    events: broadcast::Receiver<RunEvent>,
    done: oneshot::Receiver<anyhow::Result<()>>,
    shutdown: oneshot::Sender<()>,
}

impl RunEvents {
    pub async fn next(&mut self) -> anyhow::Result<Option<RunEvent>> {
        match self.events.recv().await {
            Ok(event) => Ok(Some(event)),
            Err(broadcast::error::RecvError::Closed) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// Shuts down the runner, waiting for it to finish and clean up. Returns an Err when some
    /// non-recoverable error occured, as opposed to test violations which are sent in trace events.
    pub async fn shutdown(mut self) -> anyhow::Result<()> {
        // If we can't send the signal, it means the receiver has already been dropped.
        let _ = self.shutdown.send(());
        (&mut self.done).await?
    }
}

async fn run_extractors(
    state: &BrowserState,
    extractors: &Vec<(u64, String)>,
    last_action: &Option<BrowserAction>,
) -> anyhow::Result<Vec<(u64, json::Value)>> {
    let mut results = Vec::with_capacity(extractors.len());

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

    for (key, function) in extractors {
        let json: json::Value = state
            .evaluate_function_call(
                format!(
                    "(state) => ({})({{ ...state, document, window }})",
                    function
                ),
                vec![state_partial.clone()],
            )
            .await?;
        results.push((*key, json));
    }
    Ok(results)
}

fn action_timeout(action: &BrowserAction) -> Duration {
    match action {
        BrowserAction::Back => Duration::from_secs(2),
        BrowserAction::Forward => Duration::from_secs(2),
        BrowserAction::Reload => Duration::from_secs(2),
        BrowserAction::Click { .. } => Duration::from_millis(500),
        BrowserAction::TypeText {
            text, delay_millis, ..
        } => {
            // We'll wait for the text to be entered, and an extra 100ms.
            let text_entry_millis =
                (*delay_millis).saturating_mul(text.len() as u64);
            Duration::from_millis(text_entry_millis.saturating_add(100u64))
        }
        BrowserAction::PressKey { .. } => Duration::from_millis(50),
        BrowserAction::ScrollUp { .. } => Duration::from_millis(100),
        BrowserAction::ScrollDown { .. } => Duration::from_millis(100),
    }
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

fn log_coverage_stats_total(edges: &[u8; EDGE_MAP_SIZE]) {
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
