use std::fmt::Debug;
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::Result;
use serde::{Serialize, de::DeserializeOwned};
use serde_json as json;

use crate::specification::domain::Snapshot;

/// Convert a JSON value produced by a specification's action generator
/// into a validated action.
pub trait FromGeneratedAction: Sized {
    fn from_generated(value: json::Value) -> Result<Self>;
}

/// Identity conversion.
impl FromGeneratedAction for json::Value {
    fn from_generated(value: json::Value) -> Result<Self> {
        Ok(value)
    }
}

/// A driver runs a user interface of some sort (the system under test).
pub trait InterfaceDriver {
    type Action: Clone + Debug + Serialize + DeserializeOwned;
    type ActionTemplate: Clone
        + Debug
        + Serialize
        + DeserializeOwned
        + FromGeneratedAction;
    type State: Debug;

    fn initiate(&mut self) -> Result<()>;

    fn terminate(self) -> Result<()>;

    fn next_event(&mut self) -> Option<DriverEvent<Self::State>>;

    fn apply(&mut self, action: Self::Action) -> Result<()>;

    fn extract_snapshots(
        &mut self,
        state: Arc<Self::State>,
        last_action: Option<&Self::Action>,
    ) -> Result<Vec<Snapshot>>;

    fn state_timestamp(state: &Self::State) -> SystemTime;
}

#[derive(Debug, Clone)]
pub enum DriverEvent<S> {
    StateChanged(S),
    Error(Arc<anyhow::Error>),
}
