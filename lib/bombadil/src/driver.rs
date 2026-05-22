use std::fmt::Debug;
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::Result;
use serde::Serialize;
use serde_json as json;

use crate::specification::domain::Snapshot;

/// Convert a JSON value produced by a specification's action generator
/// into a validated action.
pub trait FromGeneratedAction: Sized {
    fn from_generated(value: json::Value) -> Result<Self>;
}

/// A driver runs a user interface of some sort (the system under test).
pub trait InterfaceDriver: Send {
    type Action: Clone
        + Debug
        + Serialize
        + FromGeneratedAction
        + Send
        + 'static;
    type State: Debug + Send + 'static;

    fn initiate(&mut self) -> impl std::future::Future<Output = Result<()>>;

    fn terminate(self) -> impl std::future::Future<Output = Result<()>>;

    fn next_event(
        &mut self,
    ) -> impl std::future::Future<Output = Option<DriverEvent<Self::State>>>;

    fn apply(
        &mut self,
        action: Self::Action,
    ) -> impl std::future::Future<Output = Result<()>>;

    fn extract_snapshots(
        &self,
        state: &Self::State,
        last_action: Option<&Self::Action>,
    ) -> impl std::future::Future<Output = Result<Vec<Snapshot>>>;

    fn state_timestamp(state: &Self::State) -> SystemTime;
}

#[derive(Debug, Clone)]
pub enum DriverEvent<S> {
    StateChanged(S),
    Error(Arc<anyhow::Error>),
}
