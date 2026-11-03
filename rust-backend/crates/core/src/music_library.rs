//! Music library scanning and indexing
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A folder in the watched music library
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibraryFolder {
    /// Path to the folder
    pub path: PathBuf,
    /// Number of tracks in this folder
    pub track_count: usize,
    /// Whether this folder is being watched
    pub watched: bool,
    /// Last scanned timestamp
    pub last_scanned: Option<chrono::DateTime<chrono::Utc>>,
}

/// Library scanner configuration (legacy, use library_ext::ScanConfig)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibraryScanConfig {
    /// Root paths to scan
    pub root_paths: Vec<PathBuf>,
    /// File extensions to include
    pub include_extensions: Vec<String>,
    /// Patterns to ignore (glob)
    pub ignore_patterns: Vec<String>,
    /// Whether to include subdirectories
    pub recursive: bool,
    /// Whether to include hidden files
    pub include_hidden: bool,
    /// Pause between batches (ms) for responsiveness
    pub batch_pause_ms: u64,
}

impl Default for LibraryScanConfig {
    fn default() -> Self {
        Self {
            root_paths: Vec::new(),
            include_extensions: vec![
                "mp3".to_string(),
                "flac".to_string(),
                "ogg".to_string(),
                "m4a".to_string(),
                "wav".to_string(),
                "opus".to_string(),
                "wma".to_string(),
                "alac".to_string(),
            ],
            ignore_patterns: vec![
                ".*".to_string(),
                "**/Thumbs.db".to_string(),
                "**/Desktop.ini".to_string(),
            ],
            recursive: true,
            include_hidden: false,
            batch_pause_ms: 10,
        }
    }
}