use crate::browser::actions::BrowserAction;
use crate::browser::{BrowserEvent, BrowserOptions};
use crate::instrumentation::js::EDGE_MAP_SIZE;
use crate::specification::bundler::bundle;
use crate::specification::verifier::{Snapshot, Specification};
use crate::specification::worker::{PropertyValue, VerifierWorker};
use crate::trace::PropertyViolation;
use ::url::Url;
use serde_json as json;
use std::cmp::max;
use std::sync::Arc;
use std::time::Duration;

use crate::browser::state::{BrowserState, Coverage};
use crate::browser::{Browser, DebuggerOptions};
use crate::url::is_within_domain;

pub struct RunnerOptions {
    pub stop_on_violation: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlFlow<T> {
    Continue,
    Stop(T),
}

pub trait RunObserver {
    type StopValue;

    fn on_new_state(
        &mut self,
        state: &BrowserState,
        last_action: Option<&BrowserAction>,
        snapshots: &[Snapshot],
        violations: &[PropertyViolation],
    ) -> impl std::future::Future<
        Output = anyhow::Result<ControlFlow<Self::StopValue>>,
    >;
}

pub struct Runner {
    origin: Url,
    options: RunnerOptions,
    browser: Browser,
    verifier: Arc<VerifierWorker>,
}

impl Runner {
    pub async fn new(
        origin: Url,
        specification: Specification,
        options: RunnerOptions,
        browser_options: BrowserOptions,
        debugger_options: DebuggerOptions,
    ) -> anyhow::Result<Self> {
        let verifier = VerifierWorker::start(specification.clone()).await?;

        let browser =
            Browser::new(origin.clone(), browser_options, debugger_options)
                .await?;

        browser
            .ensure_script_evaluated(
                &bundle(".", &specification.module_specifier).await?,
            )
            .await?;

        Ok(Runner {
            origin,
            options,
            browser,
            verifier,
        })
    }

    pub async fn run<O: RunObserver>(
        mut self,
        observer: &mut O,
    ) -> anyhow::Result<Option<O::StopValue>> {
        log::info!("starting test of {}", self.origin);

        self.browser.initiate().await?;
        log::debug!("browser initiated");

        let result = Runner::run_test(
            &self.origin,
            self.options,
            &mut self.browser,
            self.verifier.clone(),
            observer,
        )
        .await;

        log::debug!("test finished, draining residuals");
        match self.verifier.drain_residuals().await {
            Ok(drained) => {
                for (property_name, operator) in &drained {
                    log::warn!(
                        "Property `{}` was undecided when the test ended (pending `{}`). With more time it may have resolved differently.",
                        property_name,
                        operator
                    );
                }
            }
            Err(e) => {
                log::warn!("failed to drain residuals: {}", e);
            }
        }

        self.browser
            .terminate()
            .await
            .expect("browser failed to terminate");

        result
    }

    async fn run_test<O: RunObserver>(
        origin: &Url,
        options: RunnerOptions,
        browser: &mut Browser,
        verifier: Arc<VerifierWorker>,
        observer: &mut O,
    ) -> anyhow::Result<Option<O::StopValue>> {
        let mut last_action: Option<BrowserAction> = None;
        let mut edges = [0u8; EDGE_MAP_SIZE];

        loop {
            let verifier = verifier.clone();
            let event = browser.next_event().await;
            match event {
                Some(event) => match event {
                    BrowserEvent::StateChanged(state) => {
                        // Step formulas and collect violations.
                        let snapshots: Arc<[Snapshot]> =
                            run_extractors(&state, &last_action).await?.into();
                        for value in snapshots.iter() {
                            log::debug!(
                                "snapshot {}: {}",
                                value.name.as_deref().unwrap_or("<unnamed>"),
                                value.value
                            );
                        }
                        let step_result = verifier
                            .step::<crate::specification::js::JsAction>(
                                snapshots.clone(),
                                state.timestamp,
                            )
                            .await?;

                        // Convert JsAction tree to BrowserAction tree
                        let action_tree =
                            step_result.actions.try_map(&mut |js_action| {
                                js_action.to_browser_action()
                            })?;

                        let mut violations =
                            Vec::with_capacity(step_result.properties.len());
                        for (name, value) in step_result.properties {
                            match value {
                                PropertyValue::False(violation) => {
                                    violations.push(PropertyViolation {
                                        name,
                                        violation,
                                    });
                                }
                                PropertyValue::Residual
                                | PropertyValue::True => {}
                            }
                        }
                        let has_violations = !violations.is_empty();

                        // Make sure we stay within origin.
                        let action_tree =
                            if !is_within_domain(&state.url, origin) {
                                action_tree.filter(&|a| {
                                    matches!(a, BrowserAction::Back)
                                })
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

                        let control = observer
                            .on_new_state(
                                &state,
                                last_action.as_ref(),
                                &snapshots,
                                &violations,
                            )
                            .await?;

                        if let ControlFlow::Stop(value) = control {
                            return Ok(Some(value));
                        }

                        if has_violations && options.stop_on_violation {
                            return Ok(None);
                        }
                        if !step_result.has_pending {
                            log::info!("all properties are definite, stopping");
                            return Ok(None);
                        }

                        let action_tree =
                            action_tree.prune().ok_or_else(|| {
                                anyhow::anyhow!("no actions available")
                            })?;

                        let action =
                            action_tree.pick(&mut rand::rng())?.clone();
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

async fn run_extractors(
    state: &BrowserState,
    last_action: &Option<BrowserAction>,
) -> anyhow::Result<Vec<Snapshot>> {
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

    // Update time cell in browser runtime before running extractors
    let timestamp_millis = state
        .timestamp
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis() as u64;

    state
        .evaluate_function_call::<json::Value>(
            "(timestamp) => { const { time } = __bombadilRequire('@antithesishq/bombadil'); time.update(null, timestamp); return true; }",
            vec![json::json!(timestamp_millis)],
        )
        .await?;

    let results: Vec<Snapshot> = state
            .evaluate_function_call(
                "(state) => __bombadilRequire('@antithesishq/bombadil').runtime.runExtractors({ ...state, document, window })",
                vec![state_partial.clone()],
            )
            .await?;

    Ok(results)
}

fn action_timeout(action: &BrowserAction) -> Duration {
    match action {
        BrowserAction::Back => Duration::from_secs(2),
        BrowserAction::Forward => Duration::from_secs(2),
        BrowserAction::Reload => Duration::from_secs(2),
        BrowserAction::Click { .. } => Duration::from_millis(500),
        BrowserAction::DoubleClick { delay_millis, .. } => {
            Duration::from_millis(delay_millis.saturating_add(500))
        }
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
        BrowserAction::Wait => Duration::from_millis(500),
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
