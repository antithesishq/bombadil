use std::fmt::Debug;
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::Result;
use bombadil_schema::Time;
use serde::{Serialize, de::DeserializeOwned};
use serde_json as json;

use crate::{
    render::Format,
    specification::{domain::Snapshot, verifier::Verifier},
};

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
    type Session: InterfaceSession;
    fn initiate(&self) -> Result<(Self::Session, Verifier)>;
}

pub trait RunState {
    fn timestamp(&self) -> Time;
}

pub trait ActionTemplate<Action> {
    fn generate<Rng: rand::TryRng + rand::RngExt>(
        &self,
        rng: &mut Rng,
    ) -> Action;

    fn accepts(&self, original: &Action) -> bool;
}

pub trait InterfaceSession {
    type Action: Clone + Debug + Serialize + DeserializeOwned + Format;
    type ActionTemplate: Clone
        + Debug
        + Serialize
        + DeserializeOwned
        + FromGeneratedAction
        + ActionTemplate<Self::Action>;
    type State: RunState + Debug;

    fn terminate(&mut self) -> Result<()>;

    fn next_event(&mut self) -> Option<DriverEvent<Self::State>>;

    fn apply(
        &mut self,
        action: Self::Action,
        state: Arc<Self::State>,
    ) -> Result<()>;

    fn extract_snapshots(
        &mut self,
        state: Arc<Self::State>,
        last_action: Option<&Self::Action>,
    ) -> Result<Vec<Snapshot>>;

    fn state_timestamp(state: &Self::State) -> SystemTime;
}

#[derive(Debug, Clone)]
pub enum DriverEvent<S> {
    StateChanged(Arc<S>),
    Error(Arc<anyhow::Error>),
}
