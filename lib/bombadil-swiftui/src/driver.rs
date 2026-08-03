use std::ops::RangeInclusive;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Result, anyhow, bail};
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

const EXIT_STATUS_GRACE: Duration = Duration::from_millis(500);

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
}

impl SwiftUIDriver {
    pub fn launch(
        specification: Specification,
        target: SwiftUITarget,
        connect_timeout: Duration,
        quiescence_timeout: Duration,
        state_timeout: Duration,
    ) -> Result<(Self, Verifier)> {
        if connect_timeout.is_zero() {
            bail!("connect timeout must be greater than zero");
        }
        if state_timeout.is_zero() {
            bail!("state timeout must be greater than zero");
        }

        let bundle_code = bundle(".", &specification.module_specifier)
            .map_err(|e| anyhow!("bundle failed: {e}"))?;

        let extractor = Extractors::initialize(&bundle_code)?;
        let verifier = Verifier::new(&bundle_code)?;

        let connection = AgentConnection::establish(&target, connect_timeout)?;

        let mut driver = Self {
            extractor,
            connection,
            quiescence_timeout,
            state_timeout,
        };

        // The agent connects as soon as the app starts, usually
        // before the first window is on screen, and an empty tree
        // gives the specification nothing to act on.
        driver.await_first_window(connect_timeout)?;

        Ok((driver, verifier))
    }

    /// Poll the agent until the accessibility tree contains at least
    /// one window (the root's children are windows).
    fn await_first_window(&mut self, timeout: Duration) -> Result<()> {
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or_else(|| anyhow!("connect timeout is too large"))?;
        loop {
            let state = self.request_state()?;
            if let Some(status) = &state.exit_status {
                bail!(
                    "app exited with code {} before opening a window",
                    status.code
                );
            }
            if state
                .root
                .as_ref()
                .is_some_and(|root| !root.children.is_empty())
            {
                return Ok(());
            }
            if Instant::now() >= deadline {
                bail!(
                    "app did not open a window within {}s",
                    timeout.as_secs()
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn exited_state(exit_status: swiftui::ProcessExitStatus) -> SwiftUIState {
        SwiftUIState {
            timestamp: SystemTime::now(),
            root: None,
            exit_status: Some(exit_status),
        }
    }

    fn reconcile_transport_error(
        &mut self,
        error: anyhow::Error,
    ) -> Result<swiftui::ProcessExitStatus> {
        self.connection
            .exit_status_within(EXIT_STATUS_GRACE)?
            .ok_or(error)
    }

    fn request_state(&mut self) -> Result<SwiftUIState> {
        if let Some(exit_status) = self.connection.exit_status()? {
            return Ok(Self::exited_state(exit_status));
        }

        let quiescence_millis =
            u64::try_from(self.quiescence_timeout.as_millis())
                .unwrap_or(u64::MAX);
        if let Err(error) = self
            .connection
            .send(&DriverMessage::GetState { quiescence_millis })
        {
            let exit_status = self.reconcile_transport_error(error)?;
            return Ok(Self::exited_state(exit_status));
        }

        match self.connection.receive(self.state_timeout) {
            Ok(AgentMessage::State { root }) => Ok(SwiftUIState {
                timestamp: SystemTime::now(),
                root: Some(root),
                exit_status: None,
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
            Err(error) => {
                let exit_status = self.reconcile_transport_error(error)?;
                Ok(Self::exited_state(exit_status))
            }
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
        self.connection.terminate()
    }

    fn next_event(&mut self) -> Option<DriverEvent<SwiftUIState>> {
        match self.request_state() {
            Ok(state) => Some(DriverEvent::StateChanged(state)),
            Err(error) => Some(DriverEvent::Error(Arc::new(error))),
        }
    }

    fn apply(&mut self, action: SwiftUIAction) -> Result<()> {
        if let Err(error) = self.connection.send(&DriverMessage::Apply {
            action: action.to_schema(),
        }) {
            self.reconcile_transport_error(error)?;
            return Ok(());
        }
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
                self.reconcile_transport_error(error)?;
            }
        }
        Ok(())
    }

    fn extract_snapshots(
        &mut self,
        state: Arc<SwiftUIState>,
        last_action: Option<&SwiftUIAction>,
    ) -> Result<Vec<Snapshot>> {
        self.extractor.run_extractors(state, last_action)
    }

    fn state_timestamp(state: &SwiftUIState) -> SystemTime {
        state.timestamp
    }
}
