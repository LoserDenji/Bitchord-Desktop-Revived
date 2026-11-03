//! MPRIS stub implementation for non-Linux platforms.
//!
//! On non-Linux platforms (Windows, macOS), this provides empty implementations
//! to maintain a consistent API across all platforms.

use bitechord_core::Track;

/// Current MPRIS playback status (stub)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackStatus {
    Playing,
    Paused,
    Stopped,
}

impl std::fmt::Display for PlaybackStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlaybackStatus::Playing => write!(f, "Playing"),
            PlaybackStatus::Paused => write!(f, "Paused"),
            PlaybackStatus::Stopped => write!(f, "Stopped"),
        }
    }
}

/// Stub MPRIS state (non-Linux platforms)
#[derive(Debug, Clone)]
pub struct MprisState;

/// Manager for non-Linux platforms
#[derive(Clone)]
pub struct MprisManager;

impl MprisManager {
    pub fn new() -> Self {
        Self
    }

    pub fn update_playback(&self, _playing: bool) {}

    pub fn update_position(&self, _pos_ms: u64) {}

    pub fn update_track(&self, _track: Option<Track>) {}

    pub fn update_playback_state(&self, _playing: bool, _position_ms: u64, _track: Option<Track>) {}
}