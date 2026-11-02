//! BitChord Network crate.
//!
//! This crate provides the networking layer for the BitChord Tauri backend.
//! It implements an InnerTube client for YouTube Music API calls and provides
//! PoToken types (no browser automation yet).

pub mod error;
pub mod http;
pub mod innertube;
pub mod models;
pub mod potoken;

// Re-export main types for ergonomic access.
pub use error::{NetworkError, Result};
pub use http::HttpClient;
pub use innertube::{InnerTubeClient, InnerTubeSession, SessionScope};
pub use models::{PlaybackTracking, PlayerFormat, PlayerResponse, PlayabilityStatus, StreamingData};
pub use potoken::{NoOpPoTokenProvider, PoTokenError, PoTokenProvider, PoTokenResult};

/// Create a new InnerTube client with the given cookie (or anonymous).
pub fn new_inner_tube_client(cookie: Option<String>) -> Result<InnerTubeClient> {
    InnerTubeClient::new(cookie)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_potoken_result_creation() {
        let result = PoTokenResult::new("token1".to_string(), "token2".to_string());
        assert_eq!(result.player_request_token, "token1");
        assert_eq!(result.streaming_data_token, "token2");
    }

    #[test]
    fn test_noop_provider_unavailable() {
        let provider = NoOpPoTokenProvider::default();
        assert!(!provider.is_available());
    }
}