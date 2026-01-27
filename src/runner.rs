use crate::browser::actions::{available_actions, BrowserAction, Timeout};
use crate::browser::{random, BrowserEvent, BrowserOptions};
use crate::instrumentation::js::EDGE_MAP_SIZE;
use crate::specification::worker::{PropertyValue, VerifierWorker};
use crate::trace::PropertyViolation;
use ::url::Url;
use serde_json as json;
use std::cmp::max;
use std::sync::Arc;
use tokio::sync::{broadcast, oneshot};
use tokio::time::timeout;
use tokio::{select, spawn};

use crate::browser::state::{BrowserState, Coverage, Exception};
use crate::browser::{Browser, DebuggerOptions};

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
        specification_source: Vec<u8>,
        options: RunnerOptions,
        browser_options: BrowserOptions,
        debugger_options: DebuggerOptions,
    ) -> anyhow::Result<Self> {
        let (events, _) = broadcast::channel(16);
        let (done_sender, done_receiver) = oneshot::channel();
        let (shutdown_sender, shutdown_receiver) = oneshot::channel();

        let browser =
            Browser::new(origin.clone(), browser_options, debugger_options)
                .await?;

        let verifier = VerifierWorker::start(specification_source)?;

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
                    origin,
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
        origin: Url,
        options: RunnerOptions,
        browser: &mut Browser,
        verifier: Arc<VerifierWorker>,
        events: broadcast::Sender<RunEvent>,
        mut shutdown: oneshot::Receiver<()>,
    ) -> anyhow::Result<()> {
        let mut last_action: Option<BrowserAction> = None;
        let mut last_action_timeout = Timeout::from_secs(1);
        let mut edges = [0u8; EDGE_MAP_SIZE];

        let extractors = verifier.extractors().await?;

        loop {
            let verifier = verifier.clone();
            select! {
                _ = &mut shutdown => {
                    return Ok(())
                },
                event = timeout( last_action_timeout.to_duration(), browser.next_event() ) => match event {
                    Ok(Some(event)) => match event {
                        BrowserEvent::StateChanged(state) => {
                            // Step formulas and collect violations.
                            let snapshots = run_extractors(&state, &extractors).await?;
                            let property_results = verifier.step(snapshots, state.timestamp).await?;
                            let mut violations = Vec::with_capacity(property_results.len());
                            for (name, value) in property_results {
                                if let PropertyValue::False(violation) = value {
                                    violations.push(PropertyViolation{ name, violation });
                                }
                            }
                            let has_violations = !violations.is_empty();

                            // Update global edges.
                            for (index, bucket) in &state.coverage.edges_new {
                                edges[*index as usize] =
                                    max(edges[*index as usize], *bucket);
                            }
                            log_coverage_stats_increment(&state.coverage);
                            log_coverage_stats_total(&edges);

                            let actions =
                                available_actions(&origin, &state).await?;

                            let action = {
                                let mut rng = rand::rng();
                                random::pick_action(&mut rng, actions)
                            };

                            events.send(RunEvent::NewState {
                                state,
                                last_action,
                                violations,
                            })?;
                            if has_violations && options.stop_on_violation {
                                return Ok(())
                            }

                            let (action, timeout) = action;
                            log::info!("picked action: {:?}", action);
                            browser.apply(action.clone()).await?;
                            last_action = Some(action);
                            last_action_timeout = timeout;
                        }
                        BrowserEvent::Error(error) => {
                            anyhow::bail!("state machine error: {}", error)
                        }
                    },
                    Ok(None) => {
                        anyhow::bail!("browser closed")
                    }
                    Err(_) => {
                        log::debug!("timed out");
                        browser.request_state().await;
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
) -> anyhow::Result<Vec<(u64, json::Value)>> {
    let mut results = Vec::with_capacity(extractors.len());

    let uncaught_exception =
        if let Some(Exception::UncaughtException(e)) = &state.exception {
            Some(e)
        } else {
            None
        };

    let unhandled_promise_rejection =
        if let Some(Exception::UnhandledPromiseRejection(e)) = &state.exception
        {
            Some(e)
        } else {
            None
        };

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
            "uncaught_exception": uncaught_exception,
            "unhandled_promise_rejection": unhandled_promise_rejection,
        },
        "console": console_entries

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

/*
async fn check_page_ok(state: &BrowserState) -> Result<(), Violation> {
    let status: Option<u16> = state.evaluate_function_call(
                        "() => window.performance.getEntriesByType('navigation')[0]?.responseStatus", vec![]
                    ).await?;
    if let Some(status) = status
        && status >= 400
    {
        invariant_violation!(
            "expected 2xx or 3xx but got {} at {} ({})",
            status,
            state.title,
            state.url
        );
    }

    for entry in &state.console_entries {
        if let ConsoleEntryLevel::Error = entry.level {
            invariant_violation!(
                "console.error at {}: {:?}",
                entry.timestamp.duration_since(UNIX_EPOCH)?.as_micros(),
                entry.args
            )
        }
    }

    if let Some(exception) = &state.exception {
        fn formatted(value: &json::Value) -> Result<String, Violation> {
            match value {
                json::Value::String(s) => Ok(s.clone()),
                other => json::to_string_pretty(other).map_err(Into::into),
            }
        }
        match exception {
            Exception::UncaughtException(value) => {
                invariant_violation!(
                    "uncaught exception: {}",
                    formatted(value)?
                )
            }
            Exception::UnhandledPromiseRejection(value) => {
                invariant_violation!(
                    "unhandled promise rejection: {}",
                    formatted(value)?
                )
            }
        }
    }

    Ok(())
}
*/

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
