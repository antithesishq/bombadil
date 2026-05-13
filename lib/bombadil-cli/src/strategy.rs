use anyhow::{Result, bail};
use bombadil::specification::domain::Snapshot;
use std::{collections::VecDeque, path::PathBuf, time::SystemTime};

use bombadil::{
    browser::{actions::BrowserAction, state::BrowserState},
    runner::{ControlFlow, RunStrategy},
    specification::convert::ToSchema,
    styled,
    trace::{PropertyViolation, writer::TraceWriter},
};
use bombadil_schema::markup;

use crate::render;

pub enum TestMode {
    RandomWalk,
    Reproduce(VecDeque<BrowserAction>),
}

pub struct TestStrategy {
    pub mode: TestMode,
    pub writer: TraceWriter,
    pub exit_on_violation: bool,
    pub test_start: Option<bombadil_schema::Time>,
    pub deadline: Option<SystemTime>,
    pub output_path: PathBuf,
    pub violations_count: u64,
}

#[derive(Clone, Copy, Debug)]
pub enum ExitReason {
    ExitOnViolation,
    TimeLimit,
    Interrupted,
    Reproduced,
}

#[derive(Clone, Copy, Debug)]
pub struct TestResult {
    pub exit_reason: ExitReason,
    pub violations_count: u64,
}

impl RunStrategy for TestStrategy {
    type StopValue = TestResult;

    async fn on_new_state(
        &mut self,
        state: &BrowserState,
        last_action: Option<&BrowserAction>,
        snapshots: &[Snapshot],
        violations: &[PropertyViolation],
    ) -> anyhow::Result<ControlFlow<Self::StopValue>> {
        let test_start = *self.test_start.get_or_insert(
            bombadil_schema::Time::from_system_time(state.timestamp),
        );

        if let Some(action) = last_action {
            println!(
                "{} {}",
                render::format_timestamp(state.timestamp, test_start),
                render::format_action(action)
            );
        }

        self.violations_count += violations.len() as u64;
        for violation in violations {
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
            .write(state, last_action, snapshots, violations)
            .await?;

        if self.violations_count > 0 && self.exit_on_violation {
            return Ok(ControlFlow::Stop(TestResult {
                exit_reason: ExitReason::ExitOnViolation,
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

        Ok(ControlFlow::Continue)
    }

    async fn on_interrupted(&mut self) -> anyhow::Result<Self::StopValue> {
        Ok(TestResult {
            exit_reason: ExitReason::Interrupted,
            violations_count: self.violations_count,
        })
    }

    async fn pick_action(
        &mut self,
        tree: bombadil::tree::Tree<BrowserAction>,
    ) -> Result<BrowserAction> {
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
                            render::format_action(&action_original),
                            styled::maybe_red(styled::maybe_bold(
                                "in the set of available actions:".into()
                            )),
                            tree.values()
                                .iter()
                                .map(render::format_action)
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
