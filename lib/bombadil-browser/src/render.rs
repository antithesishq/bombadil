use std::{
    fmt::{Display, Formatter},
    ops::RangeInclusive,
};

use bombadil::{
    specification::generators::{CharSetEntry, Regexp, StringGenerator},
    styled,
};
use bombadil_browser_keys::key_name;

use crate::browser::actions::BrowserAction;

pub trait Format {
    fn format(&self, f: &mut Formatter) -> Result<(), std::fmt::Error>;
}

impl Format for u8 {
    fn format(&self, f: &mut Formatter) -> Result<(), std::fmt::Error> {
        write!(f, "{}", self)
    }
}

impl Format for u16 {
    fn format(&self, f: &mut Formatter) -> Result<(), std::fmt::Error> {
        write!(f, "{}", self)
    }
}

impl Format for u64 {
    fn format(&self, f: &mut Formatter) -> Result<(), std::fmt::Error> {
        write!(f, "{}", self)
    }
}

impl Format for f64 {
    fn format(&self, f: &mut Formatter) -> Result<(), std::fmt::Error> {
        write!(f, "{:.01}", self)
    }
}

impl Format for String {
    fn format(&self, f: &mut Formatter) -> Result<(), std::fmt::Error> {
        write!(f, "{}", self)
    }
}

impl<T: Format> Format for RangeInclusive<T> {
    fn format(&self, f: &mut Formatter) -> Result<(), std::fmt::Error> {
        self.start().format(f)?;
        write!(f, "..=")?;
        self.end().format(f)
    }
}

impl Format for StringGenerator {
    fn format(&self, f: &mut Formatter) -> Result<(), std::fmt::Error> {
        match self {
            StringGenerator::Text { length } => {
                write!(f, "<text {}>", Formatted(length))
            }
            StringGenerator::Email => {
                write!(f, "<email>")
            }
            StringGenerator::Regexp {
                regexp: Regexp(regexp),
            } => {
                write!(f, "<regexp {}>", Formatted(regexp))
            }
            StringGenerator::CharSet { entries } => {
                write!(f, "<charset ")?;
                for entry in entries {
                    match entry {
                        CharSetEntry::Range(range) => {
                            write!(f, "\\u{{{}}}", range.start())?;
                            write!(f, "..=")?;
                            write!(f, "\\u{{{}}}", range.end())?;
                        }
                        CharSetEntry::Literal(_) => todo!(),
                    }
                }
                write!(f, ">")
            }
        }
    }
}

struct Formatted<'a, T: Format>(&'a T);

impl<'a, T: Format> Display for Formatted<'a, T> {
    fn fmt(&self, f: &mut Formatter) -> Result<(), std::fmt::Error> {
        self.0.format(f)
    }
}

pub fn format_action<
    U8: Format,
    U16: Format,
    U64: Format,
    F64: Format,
    Text: Format,
>(
    action: &BrowserAction<U8, U16, U64, F64, Text>,
) -> String {
    match action {
        BrowserAction::Back => styled::maybe_bold("Going back".to_string()),
        BrowserAction::Forward => {
            styled::maybe_bold("Going forward".to_string())
        }
        BrowserAction::Reload => {
            styled::maybe_bold("Reloading page".to_string())
        }
        BrowserAction::Wait => styled::maybe_bold("Waiting".to_string()),
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
            format!(
                "{} <{}> (x: {}, y: {}{})",
                styled::maybe_bold("Clicking".to_string()),
                fingerprint.tag,
                styled::maybe_blue(format!("{}", Formatted(&point.x))),
                styled::maybe_blue(format!("{}", Formatted(&point.y))),
                content_str
            )
        }
        BrowserAction::DoubleClick {
            fingerprint,
            point,
            delay_millis,
        } => {
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
            format!(
                "{} <{}> (x: {}, y: {}, delay: {}{})",
                styled::maybe_bold("Double-clicking".to_string()),
                fingerprint.tag,
                styled::maybe_blue(format!("{}", Formatted(&point.x))),
                styled::maybe_blue(format!("{}", Formatted(&point.y))),
                styled::maybe_blue(format!("{}ms", Formatted(delay_millis))),
                content_str
            )
        }
        BrowserAction::TypeText { text, delay_millis } => {
            format!(
                "{} {} (delay: {})",
                styled::maybe_bold("Typing".to_string()),
                styled::maybe_blue(format!("{}", Formatted(text))),
                styled::maybe_blue(format!("{}ms", Formatted(delay_millis)))
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
                styled::maybe_blue(format!("{}", Formatted(&origin.x))),
                styled::maybe_blue(format!("{}", Formatted(&origin.y))),
                styled::maybe_blue(format!("{}px", Formatted(distance)))
            )
        }
        BrowserAction::ScrollDown { origin, distance } => {
            format!(
                "{} (x: {}, y: {}, distance: {})",
                styled::maybe_bold("Scrolling down".to_string()),
                styled::maybe_blue(format!("{}", Formatted(&origin.x))),
                styled::maybe_blue(format!("{}", Formatted(&origin.y))),
                styled::maybe_blue(format!("{}px", Formatted(distance)))
            )
        }
        BrowserAction::SetFileInputFiles { selector, files } => {
            format!(
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
            format!(
                "{} from (x: {}, y: {}) to (x: {}, y: {}) ({} steps, delay: {})",
                styled::maybe_bold("Dragging".to_string()),
                styled::maybe_blue(format!("{}", Formatted(&from.x))),
                styled::maybe_blue(format!("{}", Formatted(&from.y))),
                styled::maybe_blue(format!("{}", Formatted(&to.x))),
                styled::maybe_blue(format!("{}", Formatted(&to.y))),
                styled::maybe_blue(format!("{}", Formatted(steps))),
                styled::maybe_blue(format!("{}ms", Formatted(delay_millis)))
            )
        }
        BrowserAction::SetViewport { width, height } => {
            format!(
                "{} to {}x{}",
                styled::maybe_bold("Setting viewport".to_string()),
                styled::maybe_blue(format!("{}", Formatted(width))),
                styled::maybe_blue(format!("{}", Formatted(height)))
            )
        }
    }
}
