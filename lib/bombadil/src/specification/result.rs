use std::{fmt::Display, io, time::SystemTimeError};

use oxc::diagnostics::OxcDiagnostic;
use rquickjs::{CatchResultExt, Ctx};

use crate::specification::engine::caught_to_string;

#[derive(Debug)]
pub enum SpecificationError {
    JS(String),
    IO(io::Error),
    TranspilationError(Vec<OxcDiagnostic>),
    SystemTimeError(SystemTimeError),
    OtherError(String),
}

impl Display for SpecificationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpecificationError::JS(js_error) => js_error.fmt(f),
            SpecificationError::IO(error) => error.fmt(f),
            SpecificationError::SystemTimeError(system_time_error) => {
                system_time_error.fmt(f)
            }
            SpecificationError::OtherError(message) => message.fmt(f),
            SpecificationError::TranspilationError(diagnostics) => {
                for diagnostic in diagnostics {
                    writeln!(f, "{}", diagnostic)?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for SpecificationError {}

impl From<io::Error> for SpecificationError {
    fn from(value: io::Error) -> Self {
        SpecificationError::IO(value)
    }
}

impl From<SystemTimeError> for SpecificationError {
    fn from(value: SystemTimeError) -> Self {
        SpecificationError::SystemTimeError(value)
    }
}

pub trait JsResultExt<'js, T> {
    fn js(self, ctx: &Ctx<'js>) -> Result<T>;
}

impl<'js, T> JsResultExt<'js, T> for rquickjs::Result<T> {
    fn js(self, ctx: &Ctx<'js>) -> Result<T> {
        self.catch(ctx)
            .map_err(|error| SpecificationError::JS(caught_to_string(error)))
    }
}

pub type Result<T> = std::result::Result<T, SpecificationError>;
