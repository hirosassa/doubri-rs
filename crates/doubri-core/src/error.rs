use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum DoubriError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("invalid file format: {msg}")]
    InvalidFormat { msg: String },

    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("missing field '{field}' in JSON record")]
    MissingField { field: String },

    #[error("configuration error: {msg}")]
    Config { msg: String },

    #[error("arithmetic overflow in {context}")]
    Overflow { context: &'static str },

    #[error("value out of range in {context}")]
    OutOfRange { context: &'static str },
}

/// Converts between integer types, reporting a typed error instead of truncating.
pub(crate) fn conv<T, U>(value: T, context: &'static str) -> Result<U, DoubriError>
where
    U: TryFrom<T>,
{
    U::try_from(value).map_err(|_| DoubriError::OutOfRange { context })
}
