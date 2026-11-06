use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use antithesis_sdk::assert::{AssertType, assert_raw};
use anyhow::Result;
use bombadil_ltl::eval;
use bombadil_schema::Time;
use serde::Serialize;
use serde_json::json;

use crate::antithesis;
use crate::driver::{DriverEvent, InterfaceSession, TraceWriter};
use crate::specification::convert::{
    ToSchema, violation_with_pretty_functions,
};
use crate::specification::domain::Snapshot;
use crate::specification::verifier::Verifier;
use crate::tree::Tree;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlFlow<T, A> {
    Continue(A),
    Stop(T),
}

#[derive(Debug, Clone, Serialize)]
pub struct PropertyViolation {
    pub name: String,
    pub violation: bombadil_schema::Violation,
}

impl ToSchema<bombadil_schema::PropertyViolation> for PropertyViolation {
    fn to_schema(&self) -> bombadil_schema::PropertyViolation {
        bombadil_schema::PropertyViolation {
            name: self.name.clone(),
            violation: self.violation.clone(),
        }
    }
}

pub struct PropertiesState<'a> {
    pub violations: &'a [PropertyViolation],
    pub all_definite: bool,
}

pub trait RunStrategy<Session: InterfaceSession> {
    type StopValue;

    fn on_new_state(
        &mut self,
        state: &Session::State,
        tree: Tree<Session::ActionTemplate>,
        last_action: Option<&Session::Action>,
        snapshots: &[Snapshot],
        properties: PropertiesState,
    ) -> Result<ControlFlow<Self::StopValue, Session::Action>>;

    fn deadline(&self) -> Option<Time>;

    /// Called when the deadline passes while waiting for the driver.
    fn on_time_limit(&mut self) -> Result<Self::StopValue>;

    /// Called when the driver produces no event within its stall timeout.
    fn on_stalled(&mut self) -> Result<Self::StopValue>;

    fn on_interrupted(&mut self) -> Result<Self::StopValue>;

    /// Restrict the actions available in a state. The runner applies this
    /// to the action tree before passing it to `on_new_state`.
    fn constrain(
        &self,
        _state: &Session::State,
        tree: Tree<Session::ActionTemplate>,
    ) -> Tree<Session::ActionTemplate> {
        tree
    }
}

pub trait RunState {
    fn timestamp(&self) -> Time;
}

const POLL_INTERVAL: Duration = Duration::from_millis(200);

pub fn run<
    Session: InterfaceSession,
    Strategy: RunStrategy<Session>,
    Writer: TraceWriter<Session>,
>(
    session: &mut Session,
    strategy: &mut Strategy,
    verifier: Verifier,
    writer: &mut Writer,
    interrupted: Arc<AtomicBool>,
) -> Result<Strategy::StopValue> {
    log::info!("starting test");
    log::debug!("driver initiated");

    let result = run_test(session, verifier, writer, interrupted, strategy);

    // Even after a failed run, so the trace of what happened is complete.
    let finished = writer.finish();
    let terminated = session.terminate();

    log::debug!("test finished");

    // The first error is returned, the run's before the others, and any
    // others are logged rather than dropped. If the run failed, `finish`
    // failing because writing had failed repeats the run's error, so it
    // isn't logged again.
    let mut errors = [
        finished
            .err()
            .filter(|error| {
                result.is_ok()
                    || !error
                        .to_string()
                        .starts_with(bombadil_trace::WRITER_FAILED_EARLIER)
            })
            .map(|error| error.context("failed to finish the trace")),
        terminated
            .err()
            .map(|error| error.context("failed to terminate the session")),
    ]
    .into_iter()
    .flatten();
    let first = match result {
        Ok(value) => match errors.next() {
            None => return Ok(value),
            Some(error) => error,
        },
        Err(error) => error,
    };
    for error in errors {
        log::error!("{error:#}");
    }
    Err(first)
}

