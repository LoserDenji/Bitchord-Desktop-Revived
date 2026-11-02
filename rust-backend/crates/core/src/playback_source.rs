//! Playback source - represents what we're playing from
use serde::{Deserialize, Serialize};

/// Where the current playback is coming from
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PlaybackSource {
    /// Library track by ID
    Track { track_id: uuid::Uuid },
    /// External URL or stream
    Stream { url: String, metadata: Option<TrackMetadata> },
    /// Playlist by ID
    Playlist {
        playlist_id: uuid::Uuid,
        position: usize,
    },
    /// Artist radio / similar tracks
    ArtistRadio { artist_id: uuid::Uuid },
    /// Album playback
    Album { album_id: uuid::Uuid, track_index: usize },
    /// User-defined playlist
    Custom { name: String, items: Vec<PlaylistItem> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackMetadata {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaylistItem {
    pub id: uuid::Uuid,
    pub title: Option<String>,
}

/// A seekable range for the current playback position
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