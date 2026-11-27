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
use crate::model::Picked;
use crate::specification::convert::{
    ToSchema, violation_with_pretty_functions,
};
use crate::specification::domain::Snapshot;
use crate::specification::verifier::Verifier;
use crate::tree::{Leaves, Tree};
use bombadil_trace::StepError;

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

    /// `leaves` are the actions available in `state`, as the trace records
    /// them.
    fn on_new_state(
        &mut self,
        state: &Session::State,
        leaves: &Leaves<'_, Session::ActionTemplate>,
        last_action: Option<&Session::Action>,
        snapshots: &[Snapshot],
        properties: PropertiesState,
    ) -> Result<ControlFlow<Self::StopValue, Picked<Session::Action>>>;

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

enum End<Stop> {
    Stop(Stop),
    TimeLimit,
    Stalled,
    Interrupted,
    /// The driver's error, and what the run fails with.
    Driver(String, anyhow::Error),
    Failed(anyhow::Error),
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
    let mut action_previous: Option<Picked<Session::Action>> = None;
    // Whether the trace has no state yet for `action_previous`.
    let mut action_pending = false;
    let mut violations = Vec::new();

    let mut waiting_since = Instant::now();

    // Leaves through `End`, so a pending action gets its outcome written.
    macro_rules! attempt {
        ($result:expr) => {
            match $result {
                Ok(value) => value,
                Err(error) => break End::Failed(error.into()),
            }
        };
    }

    let end = loop {
        if interrupted.load(Ordering::SeqCst) {
            break End::Interrupted;
        }
        match driver.next_event(POLL_INTERVAL) {
            Some(DriverEvent::StateChanged(state)) => {
                if antithesis::is_in_guest() {
                    // This lets the Antithesis fuzzer know of a new state in
                    // our loop, so that it can fork and change the entropy to
                    // explore the SUT.
                    antithesis_fuzzer::mark_state_boundary();
                }

                let last_action =
                    action_previous.as_ref().map(|picked| &picked.action);
                let snapshots = attempt!(
                    driver.extract_snapshots(state.clone(), last_action)
                );
                for value in snapshots.iter() {
                    log::debug!(
                        "snapshot {}: {}",
                        value.name.as_deref().unwrap_or("<unnamed>"),
                        value.value
                    );
                }

                let step_result =
                    attempt!(verifier.step::<Session::ActionTemplate>(
                        &snapshots,
                        Time::from_system_time(Session::state_timestamp(
                            &state
                        )),
                    ));

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

                let actions = strategy.constrain(&state, step_result.actions);
                let leaves = actions.leaves();

                attempt!(writer.write(
                    &state,
                    &snapshots,
                    properties.violations,
                    &leaves,
                    action_previous.as_ref(),
                ));
                action_pending = false;

                let control = attempt!(strategy.on_new_state(
                    &state,
                    &leaves,
                    last_action,
                    &snapshots,
                    properties,
                ));

                match control {
                    ControlFlow::Stop(value) => break End::Stop(value),
                    ControlFlow::Continue(picked) => {
                        log::info!("picked action: {:?}", picked.action);
                        attempt!(writer.write_action(&picked));
                        action_pending = true;
                        let applied =
                            driver.apply(picked.action.clone(), state.clone());
                        action_previous = Some(picked);
                        if let Err(error) = applied {
                            break End::Driver(format!("{error:#}"), error);
                        }
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
                    break End::TimeLimit;
                }
                if waiting_since.elapsed() >= Session::STALL_TIMEOUT {
                    log::error!(
                        "driver stalled: no event for {:?}, stopping",
                        Session::STALL_TIMEOUT
                    );
                    break End::Stalled;
                }
            }
            Some(DriverEvent::Error(error)) => {
                break End::Driver(
                    error.to_string(),
                    anyhow::anyhow!("driver error: {}", error),
                );
            }
            None => {
                break End::Driver(
                    "driver closed".to_string(),
                    anyhow::anyhow!("driver closed"),
                );
            }
        }
    };

    let (step_error, result) = match end {
        End::Stop(value) => (None, Ok(value)),
        End::TimeLimit => {
            (Some(StepError::TimeLimit), strategy.on_time_limit())
        }
        End::Stalled => (Some(StepError::Stalled), strategy.on_stalled()),
        End::Interrupted => {
            log::debug!("interrupted, stopping runner");
            (Some(StepError::Interrupted), strategy.on_interrupted())
        }
        End::Driver(message, error) => {
            (Some(StepError::Driver { message }), Err(error))
        }
        End::Failed(error) => {
            let message = format!("{error:#}");
            (Some(StepError::Failed { message }), Err(error))
        }
    };
    let written = match step_error {
        Some(step_error) if action_pending => writer.write_error(step_error),
        _ => Ok(()),
    };
    match (result, written) {
        (Ok(value), Ok(())) => Ok(value),
        (Ok(_), Err(error)) => Err(error),
        (Err(error), written) => {
            if let Err(written) = written {
                log::error!("{written:#}");
            }
            Err(error)
        }
    }
}
