use std::sync::Arc;

use anyhow::Result;
use bombadil_schema::Time;
use serde::Serialize;
use tokio::select;
use tokio::signal::ctrl_c;

use crate::driver::{DriverEvent, InterfaceDriver};
use crate::specification::convert::ToSchema;
use crate::specification::domain::Snapshot;
use crate::specification::worker::{PropertyValue, VerifierWorker};
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
    ) -> impl std::future::Future<
        Output = Result<ControlFlow<Self::StopValue, D::Action>>,
    >;

    fn on_interrupted(
        &mut self,
    ) -> impl std::future::Future<Output = Result<Self::StopValue>>;
}

pub struct Runner<D: InterfaceDriver> {
    driver: D,
    verifier: Arc<VerifierWorker>,
}

impl<D: InterfaceDriver> Runner<D> {
    pub fn new(driver: D, verifier: Arc<VerifierWorker>) -> Self {
        Self { driver, verifier }
    }

    pub async fn run<S: RunStrategy<D>>(
        mut self,
        strategy: &mut S,
    ) -> Result<Option<S::StopValue>> {
        log::info!("starting test");
        self.driver.initiate().await?;
        log::debug!("driver initiated");

        let result =
            Box::pin(Self::run_test(&mut self.driver, self.verifier, strategy))
                .await;

        log::debug!("test finished");

        self.driver
            .terminate()
            .await
            .expect("driver failed to terminate");

        result
    }

    async fn run_test<S: RunStrategy<D>>(
        driver: &mut D,
        verifier: Arc<VerifierWorker>,
        strategy: &mut S,
    ) -> Result<Option<S::StopValue>> {
        let mut last_action: Option<D::Action> = None;

        loop {
            let verifier = verifier.clone();
            let event = select! {
                event = Box::pin(driver.next_event()) => event,
                _ = ctrl_c() => {
                    let value = strategy.on_interrupted().await?;
                    return Ok(Some(value));
                }
            };
            match event {
                Some(DriverEvent::StateChanged(state)) => {
                    let snapshots: Arc<[Snapshot]> = Box::pin(
                        driver.extract_snapshots(&state, last_action.as_ref()),
                    )
                    .await?
                    .into();
                    for value in snapshots.iter() {
                        log::debug!(
                            "snapshot {}: {}",
                            value.name.as_deref().unwrap_or("<unnamed>"),
                            value.value
                        );
                    }

                    let step_result = Box::pin(verifier.step::<D::Action>(
                        snapshots.clone(),
                        Time::from_system_time(D::state_timestamp(&state)),
                    ))
                    .await?;

                    let mut violations =
                        Vec::with_capacity(step_result.properties.len());
                    for (name, value) in step_result.properties {
                        match value {
                            PropertyValue::False(violation) => {
                                violations.push(PropertyViolation {
                                    name,
                                    violation: violation.to_schema(),
                                });
                            }
                            PropertyValue::Residual | PropertyValue::True => {}
                        }
                    }

                    let control = Box::pin(strategy.on_new_state(
                        &state,
                        step_result.actions,
                        last_action.as_ref(),
                        &snapshots,
                        PropertiesState {
                            violations: &violations,
                            all_definite: step_result.all_definite,
                        },
                    ))
                    .await?;

                    match control {
                        ControlFlow::Stop(value) => return Ok(Some(value)),
                        ControlFlow::Continue(action) => {
                            log::info!("picked action: {:?}", action);
                            driver.apply(action.clone()).await?;
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
