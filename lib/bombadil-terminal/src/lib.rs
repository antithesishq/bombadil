use std::collections::VecDeque;
use std::io::Write;
use std::time::SystemTime;

use anyhow::{Result, anyhow, bail};
use bombadil::render::format_timestamp;
use bombadil::runner::{ControlFlow, PropertiesState, RunStrategy};
use bombadil::specification::convert::ToSchema;
use bombadil::specification::domain::Snapshot;
use bombadil::styled;
use bombadil::tree::Tree;
use bombadil_schema::{
    TerminalAttributes, TerminalCell, TerminalColor, TerminalStyle,
};
use owo_colors::{OwoColorize, XtermColors};

use crate::driver::{TerminalAction, TerminalDriver};
use crate::state::TerminalState;
use crate::trace::TraceWriter;

pub mod driver;
pub mod extractors;
pub mod js;
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
    pub states_seen: usize,
}

impl TerminalStrategy {
    #[hotpath::measure]
    fn pick_action(
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

    fn stop(
        &mut self,
        reason: ExitReason,
    ) -> Result<ControlFlow<ExitReason, TerminalAction>> {
        if let Some(writer) = self.writer.as_mut() {
            writer.flush()?;
        }
        Ok(ControlFlow::Stop(reason))
    }
}

impl RunStrategy<TerminalDriver> for TerminalStrategy {
    type StopValue = ExitReason;

    #[hotpath::measure]
    fn on_new_state(
        &mut self,
        state: &TerminalState,
        tree: Tree<TerminalAction>,
        last_action: Option<&TerminalAction>,
        snapshots: &[Snapshot],
        properties: PropertiesState<'_>,
    ) -> Result<ControlFlow<Self::StopValue, TerminalAction>> {
        use std::fmt::Write;

        self.states_seen += 1;

        let mut buffer =
            String::with_capacity(state.grid.size.cell_count() as usize * 4);
        write!(buffer, "\x1b[2J\x1b[H")?;

        let test_start = *self.test_start.get_or_insert(
            bombadil_schema::Time::from_system_time(state.timestamp),
        );

        // Render currently visible grid
        {
            for row_index in 0..state.grid.size.rows {
                for column_index in 0..state.grid.size.columns {
                    match &state.grid[(row_index, column_index)] {
                        TerminalCell::Occupied {
                            contents,
                            wide: _,
                            style,
                        } => {
                            let style: owo_colors::Style = to_owo_style(style);
                            if contents.is_empty() {
                                write!(buffer, "{}", " ".style(style))?;
                            } else {
                                write!(
                                    buffer,
                                    "{}",
                                    format!("{}", contents).style(style)
                                )?;
                            };
                        }
                        TerminalCell::Continuation { .. } => {}
                        TerminalCell::Empty { style } => {
                            let style: owo_colors::Style = to_owo_style(style);
                            write!(buffer, "{}", " ".style(style))?;
                        }
                    };
                }
                writeln!(buffer)?;
            }
        }

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

        if let TerminalTestMode::Reproduce(remaining) = &self.mode
            && remaining.is_empty()
        {
            log::info!("reproduction complete, stopping");
            return self.stop(ExitReason::Reproduced);
        }

        if state.terminated {
            log::info!("process terminated, stopping");
            return self.stop(ExitReason::Terminated);
        }

        if let Some(deadline) = self.deadline
            && state.timestamp >= deadline
        {
            log::info!("time limit reached, stopping");
            return self.stop(ExitReason::TimeLimit);
        }

        let action = self.pick_action(tree)?;
        writeln!(
            buffer,
            "{} {}",
            format_timestamp(state.timestamp, test_start),
            render::format_action(&action),
        )?;

        print!("{}", buffer);
        std::io::stdout().flush()?;

        Ok(ControlFlow::Continue(action))
    }

    fn on_interrupted(&mut self) -> Result<Self::StopValue> {
        if let Some(writer) = self.writer.as_mut() {
            writer.flush()?;
        }
        Ok(ExitReason::Interrupted)
    }
}

#[hotpath::measure]
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

#[hotpath::measure]
fn to_owo_style(value: &TerminalStyle) -> owo_colors::Style {
    let mut style = owo_colors::Style::new();

    if let Some(color) = to_owo_color(&value.foreground_color) {
        style = style.color(color);
    }
    if let Some(color) = to_owo_color(&value.background_color) {
        style = style.on_color(color);
    }

    if value.attributes.contains(TerminalAttributes::BOLD) {
        style = style.bold();
    }
    if value.attributes.contains(TerminalAttributes::ITALIC) {
        style = style.italic();
    }
    if value.attributes.contains(TerminalAttributes::BLINK) {
        style = style.blink();
    }
    if value.attributes.contains(TerminalAttributes::INVERSE) {
        // unsupported?
    }
    if value.attributes.contains(TerminalAttributes::STRIKETHROUGH) {
        style = style.strikethrough();
    }
    if value.attributes.contains(TerminalAttributes::DIM) {
        style = style.dimmed();
    }

    if !matches!(value.underline, bombadil_schema::TerminalUnderline::None) {
        style = style.underline();
    }

    style
}

fn to_owo_color(value: &TerminalColor) -> Option<owo_colors::DynColors> {
    use owo_colors::DynColors;
    match value {
        TerminalColor::None => None,
        TerminalColor::Palette(index) => {
            Some(DynColors::Xterm(XtermColors::from(*index)))
        }
        TerminalColor::RGB { r, g, b } => Some(DynColors::Rgb(*r, *g, *b)),
    }
}
