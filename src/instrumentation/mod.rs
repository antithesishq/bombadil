pub mod html;
pub mod js;
pub mod source_id;

/// Configuration for which types of JavaScript to instrument
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstrumentationConfig {
    pub instrument_files: bool,
    pub instrument_inline: bool,
}

impl InstrumentationConfig {
    pub fn all() -> Self {
        Self {
            instrument_files: true,
            instrument_inline: true,
        }
    }

    pub fn none() -> Self {
        Self {
            instrument_files: false,
            instrument_inline: false,
        }
    }
}

impl Default for InstrumentationConfig {
    fn default() -> Self {
        Self::all()
    }
}
