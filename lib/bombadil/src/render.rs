use std::{
    fmt::{Display, Formatter},
    ops::RangeInclusive,
    time::Duration,
};

use crate::specification::generators::{CharSetEntry, Regexp, StringGenerator};

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

impl Format for Duration {
    fn format(&self, f: &mut Formatter) -> Result<(), std::fmt::Error> {
        write!(
            f,
            "{}",
            bombadil_schema::duration::format_duration(
                *self,
                bombadil_schema::duration::FormatDurationOptions {
                    include_millis: true,
                },
            )
        )
    }
}

pub struct Formatted<'a, T: Format>(pub &'a T);

impl<'a, T: Format> Display for Formatted<'a, T> {
    fn fmt(&self, f: &mut Formatter) -> Result<(), std::fmt::Error> {
        self.0.format(f)
    }
}
