use std::collections::VecDeque;
use std::time::SystemTime;

use anyhow::{Result, anyhow, bail};
use bombadil::render::format_timestamp;
use bombadil::runner::{ControlFlow, PropertiesState, RunStrategy};
use bombadil::specification::convert::ToSchema;
use bombadil::specification::domain::Snapshot;
use bombadil::styled;
use bombadil::tree::Tree;
use bombadil_schema::swiftui::ProcessExitStatus;
use rand::{RngExt, TryRng};

use crate::driver::{SwiftUIAction, SwiftUIActionTemplate, SwiftUIDriver};
use crate::state::SwiftUIState;
use crate::trace::TraceWriter;

pub mod agent;
pub mod driver;
pub mod extractors;
pub mod js;
pub mod state;
pub mod trace;

pub enum SwiftUITestMode {
    RandomWalk,
    Reproduce(VecDeque<SwiftUIAction>),
}

pub struct SwiftUIStrategy<Rng: TryRng> {
    pub rng: Rng,
    pub mode: SwiftUITestMode,
    pub writer: Option<TraceWriter>,
    pub test_start: Option<bombadil_schema::Time>,
    pub violations_count: u64,
    pub exit_on_violation: bool,
    pub deadline: Option<SystemTime>,
    pub states_seen: usize,
}

impl<Rng: TryRng + RngExt> SwiftUIStrategy<Rng> {
    fn pick_action(
        &mut self,
        tree: Tree<SwiftUIActionTemplate>,
    ) -> Result<SwiftUIAction> {
        let tree = tree
            .prune()
            .ok_or_else(|| anyhow!("no actions available"))?;
        match &mut self.mode {
            SwiftUITestMode::RandomWalk => {
                Ok(tree.pick(&mut self.rng)?.generate(&mut self.rng))
            }
            SwiftUITestMode::Reproduce(actions) => {
                let original = actions.pop_front().ok_or_else(|| {
                    anyhow!("no remaining actions in reproduce queue")
                })?;
                let available = tree.values();
                if available.iter().any(|template| template.accepts(&original))
                {
                    Ok(original)
                } else {
                    bail!(
                        "reproduce: action {:?} not produced by the spec at this state:\n\n{:?}",
                        original,
                        available
                    );
                }
            }
        }
    }

    fn stop(
        &mut self,
        reason: ExitReason,
    ) -> Result<ControlFlow<ExitReason, SwiftUIAction>> {
        if let Some(writer) = self.writer.as_mut() {
            writer.flush()?;
        }
        Ok(ControlFlow::Stop(reason))
    }
}

impl<Rng: TryRng + RngExt> RunStrategy<SwiftUIDriver> for SwiftUIStrategy<Rng> {
    type StopValue = ExitReason;

    fn on_new_state(
        &mut self,
        state: &SwiftUIState,
        tree: Tree<SwiftUIActionTemplate>,
        last_action: Option<&SwiftUIAction>,
        snapshots: &[Snapshot],
        properties: PropertiesState<'_>,
    ) -> Result<ControlFlow<Self::StopValue, SwiftUIAction>> {
        self.states_seen += 1;

        let test_start = *self.test_start.get_or_insert(
            bombadil_schema::Time::from_system_time(state.timestamp),
        );

        self.violations_count += properties.violations.len() as u64;
        for violation in properties.violations {
            log::info!("violation of property `{}`", violation.name);
            let schema_violation = violation.to_schema();
            let markup =
                bombadil_schema::markup::render_violation(&schema_violation);
            let text = styled::markup_to_styled(&markup, test_start);
            println!(
                "\n{}\n\n{}\n",
                styled::maybe_red(styled::maybe_bold(format!(
                    "{} was violated:",
                    violation.name
                ))),
                text
            );
        }

        if let Some(writer) = self.writer.as_mut() {
            writer.write(
                state,
                last_action,
                snapshots,
                properties.violations,
            )?;
        }

        if self.violations_count > 0 && self.exit_on_violation {
            return self.stop(ExitReason::ExitOnViolation);
        }

        if let SwiftUITestMode::Reproduce(remaining) = &self.mode
            && remaining.is_empty()
        {
            log::info!("reproduction complete, stopping");
            return self.stop(ExitReason::Reproduced);
        }

        if let Some(status) = &state.exit_status {
            log::info!("app terminated, stopping");
            return self.stop(ExitReason::Terminated(status.clone()));
        }

        if let Some(deadline) = self.deadline
            && state.timestamp >= deadline
        {
            log::info!("time limit reached, stopping");
            return self.stop(ExitReason::TimeLimit);
        }

        let action = self.pick_action(tree)?;
        println!(
            "{} [{} nodes] {}",
            format_timestamp(state.timestamp, test_start),
            state.node_count(),
            format_action(&action),
        );

        Ok(ControlFlow::Continue(action))
    }

    fn on_interrupted(&mut self) -> Result<Self::StopValue> {
        if let Some(writer) = self.writer.as_mut() {
            writer.flush()?;
        }
        Ok(ExitReason::Interrupted)
    }
}

pub enum ExitReason {
    ExitOnViolation,
    TimeLimit,
    Interrupted,
    Terminated(ProcessExitStatus),
    Reproduced,
}

pub fn format_action(action: &SwiftUIAction) -> String {
    match action {
        SwiftUIAction::Tap { x, y } => format!("tap ({x:.0}, {y:.0})"),
        SwiftUIAction::TypeText { text } => format!("type {text:?}"),
        SwiftUIAction::PressKey { key } => format!("press {key}"),
        SwiftUIAction::ScrollUp { x, y, distance } => {
            format!("scroll up {distance:.0} at ({x:.0}, {y:.0})")
        }
        SwiftUIAction::ScrollDown { x, y, distance } => {
            format!("scroll down {distance:.0} at ({x:.0}, {y:.0})")
        }
    }
}
