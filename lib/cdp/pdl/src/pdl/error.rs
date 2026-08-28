use std::fmt;

#[derive(Debug)]
pub struct Error {
    pub message: String,
}
impl Error {
    pub fn new(msg: impl Into<String>) -> Self {
        Self {
            message: msg.into(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for Error {}

macro_rules! format_err {
    ($($tt:tt)*) => {
        $crate::pdl::error::Error {
            message: format!($($tt)*),
        }
    };
}

macro_rules! bail {
    ($($tt:tt)*) => { return Err(format_err!($($tt)*)) };
}

macro_rules! borrowed {
    ($m:expr) => {
        $m.map(|x|x.as_str()).map(std::borrow::Cow::Borrowed)
    };
    ($m:expr, $($tt:tt)*) => {
         borrowed!($m).ok_or_else(||format_err!($($tt)*))
    };
}

pub(crate) use {bail, borrowed, format_err};
