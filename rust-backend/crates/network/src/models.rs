//! Track request/response types from the Innertube API.
//!
//! These are the minimal fields we need to make playback decisions.
//! Full decoding of `player` responses is done by `InnerTubeXResolver` in Kotlin.

use serde::Deserialize;

/// A response from the `player` endpoint.
#[derive(Debug, Clone, Deserialize)]
pub struct PlayerResponse {
    /// Playback tracking URLs for history pings.
    pub playback_tracking: Option<PlaybackTracking>,
    /// Whether the video is playable.
    pub playability_status: Option<PlayabilityStatus>,
    /// Streaming data containing format manifests.
    pub streaming_data: Option<StreamingData>,
}

/// Information about whether a video can be played.
#[derive(Debug, Clone, Deserialize)]
pub struct PlayabilityStatus {
    /// Status string: "OK", "UNPLAYABLE", "LOGIN_REQUIRED", etc.
    pub status: String,
    /// Optional human-readable reason.
    pub reason: Option<String>,
    /// Optional subreason.
    pub subreason: Option<String>,
}

/// Adaptive streaming manifests for a track.
#[derive(Debug, Clone, Deserialize)]
pub struct StreamingData {
    /// Available formats sorted by quality.
    pub formats: Vec<PlayerFormat>,
}

/// A single audio/video format available for streaming.
#[derive(Debug, Clone, Deserialize)]
pub struct PlayerFormat {
    /// Format identification string (e.g. "fDigits").
    pub itag: Option<u32>,
    /// MIME type (e.g. "audio/mp4", "audio/webm").
    #[serde(rename = "mimeType")]
    pub mime_type: Option<String>,
    /// Bitrate in bits per second.
    pub bitrate: Option<u32>,
    /// Sample rate in Hz.
    pub sample_rate: Option<u32>,
    /// Audio channel count.
    pub audio_channels: Option<u32>,
    /// URL for the stream segment.
    pub url: Option<String>,
    /// Base64-encoded signature (will need deobfuscation via JS).
    pub signature: Option<String>,
    pub content_length: Option<u64>,
}

/// URLs for playback-related stats pings.
#[derive(Debug, Clone, Deserialize)]
pub struct PlaybackTracking {
    /// Main playback URL.
    pub playback_url: Option<String>,
    /// Watchtime reporting URL.
    pub watchtime_url: Option<String>,
    /// Ad tracking URL.
    pub atr_url: Option<String>,
    /// When to fire the ad tracking (seconds into playback).
    pub atr_after_seconds: Option<u64>,
}

impl Default for PlayerFormat {
    fn default() -> Self {
        Self {
            itag: None,
            mime_type: None,
            bitrate: None,
            sample_rate: None,
            audio_channels: None,
            url: None,
            signature: None,
            content_length: None,
        }
    }
}