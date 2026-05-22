use crate::render::format_action;
use crate::url::is_within_domain;
use anyhow::{Result, bail};
use bombadil::render::format_timestamp;
use bombadil::runner::PropertiesState;
use bombadil::styled;
use bombadil::{specification::domain::Snapshot, tree::Tree};
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
    ) -> impl std::future::Future<Output = Result<()>> + Send;
}

pub struct TestStrategy<Writer: TraceWriter> {
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

impl<Writer: TraceWriter> TestStrategy<Writer> {
    async fn pick_action(
        &mut self,
        state: &BrowserState,
        tree: Tree<BrowserAction>,
    ) -> Result<BrowserAction> {
        let tree = if is_within_domain(&state.url, &self.origin) {
            tree
        } else {
            tree.filter(&|a| matches!(a, BrowserAction::Back))
        }
        .prune()
        .ok_or_else(|| anyhow::anyhow!("no actions available"))?;

        match &mut self.mode {
            TestMode::RandomWalk => Ok(tree.pick(&mut rand::rng())?.clone()),
            TestMode::Reproduce(actions_original) => {
                if let Some(action_original) = actions_original.pop_front() {
                    let available_actions = tree.values();
                    let action_reconciled = available_actions
                        .iter()
                        .filter_map(|action| {
                            reconcile_reproducible_action(
                                action,
                                &action_original,
                            )
                        })
                        .min_by(|(_, a), (_, b)| a.total_cmp(b))
                        .map(|(action, _)| action);

                    if let Some(action) = action_reconciled {
                        Ok(action)
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

impl<Writer: TraceWriter> RunStrategy<BrowserDriver> for TestStrategy<Writer> {
    type StopValue = TestResult;

    async fn on_new_state(
        &mut self,
        state: &BrowserState,
        tree: Tree<BrowserAction>,
        last_action: Option<&BrowserAction>,
        snapshots: &[Snapshot],
        properties: PropertiesState<'_>,
    ) -> anyhow::Result<ControlFlow<Self::StopValue, BrowserAction>> {
        let test_start = *self.test_start.get_or_insert(
            bombadil_schema::Time::from_system_time(state.timestamp),
        );

        if let Some(action) = last_action {
            println!(
                "{} {}",
                format_timestamp(state.timestamp, test_start),
                format_action(action)
            );
        }

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

        self.writer
            .write(state, last_action, snapshots, properties.violations)
            .await?;

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

        Ok(ControlFlow::Continue(self.pick_action(state, tree).await?))
    }

    async fn on_interrupted(&mut self) -> anyhow::Result<Self::StopValue> {
        Ok(TestResult {
            exit_reason: ExitReason::Interrupted,
            violations_count: self.violations_count,
        })
    }
}

const RECONCILE_DISTANCE_MAX: f64 = 500.0;

fn reconcile_reproducible_action(
    candidate: &BrowserAction,
    original: &BrowserAction,
) -> Option<(BrowserAction, f64)> {
    match (candidate, original) {
        (BrowserAction::Back, BrowserAction::Back) => {
            Some((BrowserAction::Back, 0.0))
        }
        (BrowserAction::Forward, BrowserAction::Forward) => {
            Some((BrowserAction::Forward, 0.0))
        }
        (
            BrowserAction::Click {
                name: candidate_name,
                content: candidate_content,
                point: candidate_point,
            },
            BrowserAction::Click {
                name: original_name,
                content: original_content,
                point: original_point,
            },
        ) if candidate_name == original_name
            && candidate_content == original_content =>
        {
            let distance = (candidate_point.x - original_point.x)
                .hypot(candidate_point.y - original_point.y);
            (distance < RECONCILE_DISTANCE_MAX)
                .then(|| (candidate.clone(), distance))
        }
        (
            BrowserAction::DoubleClick {
                name: candidate_name,
                content: candidate_content,
                point: candidate_point,
                ..
            },
            BrowserAction::DoubleClick {
                name: original_name,
                content: original_content,
                point: original_point,
                ..
            },
        ) if candidate_name == original_name
            && candidate_content == original_content =>
        {
            let distance = (candidate_point.x - original_point.x)
                .hypot(candidate_point.y - original_point.y);
            (distance < RECONCILE_DISTANCE_MAX)
                .then(|| (candidate.clone(), distance))
        }
        (BrowserAction::TypeText { .. }, BrowserAction::TypeText { .. }) => {
            Some((original.clone(), 0.0))
        }
        (BrowserAction::PressKey { .. }, BrowserAction::PressKey { .. }) => {
            Some((original.clone(), 0.0))
        }
        (BrowserAction::ScrollUp { .. }, BrowserAction::ScrollUp { .. }) => {
            Some((candidate.clone(), 0.0))
        }
        (
            BrowserAction::ScrollDown { .. },
            BrowserAction::ScrollDown { .. },
        ) => Some((candidate.clone(), 0.0)),
        (BrowserAction::Reload, BrowserAction::Reload) => {
            Some((BrowserAction::Reload, 0.0))
        }
        (BrowserAction::Wait, BrowserAction::Wait) => {
            Some((BrowserAction::Wait, 0.0))
        }
        (
            BrowserAction::SetFileInputFiles {
                selector: candidate_selector,
                ..
            },
            BrowserAction::SetFileInputFiles {
                selector: original_selector,
                ..
            },
        ) if candidate_selector == original_selector => {
            Some((original.clone(), 0.0))
        }

        _ => None,
    }
}
