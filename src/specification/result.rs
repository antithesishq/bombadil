use std::io;

use boa_engine::JsError;

#[allow(dead_code)] // because fields are only used in Debug
#[derive(Debug)]
pub enum SpecificationError {
    JS(JsError),
    IO(io::Error),
    ModuleError(String),
}

impl From<JsError> for SpecificationError {
    fn from(value: JsError) -> Self {
        SpecificationError::JS(value)
    }
}

impl From<io::Error> for SpecificationError {
    fn from(value: io::Error) -> Self {
        SpecificationError::IO(value)
    }
}

pub type Result<T> = std::result::Result<T, SpecificationError>;
