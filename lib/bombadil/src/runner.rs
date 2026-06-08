use std::sync::Arc;

use anyhow::Result;
use bombadil_ltl::eval;
use bombadil_schema::Time;
use serde::Serialize;

use crate::driver::{DriverEvent, InterfaceDriver};
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

pub trait RunStrategy<D: InterfaceDriver> {
    type StopValue;

    fn on_new_state(
        &mut self,
        state: &D::State,
        tree: Tree<D::Action>,
        last_action: Option<&D::Action>,
        snapshots: &[Snapshot],
        properties: PropertiesState,
    ) -> Result<ControlFlow<Self::StopValue, D::Action>>;

    fn on_interrupted(&mut self) -> Result<Self::StopValue>;
}

pub struct Runner<D: InterfaceDriver> {
    driver: D,
    verifier: Verifier,
}

impl<D: InterfaceDriver> Runner<D> {
    pub fn new(driver: D, verifier: Verifier) -> Self {
        Self { driver, verifier }
    }

    pub fn run<S: RunStrategy<D>>(
        mut self,
        strategy: &mut S,
    ) -> Result<Option<S::StopValue>> {
        log::info!("starting test");
        self.driver.initiate()?;
        log::debug!("driver initiated");

        let result = Self::run_test(&mut self.driver, self.verifier, strategy);

        log::debug!("test finished");

        self.driver.terminate().expect("driver failed to terminate");

        result
    }

    fn run_test<S: RunStrategy<D>>(
        driver: &mut D,
        mut verifier: Verifier,
        strategy: &mut S,
    ) -> Result<Option<S::StopValue>> {
        let mut last_action: Option<D::Action> = None;

        loop {
            let event = driver.next_event();
            match event {
                Some(DriverEvent::StateChanged(state)) => {
                    let state = Arc::new(state);
                    let snapshots: Arc<[Snapshot]> = driver
                        .extract_snapshots(state.clone(), last_action.as_ref())?
                        .into();
                    for value in snapshots.iter() {
                        log::debug!(
                            "snapshot {}: {}",
                            value.name.as_deref().unwrap_or("<unnamed>"),
                            value.value
                        );
                    }

                    let step_result = verifier.step::<D::Action>(
                        &snapshots,
                        Time::from_system_time(D::state_timestamp(&state)),
                    )?;

                    let mut violations =
                        Vec::with_capacity(step_result.properties.len());
                    for (name, value) in step_result.properties {
                        match value {
                            eval::Value::False(violation, _) => {
                                violations.push(PropertyViolation {
                                    name,
                                    violation: violation_with_pretty_functions(
                                        &violation,
                                    )
                                    .to_schema(),
                                });
                            }
                            eval::Value::Residual(_) | eval::Value::True(_) => {
                            }
                        }
                    }

                    let control = strategy.on_new_state(
                        &state,
                        step_result.actions,
                        last_action.as_ref(),
                        &snapshots,
                        PropertiesState {
                            violations: &violations,
                            all_definite: step_result.all_definite,
                        },
                    )?;

                    match control {
                        ControlFlow::Stop(value) => return Ok(Some(value)),
                        ControlFlow::Continue(action) => {
                            log::info!("picked action: {:?}", action);
                            driver.apply(action.clone())?;
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
    }
}
