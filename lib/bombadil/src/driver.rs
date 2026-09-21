use std::fmt::Display;
use std::sync::Arc;
use std::time::SystemTime;
use std::{fmt::Debug, hash::Hasher};

use anyhow::Result;
use bombadil_schema::Time;
use serde::{Serialize, de::DeserializeOwned};
use serde_json as json;

use crate::runner::PropertyViolation;
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

#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Default)]
pub struct RunId(pub u64);

impl Display for RunId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A driver runs a user interface of some sort (the system under test).
pub trait InterfaceDriver {
    type Session: InterfaceSession;
    fn new_session(&self, run_id: RunId) -> Result<(Self::Session, Verifier)>;
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

    fn category_hash<H: Hasher>(&self, hasher: &mut H);
}

pub trait OutputWriter<Session: InterfaceSession> {
    type TraceWriter: TraceWriter<Session>;
    fn trace_writer(&mut self, run_id: RunId) -> Result<Self::TraceWriter>;
}

pub trait TraceWriter<Session: InterfaceSession> {
    fn write(
        &mut self,
        state: &Session::State,
        last_action: Option<&Session::Action>,
        snapshots: &[Snapshot],
        violations: &[PropertyViolation],
    ) -> Result<()>;
}

impl<Session: InterfaceSession> TraceWriter<Session>
    for Box<dyn TraceWriter<Session>>
{
    fn write(
        &mut self,
        state: &<Session as InterfaceSession>::State,
        last_action: Option<&<Session as InterfaceSession>::Action>,
        snapshots: &[Snapshot],
        violations: &[PropertyViolation],
    ) -> Result<()> {
        (**self).write(state, last_action, snapshots, violations)
    }
}

pub struct NoopOutputWriter;

impl<Session: InterfaceSession> OutputWriter<Session> for NoopOutputWriter {
    type TraceWriter = NoopTraceWriter;

    fn trace_writer(&mut self, _: RunId) -> Result<Self::TraceWriter> {
        Ok(NoopTraceWriter)
    }
}

pub struct NoopTraceWriter;

impl<Session: InterfaceSession> TraceWriter<Session> for NoopTraceWriter {
    fn write(
        &mut self,
        _state: &Session::State,
        _last_action: Option<&Session::Action>,
        _snapshots: &[Snapshot],
        _violations: &[PropertyViolation],
    ) -> Result<()> {
        Ok(())
    }
}

pub trait InterfaceSession {
    type Action: Clone + Debug + Serialize + DeserializeOwned + Format;
    type ActionTemplate: Clone
        + Debug
        + Serialize
        + DeserializeOwned
        + Format
        + FromGeneratedAction
        + PartialEq
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
