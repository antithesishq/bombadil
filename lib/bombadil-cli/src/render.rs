use bombadil_browser_keys::key_name;

use bombadil::{browser::actions::BrowserAction, styled};

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

pub fn format_action(action: &BrowserAction) -> String {
    match action {
        BrowserAction::Back => styled::maybe_bold("Going back".to_string()),
        BrowserAction::Forward => {
            styled::maybe_bold("Going forward".to_string())
        }
        BrowserAction::Reload => {
            styled::maybe_bold("Reloading page".to_string())
        }
        BrowserAction::Wait => styled::maybe_bold("Waiting".to_string()),
        BrowserAction::Click {
            name,
            content,
            point,
        } => {
            let content_str = content
                .as_ref()
                .map(|c| {
                    format!(
                        ", content: {}",
                        styled::maybe_blue(format!("{:?}", c))
                    )
                })
                .unwrap_or_default();
            format!(
                "{} <{name}> (x: {}, y: {}{})",
                styled::maybe_bold("Clicking".to_string()),
                styled::maybe_blue(format!("{:.1}", point.x)),
                styled::maybe_blue(format!("{:.1}", point.y)),
                content_str
            )
        }
        BrowserAction::DoubleClick {
            name,
            content,
            point,
            delay_millis,
        } => {
            let content_str = content
                .as_ref()
                .map(|c| {
                    format!(
                        ", content: {}",
                        styled::maybe_blue(format!("{:?}", c))
                    )
                })
                .unwrap_or_default();
            format!(
                "{} <{name}> (x: {}, y: {}, delay: {}{})",
                styled::maybe_bold("Double-clicking".to_string()),
                styled::maybe_blue(format!("{:.1}", point.x)),
                styled::maybe_blue(format!("{:.1}", point.y)),
                styled::maybe_blue(format!("{delay_millis}ms")),
                content_str
            )
        }
        BrowserAction::TypeText { text, delay_millis } => {
            format!(
                "{} {} (delay: {})",
                styled::maybe_bold("Typing".to_string()),
                styled::maybe_blue(format!("{:?}", text)),
                styled::maybe_blue(format!("{delay_millis}ms"))
            )
        }
        BrowserAction::PressKey { code } => {
            let key = key_name(*code).unwrap_or("Unknown");
            format!(
                "{} {} (code: {})",
                styled::maybe_bold("Pressing".to_string()),
                key,
                styled::maybe_blue(format!("{code}"))
            )
        }
        BrowserAction::ScrollUp { origin, distance } => {
            format!(
                "{} (x: {}, y: {}, distance: {})",
                styled::maybe_bold("Scrolling up".to_string()),
                styled::maybe_blue(format!("{:.1}", origin.x)),
                styled::maybe_blue(format!("{:.1}", origin.y)),
                styled::maybe_blue(format!("{:.0}px", distance))
            )
        }
        BrowserAction::ScrollDown { origin, distance } => {
            format!(
                "{} (x: {}, y: {}, distance: {})",
                styled::maybe_bold("Scrolling down".to_string()),
                styled::maybe_blue(format!("{:.1}", origin.x)),
                styled::maybe_blue(format!("{:.1}", origin.y)),
                styled::maybe_blue(format!("{:.0}px", distance))
            )
        }
    }
}
