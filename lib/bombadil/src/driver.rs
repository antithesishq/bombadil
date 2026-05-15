use std::fmt::Debug;
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::Result;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::specification::domain::Snapshot;
use crate::tree::Tree;

/// A driver implements the interface a Runner uses to drive any underlying
/// system under test — a browser, a terminal, anything that can produce
/// observable states and accept actions.
///
/// `Action` is the internal, validated action form the driver knows how to
/// apply. `JsAction` is the JS-friendly form the specification layer
/// emits via JSON.
/// Drivers convert between them in [`js_action_to_action`]; for drivers
/// where the two forms are the same, both types can be identical with a
/// trivial conversion.
/// The Runner awaits driver futures in place (it Box::pins them but
/// never `tokio::spawn`s), so the futures themselves need not be Send.
/// Some drivers — notably the terminal one — hold !Sync resources
/// (libghostty's raw pointers, single-threaded Boa contexts) and could
/// not satisfy a Send bound here. Self itself is still required to be
/// Send so Runner can be moved across awaits.
pub trait InterfaceDriver: Send {
    type Action: Clone + Debug + Serialize + Send + 'static;
    type JsAction: DeserializeOwned + Debug + Send + 'static;
    type State: Debug + Send + 'static;

    fn initiate(&mut self) -> impl std::future::Future<Output = Result<()>>;

    fn terminate(self) -> impl std::future::Future<Output = Result<()>>;

    fn next_event(
        &mut self,
    ) -> impl std::future::Future<Output = Option<DriverEvent<Self::State>>>;

    fn apply(&mut self, action: Self::Action) -> Result<()>;

    /// Run user-defined extractors against the current state and return
    /// snapshots. Each driver owns its extractor execution: the browser
    /// dispatches to Chromium via CDP, while other drivers may spin up a
    /// private Boa context. The core runner stays unaware of this.
    fn extract_snapshots(
        &self,
        state: &Self::State,
        last_action: Option<&Self::Action>,
    ) -> impl std::future::Future<Output = Result<Vec<Snapshot>>>;

    /// Convert a JS-emitted action (as deserialized from the
    /// specification) into a validated internal action.
    fn js_action_to_action(js: Self::JsAction) -> Result<Self::Action>;

    /// Extract the observation timestamp from a state.
    fn state_timestamp(state: &Self::State) -> SystemTime;

    /// Optional hook to filter the action tree based on the current state.
    /// Default: passes the tree through unchanged.
    fn filter_actions(
        &self,
        _state: &Self::State,
        tree: Tree<Self::Action>,
    ) -> Tree<Self::Action> {
        tree
    }

    /// Optional hook called after each state observation. Drivers can use
    /// this to update their own bookkeeping (coverage, transition tracking,
    /// etc.) without burdening the generic runner.
    fn observe_state(&mut self, _state: &Self::State) {}
}

#[derive(Debug, Clone)]
pub enum DriverEvent<S> {
    StateChanged(S),
    Error(Arc<anyhow::Error>),
}
