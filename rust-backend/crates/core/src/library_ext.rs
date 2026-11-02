//! Library-related extensions
use serde::{Deserialize, Serialize};

/// Track metadata for library scanning
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackMetadata {
    /// Track title
    pub title: String,
    /// Artist names
    pub artists: Vec<String>,
    /// Album title
    pub album: Option<String>,
    /// Genre tags
    pub genres: Vec<String>,
    /// Track number
    pub track_number: Option<u32>,
    /// Disc number
    pub disc_number: Option<u32>,
    /// Duration in milliseconds
    pub duration_ms: u64,
    /// Bitrate in kbps
    pub bitrate: Option<u32>,
    /// Sample rate in Hz
    pub sample_rate: Option<u32>,
    /// Channels
    pub channels: Option<u16>,
    /// File path
    pub file_path: std::path::PathBuf,
    /// File size in bytes
    pub file_size: Option<u64>,
    /// Date added
    pub date_added: chrono::DateTime<chrono::Utc>,
    /// Date modified
    pub date_modified: Option<chrono::DateTime<chrono::Utc>>,
    /// Art path
    pub art_path: Option<std::path::PathBuf>,
}

/// Result of a library scan operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanResult {
    /// Number of tracks scanned
    pub tracks_scanned: usize,
    /// Number of tracks added
    pub tracks_added: usize,
    /// Number of tracks updated
    pub tracks_updated: usize,
    /// Number of files skipped (already up to date)
    pub tracks_skipped: usize,
    /// Time elapsed in milliseconds
    pub duration_ms: u64,
    /// Any errors encountered
    pub errors: Vec<scan_error::ScanError>,
}

pub mod scan_error {
    use chrono::{DateTime, Utc};
    use serde::{Deserialize, Serialize};
    use std::path::PathBuf;

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct ScanError {
        pub path: PathBuf,
        pub error: String,
        pub timestamp: DateTime<Utc>,
    }
}