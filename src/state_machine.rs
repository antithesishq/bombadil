use anyhow::{Error, Result};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub enum Event<State> {
    StateChanged(State),
    Error(Arc<Error>),
}

pub trait StateMachine {
    type State;
    type Action;

    fn initiate(&mut self) -> impl Future<Output = Result<()>>;
    fn terminate(self) -> impl Future<Output = Result<()>>;
    fn next_event(
        &mut self,
    ) -> impl Future<Output = Option<Event<Self::State>>>;
    fn request_state(&mut self) -> impl Future<Output = ()>;
    fn apply(
        &mut self,
        action: Self::Action,
    ) -> impl Future<Output = Result<()>>;
}
