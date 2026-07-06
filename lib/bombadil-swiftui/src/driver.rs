use std::ops::RangeInclusive;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::{Result, anyhow};
use bombadil::driver::{DriverEvent, InterfaceDriver};
use bombadil::specification::bundler::bundle;
use bombadil::specification::convert::{ToInternal, ToSchema};
use bombadil::specification::domain::Snapshot;
use bombadil::specification::generators::StringGenerator;
use bombadil::specification::verifier::{Specification, Verifier};
use bombadil_schema::swiftui;
use serde::{Deserialize, Serialize};

use crate::agent::{
    AgentConnection, AgentMessage, DriverMessage, SwiftUITarget,
};
use crate::extractors::Extractors;
use crate::state::SwiftUIState;

/// An action against the app, generic over the number and text types so
/// the same shape serves both concrete actions and templates.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum SwiftUIAction<F64 = f64, Text = String> {
    /// Tap at a point in screen coordinates.
    Tap { x: F64, y: F64 },
    /// Type text into the focused element.
    TypeText { text: Text },
    /// Press a named key, e.g. "return", "tab", "escape", "delete",
    /// "up", "down", "left", "right".
    PressKey { key: String },
    /// Scroll upwards at a point by `distance` points.
    ScrollUp { x: F64, y: F64, distance: F64 },
    /// Scroll downwards at a point by `distance` points.
    ScrollDown { x: F64, y: F64, distance: F64 },
}

pub type SwiftUIActionTemplate =
    SwiftUIAction<RangeInclusive<f64>, StringGenerator>;

impl SwiftUIActionTemplate {
    pub fn generate<Rng: rand::TryRng + rand::RngExt>(
        &self,
        rng: &mut Rng,
    ) -> SwiftUIAction {
        match self {
            SwiftUIAction::Tap { x, y } => SwiftUIAction::Tap {
                x: rng.random_range(x.clone()),
                y: rng.random_range(y.clone()),
            },
            SwiftUIAction::TypeText { text } => SwiftUIAction::TypeText {
                text: text.generate(rng),
            },
            SwiftUIAction::PressKey { key } => {
                SwiftUIAction::PressKey { key: key.clone() }
            }
            SwiftUIAction::ScrollUp { x, y, distance } => {
                SwiftUIAction::ScrollUp {
                    x: rng.random_range(x.clone()),
                    y: rng.random_range(y.clone()),
                    distance: rng.random_range(distance.clone()),
                }
            }
            SwiftUIAction::ScrollDown { x, y, distance } => {
                SwiftUIAction::ScrollDown {
                    x: rng.random_range(x.clone()),
                    y: rng.random_range(y.clone()),
                    distance: rng.random_range(distance.clone()),
                }
            }
        }
    }

    pub fn accepts(&self, original: &SwiftUIAction) -> bool {
        match (self, original) {
            (
                SwiftUIAction::Tap { x, y },
                SwiftUIAction::Tap { x: ox, y: oy },
            ) => x.contains(ox) && y.contains(oy),
            (
                SwiftUIAction::TypeText { text },
                SwiftUIAction::TypeText { text: original },
            ) => text.accepts(original),
            (
                SwiftUIAction::PressKey { key },
                SwiftUIAction::PressKey { key: original },
            ) => key == original,
            (
                SwiftUIAction::ScrollUp { x, y, distance },
                SwiftUIAction::ScrollUp {
                    x: ox,
                    y: oy,
                    distance: od,
                },
            )
            | (
                SwiftUIAction::ScrollDown { x, y, distance },
                SwiftUIAction::ScrollDown {
                    x: ox,
                    y: oy,
                    distance: od,
                },
            ) => x.contains(ox) && y.contains(oy) && distance.contains(od),
            _ => false,
        }
    }
}

impl ToSchema<swiftui::SwiftUIAction> for SwiftUIAction {
    fn to_schema(&self) -> swiftui::SwiftUIAction {
        match self {
            SwiftUIAction::Tap { x, y } => {
                swiftui::SwiftUIAction::Tap { x: *x, y: *y }
            }
            SwiftUIAction::TypeText { text } => {
                swiftui::SwiftUIAction::TypeText { text: text.clone() }
            }
            SwiftUIAction::PressKey { key } => {
                swiftui::SwiftUIAction::PressKey { key: key.clone() }
            }
            SwiftUIAction::ScrollUp { x, y, distance } => {
                swiftui::SwiftUIAction::ScrollUp {
                    x: *x,
                    y: *y,
                    distance: *distance,
                }
            }
            SwiftUIAction::ScrollDown { x, y, distance } => {
                swiftui::SwiftUIAction::ScrollDown {
                    x: *x,
                    y: *y,
                    distance: *distance,
                }
            }
        }
    }
}

