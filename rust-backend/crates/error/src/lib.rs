//! BitChord Error Types
pub use thiserror::Error;

/// Application result type
pub type Result<T> = std::result::Result<T, Error>;

/// Error types for BitChord application
#[derive(Debug, Error)]
pub enum Error {
    #[error("Configuration error: {0}")]
    ConfigError(String),
    #[error("Database error: {0}")]
    DatabaseError(String),
    #[error("IO error: {0}")]
    IoError(String),
}

impl From<anyhow::Error> for Error {
    fn from(e: anyhow::Error) -> Self {
        Error::IoError(e.to_string())
    }
}