//! PoToken types for YouTube BotGuard challenges.
//!
//! A PoToken is a JavaScript-generated integrity token minted by a browser
//! (or similar engine) to prove the client is not automated.
//!
//! **No browser automation is implemented yet** — this module provides the
//! types and interface that would be used by a future browser-based minter.
//! The [`PoTokenProvider`] trait can be implemented by any solution that
//! generates tokens (e.g., Playwright, headless Chrome, or a native Rust
//! implementation if one is reverse-engineered).

use std::fmt;

/// A generated PoToken with associated metadata.
#[derive(Debug, Clone)]
pub struct PoTokenResult {
    /// Token for the player request.
    pub player_request_token: String,
    /// Token for streaming data requests.
    pub streaming_data_token: String,
}

impl PoTokenResult {
    pub fn new(player_request_token: String, streaming_data_token: String) -> Self {
        Self {
            player_request_token,
            streaming_data_token,
        }
    }
}

/// Errors that can occur during PoToken generation.
#[derive(Debug)]
pub enum PoTokenError {
    /// The browser automation infrastructure is not available.
    BrowserUnavailable,
    /// The BotGuard challenge page failed to load.
    ChallengeLoadFailed(String),
    /// Token generation timed out.
    Timeout,
    /// The integrity token was invalid or expired.
    InvalidIntegrityToken,
    /// JavaScript execution failed.
    JsExecutionFailed(String),
    /// Unknown or unexpected error.
    Other(String),
}

impl fmt::Display for PoTokenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PoTokenError::BrowserUnavailable => write!(f, "browser automation unavailable"),
            PoTokenError::ChallengeLoadFailed(msg) => {
                write!(f, "challenge load failed: {msg}")
            }
            PoTokenError::Timeout => write!(f, "token generation timed out"),
            PoTokenError::InvalidIntegrityToken => {
                write!(f, "integrity token is invalid or expired")
            }
            PoTokenError::JsExecutionFailed(msg) => {
                write!(f, "JavaScript execution failed: {msg}")
            }
            PoTokenError::Other(msg) => write!(f, "PoToken error: {msg}"),
        }
    }
}

impl std::error::Error for PoTokenError {}

/// Trait for PoToken generation.
///
/// Implementations use a browser or similar mechanism to execute BotGuard
/// JavaScript and mint tokens bound to a specific identifier (visitor data or video ID).
///
/// **No browser automation is currently provided** — this is a placeholder
/// for a future implementation.
#[async_trait::async_trait]
pub trait PoTokenProvider: Send + Sync {
    /// Generate a PoToken for the given identifier.
    ///
    /// The identifier is typically either a `visitorData` string or a `videoId`.
    /// The returned token is bound to that identifier.
    async fn generate_token(&self, identifier: &str) -> Result<PoTokenResult, PoTokenError>;

    /// Check whether this provider is currently usable.
    ///
    /// Returns `false` if the browser is unavailable, crashed, or the
    /// underlying integrity token has expired.
    fn is_available(&self) -> bool;

    /// Close any resources held by the provider (e.g., browser pages).
    fn close(&mut self);
}

/// A no-op provider that refuses to generate tokens.
///
/// Use this when you want PoToken support in the type system but don't
/// require actual token generation (e.g., offline mode, or before browser
/// automation is implemented).
#[derive(Debug, Clone, Default)]
pub struct NoOpPoTokenProvider;

#[async_trait::async_trait]
impl PoTokenProvider for NoOpPoTokenProvider {
    async fn generate_token(&self, _identifier: &str) -> Result<PoTokenResult, PoTokenError> {
        Err(PoTokenError::BrowserUnavailable)
    }

    fn is_available(&self) -> bool {
        false
    }

    fn close(&mut self) {}
}

/// Build the JavaScript payload that the PoToken page needs to execute.
///
/// Takes a scrambled challenge response from YouTube's Create endpoint and
/// returns a JSON object that can be embedded in JavaScript.
pub fn build_js_challenge(challenge_data: &serde_json::Value) -> Result<String, PoTokenError> {
    // Minimal implementation: serialize the challenge data.
    Ok(serde_json::to_string(challenge_data)
        .map_err(|e| PoTokenError::JsExecutionFailed(e.to_string()))?)
}