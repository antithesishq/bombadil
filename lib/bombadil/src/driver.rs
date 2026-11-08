use std::collections::BTreeMap;
use std::fmt::Display;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use std::{fmt::Debug, hash::Hasher};

use anyhow::Result;
use bombadil_schema::Time;
use serde::{Serialize, de::DeserializeOwned};
use serde_json as json;

use crate::model::StepModel;
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

    /// Named similarity hashes (64-bit simhashes) of the state, e.g.
    /// `screen` or `coverage`, as the driver defines them. Training combines
    /// them into one state simhash (see `crate::model`).
    fn hashes(&self) -> BTreeMap<String, u64> {
        BTreeMap::new()
    }
}

pub trait ActionTemplate<Action> {
    fn generate<Rng: rand::TryRng + rand::RngExt>(
        &self,
        rng: &mut Rng,
    ) -> Action;

    fn accepts(&self, original: &Action) -> bool;

    /// Whether this template is the one `original` was generated from. Stricter
    /// than `accepts`, which replay needs: two links to one page accept each
    /// other's clicks. Default: `accepts`.
    fn produced(&self, original: &Action) -> bool {
        self.accepts(original)
    }

    fn category_hash<H: Hasher>(&self, hasher: &mut H);

    /// The full template as JSON, written once per action key to the table
    /// `--action-templates` produces. It carries all of the template's
    /// parameters, so that tooling can split a coarse template into finer ones
    /// from the table and the trace alone.
    fn to_json(&self) -> serde_json::Value;

    /// Whether `self`'s generatable set contains all of `other`'s. Overlap
    /// serving uses it to match an offered (coarse) template against a learned
    /// (fine) one.
    fn contains(&self, other: &Self) -> bool;
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
        model: &StepModel,
    ) -> Result<()>;

    /// Completes the trace after the last state, reporting any failure to
    /// write it. Called once by the runner when a run ends.
    fn finish(&mut self) -> Result<()> {
        Ok(())
    }
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
        model: &StepModel,
    ) -> Result<()> {
        (**self).write(state, last_action, snapshots, violations, model)
    }

    fn finish(&mut self) -> Result<()> {
        (**self).finish()
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
        _model: &StepModel,
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

    /// Amount of time we'll wait for `next_event` to return an event
    /// before considering the driver stalled.
    const STALL_TIMEOUT: Duration;

    fn terminate(&mut self) -> Result<()>;

    fn next_event(
        &mut self,
        timeout: Duration,
    ) -> Option<DriverEvent<Self::State>>;

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
    Timeout,
}