impl ToInternal<SwiftUIAction> for swiftui::SwiftUIAction {
    fn to_internal(&self) -> SwiftUIAction {
        match self {
            swiftui::SwiftUIAction::Tap { x, y } => {
                SwiftUIAction::Tap { x: *x, y: *y }
            }
            swiftui::SwiftUIAction::TypeText { text } => {
                SwiftUIAction::TypeText { text: text.clone() }
            }
            swiftui::SwiftUIAction::PressKey { key } => {
                SwiftUIAction::PressKey { key: key.clone() }
            }
            swiftui::SwiftUIAction::ScrollUp { x, y, distance } => {
                SwiftUIAction::ScrollUp {
                    x: *x,
                    y: *y,
                    distance: *distance,
                }
            }
            swiftui::SwiftUIAction::ScrollDown { x, y, distance } => {
                SwiftUIAction::ScrollDown {
                    x: *x,
                    y: *y,
                    distance: *distance,
                }
            }
        }
    }
}

pub struct SwiftUIDriver {
    extractor: Extractors,
    connection: AgentConnection,
    quiescence_timeout: Duration,
    state_timeout: Duration,
    last_action: Option<SwiftUIAction>,
}

impl SwiftUIDriver {
    pub fn launch(
        specification: Specification,
        target: SwiftUITarget,
        connect_timeout: Duration,
        quiescence_timeout: Duration,
        state_timeout: Duration,
    ) -> Result<(Self, Verifier)> {
        let bundle_code = bundle(".", &specification.module_specifier)
            .map_err(|e| anyhow!("bundle failed: {e}"))?;

        let extractor = Extractors::initialize(&bundle_code)?;
        let verifier = Verifier::new(&bundle_code)?;

        let connection = AgentConnection::establish(&target, connect_timeout)?;

        Ok((
            Self {
                extractor,
                connection,
                quiescence_timeout,
                state_timeout,
                last_action: None,
            },
            verifier,
        ))
    }

    fn exited_state(
        &self,
        exit_status: swiftui::ProcessExitStatus,
    ) -> SwiftUIState {
        SwiftUIState {
            timestamp: SystemTime::now(),
            root: None,
            exit_status: Some(exit_status),
            last_action: self.last_action.clone(),
        }
    }

    fn request_state(&mut self) -> Result<SwiftUIState> {
        if let Some(exit_status) = self.connection.exit_status()? {
            return Ok(self.exited_state(exit_status));
        }

        self.connection.send(&DriverMessage::GetState {
            quiescence_millis: self.quiescence_timeout.as_millis() as u64,
        })?;

        match self.connection.receive(self.state_timeout) {
            Ok(AgentMessage::State { root }) => Ok(SwiftUIState {
                timestamp: SystemTime::now(),
                root,
                exit_status: None,
                last_action: self.last_action.clone(),
            }),
            Ok(AgentMessage::Error { message }) => {
                Err(anyhow!("agent failed to produce a state: {message}"))
            }
            Ok(other) => {
                Err(anyhow!("expected state from agent, got {other:?}"))
            }
            // The app may have exited between the check above and the
            // read — e.g. the last action crashed it. That's a regular
            // terminal state, not a driver error.
            Err(error) => match self.connection.exit_status()? {
                Some(exit_status) => Ok(self.exited_state(exit_status)),
                None => Err(error),
            },
        }
    }
}

impl InterfaceDriver for SwiftUIDriver {
    type Action = SwiftUIAction;
    type ActionTemplate = SwiftUIActionTemplate;
    type State = SwiftUIState;

    fn initiate(&mut self) -> Result<()> {
        Ok(())
    }

    fn terminate(mut self) -> Result<()> {
        self.connection.kill();
        Ok(())
    }

    fn next_event(&mut self) -> Option<DriverEvent<SwiftUIState>> {
        match self.request_state() {
            Ok(state) => Some(DriverEvent::StateChanged(state)),
            Err(error) => Some(DriverEvent::Error(Arc::new(error))),
        }
    }

    fn apply(&mut self, action: SwiftUIAction) -> Result<()> {
        self.connection.send(&DriverMessage::Apply {
            action: action.to_schema(),
        })?;
        match self.connection.receive(self.state_timeout) {
            Ok(AgentMessage::Applied {}) => {}
            Ok(AgentMessage::Error { message }) => {
                // A failed action (e.g. a tap that hit no interactive
                // element) is part of fuzzing, not a reason to stop.
                log::warn!("agent could not apply {action:?}: {message}");
            }
            Ok(other) => {
                anyhow::bail!("expected apply ack from agent, got {other:?}")
            }
            Err(error) => {
                // The action may have terminated the app; `next_event`
                // then reports the exit as the final state.
                if self.connection.exit_status()?.is_none() {
                    return Err(error);
                }
            }
        }
        self.last_action = Some(action);
        Ok(())
    }

    fn extract_snapshots(
        &mut self,
        state: Arc<SwiftUIState>,
        _last_action: Option<&SwiftUIAction>,
    ) -> Result<Vec<Snapshot>> {
        self.extractor.run_extractors(state)
    }

    fn state_timestamp(state: &SwiftUIState) -> SystemTime {
        state.timestamp
    }
}
