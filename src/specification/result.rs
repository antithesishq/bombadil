use boa_engine::JsError;

#[derive(Debug)]
pub enum SpecificationError {
    #[allow(dead_code)]
    JsError(JsError),
}

impl From<JsError> for SpecificationError {
    fn from(value: JsError) -> Self {
        SpecificationError::JsError(value)
    }
}

pub type Result<T> = std::result::Result<T, SpecificationError>;