fn run_test<
    Session: InterfaceSession,
    Strategy: RunStrategy<Session>,
    Writer: TraceWriter<Session>,
>(
    driver: &mut Session,
    mut verifier: Verifier,
    writer: &mut Writer,
    interrupted: Arc<AtomicBool>,
    strategy: &mut Strategy,
) -> Result<Strategy::StopValue> {
    let mut last_action: Option<Session::Action> = None;
    let mut violations = Vec::new();

    let mut waiting_since = Instant::now();

    while !interrupted.load(Ordering::SeqCst) {
        match driver.next_event(POLL_INTERVAL) {
            Some(DriverEvent::StateChanged(state)) => {
                if antithesis::is_in_guest() {
                    // This lets the Antithesis fuzzer know of a new state in
                    // our loop, so that it can fork and change the entropy to
                    // explore the SUT.
                    antithesis_fuzzer::mark_state_boundary();
                }

                let snapshots = driver
                    .extract_snapshots(state.clone(), last_action.as_ref())?;
                for value in snapshots.iter() {
                    log::debug!(
                        "snapshot {}: {}",
                        value.name.as_deref().unwrap_or("<unnamed>"),
                        value.value
                    );
                }

                let step_result = verifier.step::<Session::ActionTemplate>(
                    &snapshots,
                    Time::from_system_time(Session::state_timestamp(&state)),
                )?;

                violations.clear();
                for (name, value) in step_result.properties() {
                    let (condition, hit, details) = match value {
                        eval::Value::False(violation, _) => {
                            let violation =
                                violation_with_pretty_functions(violation)
                                    .to_schema();
                            violations.push(PropertyViolation {
                                name: name.clone(),
                                violation: violation.clone(),
                            });
                            (false, true, json!({ "violation": violation }))
                        }
                        eval::Value::Residual(_) | eval::Value::True(_) => {
                            (true, true, json!({}))
                        }
                    };

                    // Catalog properties, or report their violations, to Antithesis.
                    assert_raw(
                        condition,
                        name.clone(),
                        &details,
                        "".into(),
                        "".into(),
                        "".into(),
                        0,
                        0,
                        hit,  // hit
                        true, // must_hit
                        AssertType::Always,
                        "Bombadil property".into(),
                        name.clone(),
                    );
                }

                let properties = PropertiesState {
                    violations: &violations,
                    all_definite: step_result.all_definite,
                };

                writer.write(
                    &state,
                    last_action.as_ref(),
                    &snapshots,
                    properties.violations,
                )?;

                let actions = strategy.constrain(&state, step_result.actions);
                let control = strategy.on_new_state(
                    &state,
                    actions,
                    last_action.as_ref(),
                    &snapshots,
                    properties,
                )?;

                match control {
                    ControlFlow::Stop(value) => return Ok(value),
                    ControlFlow::Continue(action) => {
                        log::info!("picked action: {:?}", action);
                        driver.apply(action.clone(), state.clone())?;
                        last_action = Some(action);
                        waiting_since = Instant::now();
                    }
                }
            }
            Some(DriverEvent::Timeout) => {
                if strategy.deadline().is_some_and(|deadline| {
                    Time::from_system_time(SystemTime::now()) >= deadline
                }) {
                    log::warn!(
                        "time limit reached while waiting for driver, stopping"
                    );
                    return strategy.on_time_limit();
                }
                if waiting_since.elapsed() >= Session::STALL_TIMEOUT {
                    log::error!(
                        "driver stalled: no event for {:?}, stopping",
                        Session::STALL_TIMEOUT
                    );
                    return strategy.on_stalled();
                }
            }
            Some(DriverEvent::Error(error)) => {
                anyhow::bail!("driver error: {}", error);
            }
            None => {
                anyhow::bail!("driver closed");
            }
        }
    }
    log::debug!("interrupted, stopping runner");
    strategy.on_interrupted()
}
