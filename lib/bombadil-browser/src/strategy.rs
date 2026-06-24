use crate::browser::actions::BrowserActionTemplate;
use crate::render::format_action;
use crate::url::is_within_domain;
use anyhow::{Result, bail};
use bombadil::render::format_timestamp;
use bombadil::runner::PropertiesState;
use bombadil::styled;
use bombadil::{specification::domain::Snapshot, tree::Tree};
use rand::{RngExt, TryRng};
use std::{collections::VecDeque, path::PathBuf, time::SystemTime};
use url::Url;

use crate::{
    browser::{actions::BrowserAction, state::BrowserState},
    convert::ToSchema,
    driver::BrowserDriver,
    runner::{ControlFlow, PropertyViolation, RunStrategy},
};
use bombadil_schema::markup;

pub enum TestMode {
    RandomWalk,
    Reproduce(VecDeque<BrowserAction>),
}

pub trait TraceWriter {
    fn write(
        &mut self,
        state: &BrowserState,
        last_action: Option<&BrowserAction>,
        snapshots: &[Snapshot],
        violations: &[PropertyViolation],
    ) -> Result<()>;
}

pub struct TestStrategy<Writer: TraceWriter, Rng> {
    pub rng: Rng,
    pub mode: TestMode,
    pub writer: Writer,
    pub exit_on_violation: bool,
    pub test_start: Option<bombadil_schema::Time>,
    pub deadline: Option<SystemTime>,
    pub origin: Url,
    pub output_path: PathBuf,
    pub violations_count: u64,
}

#[derive(Clone, Copy, Debug)]
pub enum ExitReason {
    ExitOnViolation,
    TimeLimit,
    Interrupted,
    Reproduced,
    AllDefinite,
}

#[derive(Clone, Copy, Debug)]
pub struct TestResult {
    pub exit_reason: ExitReason,
    pub violations_count: u64,
}

impl<Writer: TraceWriter, Rng: TryRng + RngExt> TestStrategy<Writer, Rng> {
    fn pick_action(
        &mut self,
        state: &BrowserState,
        tree: Tree<BrowserActionTemplate>,
    ) -> Result<BrowserAction> {
        let tree = if is_within_domain(&state.url, &self.origin) {
            tree
        } else {
            tree.filter(&|a| matches!(a, BrowserAction::Back))
        }
        .prune()
        .ok_or_else(|| anyhow::anyhow!("no actions available"))?;

        match &mut self.mode {
            TestMode::RandomWalk => {
                let template = tree.pick(&mut self.rng)?.clone();
                let action = template.generate(&mut self.rng);
                Ok(action)
            }
            TestMode::Reproduce(actions_original) => {
                if let Some(action_original) = actions_original.pop_front() {
                    let available_actions = tree.values();
                    let action_reconciled =
                        available_actions.iter().any(|action_template| {
                            action_template.accepts(&action_original)
                        });

                    if action_reconciled {
                        Ok(action_original)
                    } else {
                        println!(
                            "\n{}\n\n{}\n\n{}\n\n{}\n",
                            styled::maybe_red(styled::maybe_bold(
                                "no match for original:".into()
                            )),
                            format_action(&action_original),
                            styled::maybe_red(styled::maybe_bold(
                                "in the set of available actions:".into()
                            )),
                            tree.values()
                                .iter()
                                .map(format_action)
                                .collect::<Vec<String>>()
                                .join("\n")
                        );
                        bail!("reproduction and original test diverged!");
                    }
                } else {
                    bail!(
                        "no remaining actions in prefix to apply (this is a bug)"
                    )
                }
            }
        }
    }
}

impl<Writer: TraceWriter, Rng: TryRng + RngExt> RunStrategy<BrowserDriver>
    for TestStrategy<Writer, Rng>
{
    type StopValue = TestResult;

    fn on_new_state(
        &mut self,
        state: &BrowserState,
        tree: Tree<BrowserActionTemplate>,
        last_action: Option<&BrowserAction>,
        snapshots: &[Snapshot],
        properties: PropertiesState<'_>,
    ) -> anyhow::Result<ControlFlow<Self::StopValue, BrowserAction>> {
        let test_start = *self.test_start.get_or_insert(
            bombadil_schema::Time::from_system_time(state.timestamp),
        );

        self.violations_count += properties.violations.len() as u64;
        for violation in properties.violations {
            log::info!("violation of property `{}`", violation.name);
            let api_violation = violation.to_schema();
            let markup = markup::render_violation(&api_violation);
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

        self.writer.write(
            state,
            last_action,
            snapshots,
            properties.violations,
        )?;

        if self.violations_count > 0 && self.exit_on_violation {
            return Ok(ControlFlow::Stop(TestResult {
                exit_reason: ExitReason::ExitOnViolation,
                violations_count: self.violations_count,
            }));
        }

        if properties.all_definite {
            log::info!("all properties are definite, stopping");
            return Ok(ControlFlow::Stop(TestResult {
                exit_reason: ExitReason::AllDefinite,
                violations_count: self.violations_count,
            }));
        }

        if let TestMode::Reproduce(browser_actions) = &self.mode
            && browser_actions.is_empty()
        {
            return Ok(ControlFlow::Stop(TestResult {
                exit_reason: ExitReason::Reproduced,
                violations_count: self.violations_count,
            }));
        }

        if let Some(deadline) = self.deadline
            && state.timestamp >= deadline
        {
            log::info!("time limit reached, stopping");
            return Ok(ControlFlow::Stop(TestResult {
                exit_reason: ExitReason::TimeLimit,
                violations_count: self.violations_count,
            }));
        }

        let action = self.pick_action(state, tree)?;
        println!(
            "{} {}",
            format_timestamp(state.timestamp, test_start),
            format_action(&action)
        );

        Ok(ControlFlow::Continue(action))
    }

    fn on_interrupted(&mut self) -> anyhow::Result<Self::StopValue> {
        Ok(TestResult {
            exit_reason: ExitReason::Interrupted,
            violations_count: self.violations_count,
        })
    }
}
