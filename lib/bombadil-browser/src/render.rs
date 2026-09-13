use std::fmt::Formatter;

use bombadil::{
    render::{Format, Formatted},
    styled,
};
use bombadil_browser_keys::key_name;

use crate::browser::actions::BrowserAction;

impl<U8: Format, U16: Format, U64: Format, F64: Format, Text: Format> Format
    for BrowserAction<U8, U16, U64, F64, Text>
{
    fn format(&self, f: &mut Formatter) -> Result<(), std::fmt::Error> {
        match self {
            BrowserAction::Back => {
                write!(f, "{}", styled::maybe_bold("Going back".to_string()))
            }
            BrowserAction::Forward => {
                write!(f, "{}", styled::maybe_bold("Going forward".to_string()))
            }
            BrowserAction::Reload => {
                write!(
                    f,
                    "{}",
                    styled::maybe_bold("Reloading page".to_string())
                )
            }
            BrowserAction::Wait => {
                write!(f, "{}", styled::maybe_bold("Waiting".to_string()))
            }
            BrowserAction::Click { fingerprint, point } => {
                let content_str = fingerprint
                    .text_content
                    .as_ref()
                    .map(|c| {
                        format!(
                            ", content: {}",
                            styled::maybe_blue(format!("{:?}", c))
                        )
                    })
                    .unwrap_or_default();
                write!(
                    f,
                    "{} <{}> (x: {}, y: {}{})",
                    styled::maybe_bold("Clicking".to_string()),
                    fingerprint.tag,
                    styled::maybe_blue(format!("{}", Formatted(&point.x))),
                    styled::maybe_blue(format!("{}", Formatted(&point.y))),
                    content_str
                )
            }
            BrowserAction::DoubleClick { fingerprint, point } => {
                let content_str = fingerprint
                    .text_content
                    .as_ref()
                    .map(|c| {
                        format!(
                            ", content: {}",
                            styled::maybe_blue(format!("{:?}", c))
                        )
                    })
                    .unwrap_or_default();
                write!(
                    f,
                    "{} <{}> (x: {}, y: {}{})",
                    styled::maybe_bold("Double-clicking".to_string()),
                    fingerprint.tag,
                    styled::maybe_blue(format!("{}", Formatted(&point.x))),
                    styled::maybe_blue(format!("{}", Formatted(&point.y))),
                    content_str
                )
            }
            BrowserAction::TypeText { text, delay_millis } => {
                write!(
                    f,
                    "{} {} (delay: {})",
                    styled::maybe_bold("Typing".to_string()),
                    styled::maybe_blue(format!("{}", Formatted(text))),
                    styled::maybe_blue(format!(
                        "{}ms",
                        Formatted(delay_millis)
                    ))
                )
            }
            BrowserAction::PressKey { code } => {
                let key = key_name(*code).unwrap_or("Unknown");
                write!(
                    f,
                    "{} {} (code: {})",
                    styled::maybe_bold("Pressing".to_string()),
                    key,
                    styled::maybe_blue(format!("{code}"))
                )
            }
            BrowserAction::ScrollUp { origin, distance } => {
                write!(
                    f,
                    "{} (x: {}, y: {}, distance: {})",
                    styled::maybe_bold("Scrolling up".to_string()),
                    styled::maybe_blue(format!("{}", Formatted(&origin.x))),
                    styled::maybe_blue(format!("{}", Formatted(&origin.y))),
                    styled::maybe_blue(format!("{}px", Formatted(distance)))
                )
            }
            BrowserAction::ScrollDown { origin, distance } => {
                write!(
                    f,
                    "{} (x: {}, y: {}, distance: {})",
                    styled::maybe_bold("Scrolling down".to_string()),
                    styled::maybe_blue(format!("{}", Formatted(&origin.x))),
                    styled::maybe_blue(format!("{}", Formatted(&origin.y))),
                    styled::maybe_blue(format!("{}px", Formatted(distance)))
                )
            }
            BrowserAction::SetFileInputFiles { selector, files } => {
                write!(
                    f,
                    "{} {} with {} file(s)",
                    styled::maybe_bold("Setting file input".to_string()),
                    styled::maybe_blue(format!("{:?}", selector)),
                    styled::maybe_blue(format!("{}", files.len()))
                )
            }
            BrowserAction::MouseDrag {
                from,
                to,
                steps,
                delay_millis,
            } => {
                write!(
                    f,
                    "{} from (x: {}, y: {}) to (x: {}, y: {}) ({} steps, delay: {})",
                    styled::maybe_bold("Dragging".to_string()),
                    styled::maybe_blue(format!("{}", Formatted(&from.x))),
                    styled::maybe_blue(format!("{}", Formatted(&from.y))),
                    styled::maybe_blue(format!("{}", Formatted(&to.x))),
                    styled::maybe_blue(format!("{}", Formatted(&to.y))),
                    styled::maybe_blue(format!("{}", Formatted(steps))),
                    styled::maybe_blue(format!(
                        "{}ms",
                        Formatted(delay_millis)
                    ))
                )
            }
            BrowserAction::SetViewport { width, height } => {
                write!(
                    f,
                    "{} to {}x{}",
                    styled::maybe_bold("Setting viewport".to_string()),
                    styled::maybe_blue(format!("{}", Formatted(width))),
                    styled::maybe_blue(format!("{}", Formatted(height)))
                )
            }
            BrowserAction::Custom { name, arguments } => {
                write!(
                    f,
                    "{}({})",
                    styled::maybe_bold(name.clone()),
                    arguments
                        .iter()
                        .map(|argument| format!("{argument}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
        }
    }
}
