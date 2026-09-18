use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

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

    fn on_interrupted(&mut self) -> Result<Self::StopValue>;
}

pub trait RunState {
    fn timestamp(&self) -> Time;
}

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

    session.terminate()?;

    log::debug!("test finished");

    result
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

    while !interrupted.load(Ordering::SeqCst) {
        let event = driver.next_event();

        if antithesis::is_in_guest() {
            // This lets the Antithesis fuzzer know of a new state in our loop,
            // so that it can fork and change the entropy to explore the SUT.
            antithesis_fuzzer::mark_state_boundary();
        }

        match event {
            Some(DriverEvent::StateChanged(state)) => {
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

                let control = strategy.on_new_state(
                    &state,
                    step_result.actions,
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
                    }
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
