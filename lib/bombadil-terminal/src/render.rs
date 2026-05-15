use bombadil::styled;

use crate::driver::TerminalAction;

pub fn format_timestamp(
    timestamp: std::time::SystemTime,
    test_start: bombadil_schema::Time,
) -> String {
    let time = bombadil_schema::Time::from_system_time(timestamp);
    let elapsed = std::time::Duration::from_micros(
        time.as_micros().saturating_sub(test_start.as_micros()),
    );
    styled::maybe_dimmed(bombadil_schema::duration::format_duration(
        elapsed,
        bombadil_schema::duration::FormatDurationOptions {
            include_millis: true,
        },
    ))
}

pub fn format_action(action: &TerminalAction) -> String {
    match action {
        TerminalAction::TypeText { text } => {
            format!(
                "{} {}",
                styled::maybe_bold("Typing".to_string()),
                styled::maybe_blue(format!("{:?}", text)),
            )
        }
        TerminalAction::PressKey { code } => {
            let rendered = char::from_u32(*code)
                .map(|c| format!("{:?}", c))
                .unwrap_or_else(|| format!("U+{:04X}", code));
            format!(
                "{} {} (code: {})",
                styled::maybe_bold("Pressing".to_string()),
                styled::maybe_blue(rendered),
                styled::maybe_blue(format!("{code}")),
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
