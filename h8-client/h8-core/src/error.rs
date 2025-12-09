//! Error types for h8-core.

use thiserror::Error;

/// Result type alias using h8-core's Error type.
pub type Result<T> = std::result::Result<T, Error>;

/// Error types for h8-core operations.
#[derive(Debug, Error)]
pub enum Error {
    /// Configuration error.
    #[error("configuration error: {0}")]
    Config(String),

    /// Path discovery error.
    #[error("path error: {0}")]
    Path(String),

    /// Database error.
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),

    /// HTTP/service error.
    #[error("service error: {0}")]
    Service(String),

    /// IO error.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON serialization error.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// ID pool exhausted.
    #[error("ID pool exhausted - no free IDs available")]
    IdPoolExhausted,

    /// ID not found.
    #[error("ID not found: {0}")]
    IdNotFound(String),
}

impl From<reqwest::Error> for Error {
    fn from(err: reqwest::Error) -> Self {
        Error::Service(err.to_string())
    }
}

impl From<config::ConfigError> for Error {
    fn from(err: config::ConfigError) -> Self {
        Error::Config(err.to_string())
    }
}
