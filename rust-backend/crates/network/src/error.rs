//! Network-specific error types.

use thiserror::Error;

/// Errors that can occur during network operations.
#[derive(Debug, Error)]
pub enum NetworkError {
    /// An underlying transport error (connection reset, DNS failure, etc.).
    #[error("transport error: {0}")]
    Transport(#[from] reqwest::Error),

    /// The remote server returned an unexpected HTTP status.
    #[error("unexpected status {status} for {url}")]
    HttpStatus {
        status: u16,
        url: String,
    },

    /// The response body could not be parsed as the expected type.
    #[error("failed to parse response: {0}")]
    Parse(#[from] serde_json::Error),

    /// The session is not authenticated (no cookie / no sign-in).
    #[error("not signed in: {0}")]
    NotSignedIn(String),

    /// PoToken generation is unavailable or failed.
    #[error("PoToken error: {0}")]
    PoToken(String),

    /// A timeout was reached while waiting for a response.
    #[error("request timed out: {0}")]
    Timeout(String),
}

/// Convenience Result alias used throughout this crate.
pub type Result<T> = std::result::Result<T, NetworkError>;
