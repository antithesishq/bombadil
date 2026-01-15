use std::cmp::max;
use std::fmt::Display;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use crate::browser::actions::{available_actions, BrowserAction, Timeout};
use crate::browser::random;
use crate::instrumentation::EDGE_MAP_SIZE;
use crate::proxy::Proxy;
use crate::state_machine::{self, StateMachine};
use ::url::Url;
use serde::Serialize;
use serde_json as json;
use tokio::sync::{broadcast, oneshot};
use tokio::time::timeout;
use tokio::{select, spawn};

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
        violation: Option<Violation>,
    },
}

pub struct Runner {
    origin: Url,
    browser: Browser,
    proxy: Proxy,
    events: broadcast::Sender<RunEvent>,
    shutdown_sender: oneshot::Sender<()>,
    shutdown_receiver: oneshot::Receiver<()>,
    done_sender: oneshot::Sender<anyhow::Result<()>>,
    done_receiver: oneshot::Receiver<anyhow::Result<()>>,
}

impl Runner {
    pub async fn new(
        origin: Url,
        browser_options: &BrowserOptions,
    ) -> anyhow::Result<Self> {
        let (events, _) = broadcast::channel(16);
        let (done_sender, done_receiver) = oneshot::channel();
        let (shutdown_sender, shutdown_receiver) = oneshot::channel();
        let proxy = Proxy::spawn(0).await?;

        let mut browser_options = browser_options.clone();
        browser_options.proxy =
            Some(format!("http://127.0.0.1:{}", proxy.port));

        let browser = Browser::new(origin.clone(), &browser_options).await?;

        Ok(Runner {
            origin,
            browser,
            proxy,
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
            mut browser,
            proxy,
            events,
            shutdown_sender,
            shutdown_receiver,
            done_sender,
            done_receiver,
        } = self;

        log::info!("starting test of {}", origin);
        let events_receiver = events.subscribe();

        spawn(async move {
            let result = async {
                browser.initiate().await?;
                Runner::run_test(
                    origin,
                    browser,
                    proxy,
                    events,
                    shutdown_receiver,
                )
                .await
            }
            .await;

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
        mut browser: Browser,
        proxy: Proxy,
        events: broadcast::Sender<RunEvent>,
        mut shutdown: oneshot::Receiver<()>,
    ) -> anyhow::Result<()> {
        let mut last_action: Option<BrowserAction> = None;
        let mut last_action_timeout = Timeout::from_secs(1);
        let mut edges = [0u8; EDGE_MAP_SIZE];
        let mut hash_previous: Option<u64> = None;

        loop {
            select! {
                _ = &mut shutdown => {
                    log::info!("shutting down...");
                    browser.terminate().await?;
                    proxy.stop();
                    log::info!("browser and proxy have been shut down");
                    return Ok(())
                },
                event = timeout( last_action_timeout.to_duration(), browser.next_event() ) => match event {
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
                            events.send(RunEvent::NewTraceEntry {
                                entry: entry.clone(),
                                violation,
                            })?;

                            hash_previous = state.transition_hash;

                            let actions =
                                available_actions(&origin, &state).await?;

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

    pub async fn shutdown(mut self) -> anyhow::Result<()> {
        self.shutdown
            .send(())
            .map_err(|_| anyhow::anyhow!("sending shutdown signal failed"))?;
        (&mut self.done).await?
    }
}

#[derive(Clone, Debug)]
pub enum Violation {
    Invariant(String),
    Unknown(Arc<anyhow::Error>),
}

impl<E: Into<anyhow::Error>> From<E> for Violation {
    fn from(value: E) -> Self {
        Violation::Unknown(Arc::new(value.into()))
    }
}

impl Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Violation::Invariant(message) => {
                write!(f, "invariant: {}", message)
            }
            Violation::Unknown(error) => {
                write!(f, "{}", error)
            }
        }
    }
}

macro_rules! invariant_violation {
    ($msg:literal $(,)?) => {
        return Result::Err(Violation::Invariant(format!("{}", $msg)))
    };
    ($err:expr $(,)?) => {
        return Result::Err(Violation::Invariant(format!("{}", $err)))
    };
    ($fmt:expr, $($arg:tt)*) => {
        return Result::Err(Violation::Invariant(format!($fmt, $($arg)*)))
    };
}

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
        match entry.level {
            ConsoleEntryLevel::Error => invariant_violation!(
                "console.error at {}: {:?}",
                entry.timestamp.duration_since(UNIX_EPOCH)?.as_micros(),
                entry.args
            ),
            _ => {}
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
