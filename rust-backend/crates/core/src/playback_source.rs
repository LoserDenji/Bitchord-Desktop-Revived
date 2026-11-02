//! Playback source - represents what we're playing from
use serde::{Deserialize, Serialize};

/// The type of playback source
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PlaybackSourceType {
    /// A YouTube Music track
    Track,
    /// An album
    Album,
    /// An artist page
    Artist,
    /// A playlist
    Playlist,
    /// Radio station
    Radio,
}

/// Where the current playback is coming from
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaybackSource {
    /// The video ID or browse ID of the source
    pub id: String,
    /// The type of the source (track, album, artist, playlist, etc.)
    #[serde(default)]
    pub source_type: Option<PlaybackSourceType>,
    /// When this track started playing
    #[serde(default)]
    pub start_time: Option<f64>,
}

/// A seekable range for the current playback position (existing content)
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SeekableRange {
    /// Start of the range (in ms)
    pub start: u64,
    /// End of the range (in ms), or 0 for full track
    pub end: u64,
}

impl SeekableRange {
    pub fn full(duration_ms: u64) -> Self {
        Self { start: 0, end: 0 }
    }
    
    pub fn bounded(start: u64, duration_ms: u64) -> Self {
        Self { start, end: duration_ms.saturating_sub(1000) }
    }
    
    pub fn duration(&self) -> u64 {
        if self.end == 0 {
            0 // Unknown/full track
        } else {
            self.end.saturating_sub(self.start)
        }
    }
}