//! External drivers: Bombadil drives an app through a separate driver
//! program (for example a SwiftUI, Jetpack Compose or Flutter test
//! harness) that speaks the line-delimited JSON protocol in
//! [`protocol`] over its stdin and stdout.

use std::collections::VecDeque;
use std::io::Write;
use std::time::{Duration, SystemTime};

use anyhow::{Result, anyhow, bail};
use bombadil::driver::{ActionTemplate, RunState};
use bombadil::render::Formatted;
use bombadil::runner::{ControlFlow, PropertiesState, RunStrategy};
use bombadil::specification::convert::ToSchema;
use bombadil::specification::domain::Snapshot;
use bombadil::styled;
use bombadil::tree::Tree;
use bombadil_schema::Time;
use rand::{RngExt, TryRng};

use crate::action::{ExternalAction, ExternalActionTemplate};
use crate::driver::{ExternalSession, ExternalState};

pub mod action;
pub mod driver;
pub mod protocol;
pub mod trace;

pub enum ExternalTestMode {
    RandomWalk,
    Reproduce(VecDeque<ExternalAction>),
}

pub enum ExitReason {
    ExitOnViolation,
    TimeLimit,
    Stalled,
    Interrupted,
    Reproduced,
}

pub struct ExternalStrategy<Rng: TryRng> {
    pub rng: Rng,
    pub mode: ExternalTestMode,
    pub test_start: Option<Time>,
    pub violations_count: u64,
    pub exit_on_violation: bool,
    pub deadline: Option<SystemTime>,
    pub quiet: bool,
}

impl<Rng: TryRng + RngExt> ExternalStrategy<Rng> {
    fn pick_action(
        &mut self,
        tree: Tree<ExternalActionTemplate>,
    ) -> Result<ExternalAction> {
        let tree = tree
            .prune()
            .ok_or_else(|| anyhow!("no actions available"))?;
        match &mut self.mode {
            ExternalTestMode::RandomWalk => {
                Ok(tree.pick(&mut self.rng)?.generate(&mut self.rng))
            }
            ExternalTestMode::Reproduce(actions) => {
                let original = actions.pop_front().ok_or_else(|| {
                    anyhow!("no remaining actions in reproduce queue")
                })?;
                let available = tree.values();
                if available.iter().any(|template| template.accepts(&original))
                {
                    Ok(original)
                } else {
                    bail!(
                        "reproduce: action {} not produced by the spec at this state:\n\n{:?}",
                        original.0,
                        available
                    );
                }
            }
        }
    }
}

impl<Rng: TryRng + RngExt> RunStrategy<ExternalSession>
    for ExternalStrategy<Rng>
{
    type StopValue = ExitReason;

    fn on_new_state(
        &mut self,
        state: &ExternalState,
        tree: Tree<ExternalActionTemplate>,
        _last_action: Option<&ExternalAction>,
        _snapshots: &[Snapshot],
        properties: PropertiesState<'_>,
    ) -> Result<ControlFlow<ExitReason, ExternalAction>> {
        let test_start = *self.test_start.get_or_insert(state.timestamp());

        self.violations_count += properties.violations.len() as u64;
        if !self.quiet {
            for violation in properties.violations {
                let markup = bombadil_schema::markup::render_violation(
                    &violation.to_schema(),
                );
                println!(
                    "\n{}\n\n{}\n",
                    styled::maybe_red(styled::maybe_bold(format!(
                        "{} was violated:",
                        violation.name
                    ))),
                    styled::markup_to_styled(&markup, test_start)
                );
            }
        }

        if self.violations_count > 0 && self.exit_on_violation {
            return Ok(ControlFlow::Stop(ExitReason::ExitOnViolation));
        }
        if let ExternalTestMode::Reproduce(remaining) = &self.mode
            && remaining.is_empty()
        {
            return Ok(ControlFlow::Stop(ExitReason::Reproduced));
        }
        if self
            .deadline()
            .is_some_and(|deadline| state.timestamp() >= deadline)
        {
            return Ok(ControlFlow::Stop(ExitReason::TimeLimit));
        }

        let action = self.pick_action(tree)?;
        if !self.quiet {
            let elapsed = Duration::from_micros(
                state
                    .timestamp()
                    .as_micros()
                    .saturating_sub(test_start.as_micros()),
            );
            println!("{} {}", Formatted(&elapsed), Formatted(&action));
            std::io::stdout().flush()?;
        }
        Ok(ControlFlow::Continue(action))
    }

    fn deadline(&self) -> Option<Time> {
        self.deadline.map(Time::from_system_time)
    }

    fn on_time_limit(&mut self) -> Result<ExitReason> {
        Ok(ExitReason::TimeLimit)
    }

    fn on_stalled(&mut self) -> Result<ExitReason> {
        Ok(ExitReason::Stalled)
    }

    fn on_interrupted(&mut self) -> Result<ExitReason> {
        Ok(ExitReason::Interrupted)
    }
}
