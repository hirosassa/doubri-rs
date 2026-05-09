use thiserror::Error;

#[derive(Debug, Error)]
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
}
