use serde_json as json;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

use crate::specification::ltl::{self, PrettyFunction};
use crate::specification::result::SpecificationError;
use crate::specification::verifier::Verifier;

enum Command {
    GetProperties {
        reply: oneshot::Sender<Vec<String>>,
    },
    GetExtractors {
        reply: oneshot::Sender<Result<Vec<(u64, String)>, SpecificationError>>,
    },

    Step {
        snapshots: Vec<(u64, json::Value)>,
        time: ltl::Time,
        reply: oneshot::Sender<
            Result<Vec<(String, PropertyValue)>, SpecificationError>,
        >,
    },
}

#[derive(Debug, Clone)]
pub enum PropertyValue {
    True,
    False(ltl::Violation<PrettyFunction>),
    Residual,
}

impl From<&ltl::Value> for PropertyValue {
    fn from(value: &ltl::Value) -> Self {
        match value {
            ltl::Value::True => PropertyValue::True,
            ltl::Value::False(violation) => {
                PropertyValue::False(violation.with_pretty_functions())
            }
            ltl::Value::Residual(_) => PropertyValue::Residual,
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
    /// Call this once at startup and share the `Arc<WorkerHandle>` as needed.
    pub fn start(
        specification_source: Vec<u8>,
    ) -> Result<Arc<Self>, SpecificationError> {
        let (tx, mut rx) = mpsc::channel::<Command>(32);
        let handle = Arc::new(VerifierWorker { tx });

        let _worker_thread = std::thread::spawn(move || {
            let mut verifier = match Verifier::new(specification_source) {
                Ok(verifier) => verifier,
                // TODO: send this error back instead, somehow
                Err(error) => panic!("specification error: {}", error),
            };
            while let Some(command) = rx.blocking_recv() {
                match command {
                    Command::GetProperties { reply } => {
                        let _ = reply.send(verifier.properties());
                    }
                    Command::GetExtractors { reply } => {
                        let _ = reply.send(verifier.extractors());
                    }
                    Command::Step {
                        snapshots,
                        time,
                        reply,
                    } => {
                        let _ = reply.send(verifier.step(snapshots, time).map(
                            |values| {
                                values
                                    .iter()
                                    .map(|(key, value)| {
                                        (
                                            key.clone(),
                                            PropertyValue::from(value),
                                        )
                                    })
                                    .collect()
                            },
                        ));
                    }
                }
            }
        });

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
    pub async fn extractors(&self) -> Result<Vec<(u64, String)>, WorkerError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(Command::GetExtractors { reply: reply_tx })
            .await
            .map_err(|_| WorkerError::WorkerGone)?;
        reply_rx
            .await
            .map_err(|_| WorkerError::WorkerGone)
            .and_then(|result| result.map_err(WorkerError::SpecificationError))
    }
    pub async fn step(
        &self,
        snapshots: Vec<(u64, json::Value)>,
        time: ltl::Time,
    ) -> Result<Vec<(String, PropertyValue)>, WorkerError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(Command::Step {
                reply: reply_tx,
                snapshots,
                time,
            })
            .await
            .map_err(|_| WorkerError::WorkerGone)?;
        reply_rx
            .await
            .map_err(|_| WorkerError::WorkerGone)
            .and_then(|result| result.map_err(WorkerError::SpecificationError))
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
