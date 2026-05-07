use serde::de::DeserializeOwned;
use serde_json as json;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

use bombadil_ltl::eval;
use bombadil_ltl::violation;
use bombadil_schema::Time;

use crate::specification::convert::{
    PrettyFunction, violation_with_pretty_functions,
};
use crate::specification::domain::{BombadilDomain, Snapshot};
use crate::specification::js::RuntimeFunction;
use crate::specification::result::SpecificationError;
use crate::specification::verifier::{Specification, Verifier};
use crate::tree::Tree;

enum Command {
    GetProperties {
        reply: oneshot::Sender<Vec<String>>,
    },
    Step {
        snapshots: Arc<[Snapshot]>,
        time: Time,
        reply: oneshot::Sender<Result<RawStepResult, SpecificationError>>,
    },
}

struct RawStepResult {
    properties: Vec<(String, PropertyValue)>,
    actions: Tree<json::Value>,
    has_pending: bool,
}

#[derive(Debug, Clone)]
pub struct StepResult<A> {
    pub properties: Vec<(String, PropertyValue)>,
    pub actions: Tree<A>,
    pub has_pending: bool,
}

#[derive(Debug, Clone)]
pub enum PropertyValue {
    True,
    False(violation::Violation<BombadilDomain<PrettyFunction>>),
    Residual,
}

impl From<&eval::Value<BombadilDomain<RuntimeFunction>>> for PropertyValue {
    fn from(value: &eval::Value<BombadilDomain<RuntimeFunction>>) -> Self {
        match value {
            eval::Value::True(_) => PropertyValue::True,
            eval::Value::False(violation, _) => {
                PropertyValue::False(violation_with_pretty_functions(violation))
            }
            eval::Value::Residual(_) => PropertyValue::Residual,
        }
    }
}

#[derive(Clone)]
pub struct VerifierWorker {
    tx: mpsc::Sender<Command>,
}

impl VerifierWorker {
    /// Starts the worker on its own OS thread and returns a handle.
    ///
    /// Call this once at startup and share the handle as needed.
    pub async fn start(
        specification: Specification,
    ) -> Result<Arc<Self>, SpecificationError> {
        use crate::specification::bundler::bundle;

        let bundle_code = bundle(".", &specification.module_specifier)
            .await
            .map_err(|e| {
                SpecificationError::OtherError(format!(
                    "Failed to bundle specification: {}",
                    e
                ))
            })?;

        let (ready_tx, ready_rx) =
            oneshot::channel::<Result<(), SpecificationError>>();

        let (tx, mut rx) = mpsc::channel::<Command>(32);
        let handle = Arc::new(VerifierWorker { tx });

        let _worker_thread = std::thread::Builder::new()
            .stack_size(16 * 1024 * 1024) // 16MB stack to avoid overflows
            .spawn(move || {
                let mut verifier = match Verifier::new(&bundle_code) {
                    Ok(verifier) => {
                        let _ = ready_tx.send(Ok(()));
                        verifier
                    }
                    Err(error) => {
                        let _ = ready_tx.send(Err(error));
                        return;
                    }
                };
                while let Some(command) = rx.blocking_recv() {
                    match command {
                        Command::GetProperties { reply } => {
                            let _ = reply.send(verifier.properties());
                        }
                        Command::Step {
                            snapshots,
                            time,
                            reply,
                        } => {
                            let _ = reply.send(
                                verifier
                                    .step::<json::Value>(&snapshots, time)
                                    .map(|result| RawStepResult {
                                        properties: result
                                            .properties
                                            .iter()
                                            .map(|(key, value)| {
                                                (
                                                    key.clone(),
                                                    PropertyValue::from(value),
                                                )
                                            })
                                            .collect(),
                                        actions: result.actions,
                                        has_pending: result.has_pending,
                                    }),
                            );
                        }
                    }
                }
            });

        ready_rx.await.map_err(|error| {
            SpecificationError::OtherError(format!(
                "worker failed to receive ready signal: {}",
                error
            ))
        })??;
        Ok(handle)
    }

    pub async fn properties(&self) -> Result<Vec<String>, WorkerError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(Command::GetProperties { reply: reply_tx })
            .await
            .map_err(|_| WorkerError::WorkerGone)?;
        reply_rx.await.map_err(|_| WorkerError::WorkerGone)
    }

    pub async fn step<A: DeserializeOwned>(
        &self,
        snapshots: Arc<[Snapshot]>,
        time: Time,
    ) -> Result<StepResult<A>, WorkerError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(Command::Step {
                reply: reply_tx,
                snapshots,
                time,
            })
            .await
            .map_err(|_| WorkerError::WorkerGone)?;
        let result = reply_rx
            .await
            .map_err(|_| WorkerError::WorkerGone)?
            .map_err(WorkerError::SpecificationError)?;
        let actions = result.actions.try_map(&mut |v| {
            json::from_value(v).map_err(|e| {
                WorkerError::SpecificationError(SpecificationError::OtherError(
                    format!("failed to deserialize action: {}", e),
                ))
            })
        })?;
        Ok(StepResult {
            properties: result.properties,
            actions,
            has_pending: result.has_pending,
        })
    }
}

#[derive(Debug)]
pub enum WorkerError {
    WorkerGone,
    SpecificationError(SpecificationError),
}

impl std::fmt::Display for WorkerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkerError::WorkerGone => write!(f, "WorkerGone"),
            WorkerError::SpecificationError(specification_error) => {
                specification_error.fmt(f)
            }
        }
    }
}

impl std::error::Error for WorkerError {}
