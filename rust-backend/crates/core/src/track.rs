//! Track - Audio file representation with metadata
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::album::Album;
use super::artist::Artist;

/// A track represents a single audio file with its metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    /// Unique identifier for this track
    pub id: Uuid,
    
    /// Track title
    pub title: String,
    
    /// Artist information
    pub artist: Vec<Artist>,
    
    /// Album information
    pub album: Option<Album>,
    
    /// Track number on the album (1-indexed)
    pub track_number: Option<u32>,
    
    /// Disc number (for multi-disc albums, 1-indexed)
    pub disc_number: Option<u32>,
    
    /// Total tracks on the album
    pub album_total_tracks: Option<u32>,
    
    /// Duration in milliseconds
    pub duration_ms: u64,
    
    /// Audio bitrate in kbps
    pub bitrate: Option<u32>,
    
    /// Audio sample rate in Hz
    pub sample_rate: Option<u32>,
    
    /// Audio channel count (1 = mono, 2 = stereo, etc.)
    pub channels: Option<u16>,
    
    /// File size in bytes
    pub file_size_bytes: Option<u64>,
    
    /// Full path to the audio file on disk
    pub file_path: std::path::PathBuf,
    
    /// Date when the track was added to the library
    pub added_at: DateTime<Utc>,
    
    /// Date when the track was last modified (from filesystem)
    pub modified_at: Option<DateTime<Utc>>,
    
    /// File modification time (for sync detection)
    pub file_modified_at: Option<DateTime<Utc>>,
    
    /// Genre tags
    pub genres: Vec<String>,
    
    /// Lyrics (plain text)
    pub lyrics: Option<String>,
    
    /// Lyrics with timing information
    pub timed_lyrics: Option<Vec<LyricLine>>,
    
    /// Composer credits
    pub composers: Vec<String>,
    
    /// Copyright info
    pub copyright: Option<String>,
    
    /// Rating from user (0-5 stars)
    pub rating: Option<u8>,
    
    /// Play count
    pub play_count: u32,
    
    /// Skip count
    pub skip_count: u32,
    
    /// Relative path from the music library root
    pub relative_path: std::path::PathBuf,
}

/// A single lyric line with timing information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LyricLine {
    /// Start time in milliseconds
    pub start_ms: u64,
    /// End time in milliseconds (0 = end of track)
    pub end_ms: u64,
    /// Lyric text
    pub text: String,
}

impl Track {
    /// Create a new track with the given parameters
    pub fn new(
        title: impl Into<String>,
        file_path: impl Into<std::path::PathBuf>,
        duration_ms: u64,
        added_at: DateTime<Utc>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            title: title.into(),
            artist: Vec::new(),
            album: None,
            track_number: None,
            disc_number: None,
            album_total_tracks: None,
            duration_ms,
            bitrate: None,
            sample_rate: None,
            channels: None,
            file_size_bytes: None,
            file_path: file_path.into(),
            added_at,
            modified_at: None,
            file_modified_at: None,
            genres: Vec::new(),
            lyrics: None,
            timed_lyrics: None,
            composers: Vec::new(),
            copyright: None,
            rating: None,
            play_count: 0,
            skip_count: 0,
            relative_path: std::path::PathBuf::new(),
        }
    }

    /// Get the display title (title + artist if needed for disambiguation)
    pub fn display_title(&self) -> String {
        if self.artist.is_empty() {
            self.title.clone()
        } else {
            format!(
                "{} - {}",
                self.title,
                self.artist
                    .iter()
                    .map(|a| a.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    }

    /// Check if this track is a video track (has video streams)
    pub fn is_video(&self) -> bool {
        // Video tracks typically have specific extensions or container formats
        matches!(
            self.file_path.extension().map(|s| s.to_str()),
            Some(Some("mp4") | Some("mkv") | Some("avi") | Some("mov") | Some("webm"))
        )
    }
}