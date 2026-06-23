use bombadil::styled;

use crate::driver::TerminalAction;

pub fn format_action(action: &TerminalAction) -> String {
    match action {
        TerminalAction::TypeText { text } => {
            format!(
                "{} {}",
                styled::maybe_bold("Typing".to_string()),
                styled::maybe_blue(format!("{:?}", text)),
            )
        }
        TerminalAction::Resize { size } => {
            format!(
                "{} (columns: {}, rows: {})",
                styled::maybe_bold("Resizing".to_string()),
                styled::maybe_blue(format!("{}", size.columns)),
                styled::maybe_blue(format!("{}", size.rows)),
            )
        }
        TerminalAction::ScrollUp {} => {
            styled::maybe_bold("Scrolling up".to_string())
        }
        TerminalAction::ScrollDown {} => {
            styled::maybe_bold("Scrolling down".to_string())
        }
    }
}
