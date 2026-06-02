use std::collections::VecDeque;
use std::time::SystemTime;

use anyhow::{Result, anyhow, bail};
use bombadil::render::format_timestamp;
use bombadil::runner::{ControlFlow, PropertiesState, RunStrategy};
use bombadil::specification::convert::ToSchema;
use bombadil::specification::domain::Snapshot;
use bombadil::styled;
use bombadil::tree::Tree;

use crate::driver::{TerminalAction, TerminalDriver};
use crate::state::TerminalState;
use crate::trace::TraceWriter;

pub mod driver;
pub mod extractors;
pub mod pty;
pub mod render;
pub mod state;
pub mod trace;

pub enum TerminalTestMode {
    RandomWalk,
    Reproduce(VecDeque<TerminalAction>),
}

pub struct TerminalStrategy {
    pub mode: TerminalTestMode,
    pub writer: Option<TraceWriter>,
    pub test_start: Option<bombadil_schema::Time>,
    pub violations_count: u64,
    pub exit_on_violation: bool,
    pub deadline: Option<SystemTime>,
}

impl TerminalStrategy {
    async fn pick_action(
        &mut self,
        tree: Tree<TerminalAction>,
    ) -> Result<TerminalAction> {
        let tree = tree
            .prune()
            .ok_or_else(|| anyhow::anyhow!("no actions available"))?;
        match &mut self.mode {
            TerminalTestMode::RandomWalk => {
                Ok(tree.pick(&mut rand::rng())?.clone())
            }
            TerminalTestMode::Reproduce(actions) => {
                let original = actions.pop_front().ok_or_else(|| {
                    anyhow!("no remaining actions in reproduce queue")
                })?;
                let available = tree.values();
                if available.iter().any(|a| actions_match(a, &original)) {
                    Ok(original)
                } else {
                    bail!(
                        "reproduce: action {:?} not produced by the spec at this state",
                        original
                    );
                }
            }
        }
    }
}

impl RunStrategy<TerminalDriver> for TerminalStrategy {
    type StopValue = ExitReason;

    async fn on_new_state(
        &mut self,
        state: &TerminalState,
        tree: Tree<TerminalAction>,
        last_action: Option<&TerminalAction>,
        snapshots: &[Snapshot],
        properties: PropertiesState<'_>,
    ) -> Result<ControlFlow<Self::StopValue, TerminalAction>> {
        let test_start = *self.test_start.get_or_insert(
            bombadil_schema::Time::from_system_time(state.timestamp),
        );

        println!();
        for row in &state.rows {
            println!("{}", row);
        }
        println!();

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
            writer
                .write(state, last_action, snapshots, properties.violations)
                .await?;
        }

        if self.violations_count > 0 && self.exit_on_violation {
            return Ok(ControlFlow::Stop(ExitReason::ExitOnViolation));
        }

        if let TerminalTestMode::Reproduce(remaining) = &self.mode
            && remaining.is_empty()
        {
            log::info!("reproduction complete, stopping");
            return Ok(ControlFlow::Stop(ExitReason::Reproduced));
        }

        if state.terminated {
            log::info!("process terminated, stopping");
            return Ok(ControlFlow::Stop(ExitReason::Terminated));
        }

        if let Some(deadline) = self.deadline
            && state.timestamp >= deadline
        {
            log::info!("time limit reached, stopping");
            return Ok(ControlFlow::Stop(ExitReason::TimeLimit));
        }

        let action = self.pick_action(tree).await?;
        println!(
            "{} {}",
            format_timestamp(state.timestamp, test_start),
            render::format_action(&action),
        );

        Ok(ControlFlow::Continue(action))
    }

    async fn on_interrupted(&mut self) -> Result<Self::StopValue> {
        Ok(ExitReason::Interrupted)
    }
}

fn actions_match(a: &TerminalAction, b: &TerminalAction) -> bool {
    serde_json::to_value(a).ok() == serde_json::to_value(b).ok()
}

pub enum ExitReason {
    ExitOnViolation,
    TimeLimit,
    Interrupted,
    Terminated,
    Reproduced,
    AllDefinite,
}
