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
pub enum ControlFlow<T> {
    Continue,
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

pub trait RunStrategy<D: InterfaceDriver> {
    type StopValue;

    fn on_new_state(
        &mut self,
        state: &D::State,
        last_action: Option<&D::Action>,
        snapshots: &[Snapshot],
        violations: &[PropertyViolation],
    ) -> impl std::future::Future<Output = Result<ControlFlow<Self::StopValue>>>;

    fn pick_action(
        &mut self,
        tree: Tree<D::Action>,
    ) -> impl std::future::Future<Output = Result<D::Action>>;

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

        // Box::pin the inner loop so its (potentially large) future is
        // heap-allocated rather than living on the test thread's modest
        // 2 MB stack.
        let result = Box::pin(Self::run_test(
            &mut self.driver,
            self.verifier,
            strategy,
        ))
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
            // Box::pin each in-loop trait-method await so the per-iteration
            // future state stays small. With three async-in-trait calls
            // (next_event, extract_snapshots, on_new_state, pick_action)
            // plus the verifier worker step, the inlined state machine
            // grows past the test thread's 2 MB stack on heavier specs.
            let event = select! {
                event = Box::pin(driver.next_event()) => event,
                _ = ctrl_c() => {
                    let value = strategy.on_interrupted().await?;
                    return Ok(Some(value));
                }
            };
            match event {
                Some(DriverEvent::StateChanged(state)) => {
                    driver.observe_state(&state);

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

                    let step_result = Box::pin(verifier.step::<D::JsAction>(
                        snapshots.clone(),
                        Time::from_system_time(D::state_timestamp(&state)),
                    ))
                    .await?;

                    let action_tree = step_result
                        .actions
                        .try_map(&mut |js| D::js_action_to_action(js))?;

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

                    let action_tree =
                        driver.filter_actions(&state, action_tree);

                    let control = Box::pin(strategy.on_new_state(
                        &state,
                        last_action.as_ref(),
                        &snapshots,
                        &violations,
                    ))
                    .await?;

                    if let ControlFlow::Stop(value) = control {
                        return Ok(Some(value));
                    }

                    if !step_result.has_pending {
                        log::info!("all properties are definite, stopping");
                        return Ok(None);
                    }

                    let action_tree = action_tree.prune().ok_or_else(|| {
                        anyhow::anyhow!("no actions available")
                    })?;

                    let action =
                        Box::pin(strategy.pick_action(action_tree)).await?;
                    log::info!("picked action: {:?}", action);
                    driver.apply(action.clone())?;
                    last_action = Some(action);
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
