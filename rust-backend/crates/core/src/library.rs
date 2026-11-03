//! Library representation
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Music library statistics
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LibraryStats {
    /// Total number of tracks
    pub total_tracks: usize,
    /// Total number of albums
    pub total_albums: usize,
    /// Total number of artists
    pub total_artists: usize,
    /// Total number of playlists
    pub total_playlists: usize,
    /// Total duration of all tracks in seconds
    pub total_duration_seconds: u64,
    /// Library size on disk in bytes
    pub library_size_bytes: u64,
    /// Last scan timestamp
    pub last_scanned: Option<DateTime<Utc>>,
    /// Total artwork files
    pub total_artwork: usize,
}

/// A music library containing tracks, albums, artists, and playlists
use crate::track::Track;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Library {
    /// Root folder of the library
    pub root_path: PathBuf,
    
    /// Library statistics
    pub stats: LibraryStats,
    
    /// Library name
    pub name: String,
    
    /// Library created timestamp
    pub created_at: DateTime<Utc>,
    
    /// Last updated timestamp
    pub updated_at: DateTime<Utc>,
}

impl Library {
    /// Find a track by its UUID
    pub fn find_track(&self, id: uuid::Uuid) -> Option<&Track> {
        // This would search through the library's tracks
        // For now, return None since tracks are stored in the database
        None
    }

    pub fn new(root_path: PathBuf, name: String) -> Self {
        Self {
            root_path,
            stats: LibraryStats::default(),
            name,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }
}