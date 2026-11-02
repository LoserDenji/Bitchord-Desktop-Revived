//! Application-level error types for BitChord
use thiserror::Error;
use std::path::PathBuf;

/// Result type alias for convenience
pub type Result<T> = std::result::Result<T, Error>;

/// Domain errors that can occur in the backend
#[derive(Debug, Error)]
pub enum Error {
    // Track/Song errors
    #[error("Track not found: {0}")]
    TrackNotFound(String),
    
    #[error("No audio tracks found in directory: {path}")]
    NoTracksInDirectory { path: PathBuf },
    
    #[error("Failed to read audio file: {path}")]
    CannotReadAudioFile { path: PathBuf, source: std::io::Error },
    
    #[error("Unsupported audio format: {0}")]
    UnsupportedFormat(String),
    
    // Album/Artist errors
    #[error("Album not found: {0}")]
    AlbumNotFound(String),
    
    #[error("Artist not found: {0}")]
    ArtistNotFound(String),
    
    // Playlist errors
    #[error("Playlist not found: {0}")]
    PlaylistNotFound(String),
    
    #[error("Cannot remove track from playlist: not found")]
    CannotRemoveTrackFromPlaylist,
    
    // Queue errors
    #[error("Queue is empty")]
    EmptyQueue,
    
    #[error("Invalid queue position: {0}")]
    InvalidQueuePosition(usize),
    
    // Playback errors
    #[error("Playback not active or device unavailable")]
    NoActivePlayback,
    
    #[error("Audio device error: {0}")]
    AudioDeviceError(String),
    
    #[error("Failed to decode audio stream")]
    DecodeError,
    
    #[error("Seek error: position {pos} outside range 0-{len}")]
    SeekOutOfRange { pos: u64, len: u64 },
    
    // Database errors
    #[error("Database error: {0}")]
    DatabaseError(String),
    
    #[error("Failed to open database: {path}")]
    CannotOpenDatabase { path: PathBuf, source: anyhow::Error },
    
    #[error("Migration failed: {0}")]
    MigrationFailed(String),
    
    // Configuration errors
    #[error("Configuration error: {0}")]
    ConfigError(String),
    
    #[error("Configuration file not found: {0}")]
    ConfigNotFound(PathBuf),
    
    #[error("Invalid configuration value for {key}: {value}")]
    InvalidConfig { key: String, value: String },
    
    // Library scanning errors
    #[error("Library scan interrupted")]
    LibraryScanInterrupted,
    
    #[error("Library path not accessible: {path}")]
    LibraryPathNotAccessible { path: PathBuf },
    
    // Sync errors
    #[error("Sync error: {0}")]
    SyncError(String),
    
    // Network errors
    #[error("Network error: {0}")]
    NetworkError(String),
    
    #[error("Authentication required")]
    AuthRequired,
    
    // System errors
    #[error("Permission denied: {0}")]
    PermissionDenied(String),
    
    #[error("Path traversal attempt blocked")]
    PathTraversalBlocked,
    
    #[error("Cache directory not available")]
    CacheNotAvailable,
    
    #[error("Temporary directory not available")]
    TempDirNotAvailable,
    
    // Serialization errors
    #[error("Serialization error: {0}")]
    SerializationError(String),
    
    // Other internal errors
    #[error("Internal error: {0}")]
    Internal(String),
    
    #[error("Operation cancelled")]
    Cancelled,
    
    // Search errors
    #[error("Search error: {0}")]
    SearchError(String),
    
    // Party/Listen Together errors
    #[error("Party not found: {0}")]
    PartyNotFound(String),
    
    #[error("Cannot join party: {0}")]
    CannotJoinParty(String),
    
    #[error("Party has reached maximum members")]
    PartyFull,
    
    // Playback source errors
    #[error("Unknown playback source type: {0}")]
    UnknownPlaybackSource(String),
}

impl Error {
    /// Check if this error is recoverable
    pub fn is_recoverable(&self) -> bool {
        matches!(
            self,
            Error::NoActivePlayback
                | Error::EmptyQueue
                | Error::TrackNotFound(_)
                | Error::AlbumNotFound(_)
                | Error::ArtistNotFound(_)
                | Error::PlaylistNotFound(_)
                | Error::NoTracksInDirectory { .. }
                | Error::NetworkError(_)
                | Error::SyncError(_)
        )
    }
    
    /// Check if this error is user-actionable
    pub fn requires_user_action(&self) -> bool {
        matches!(
            self,
            Error::CannotReadAudioFile { .. }
                | Error::LibraryPathNotAccessible { .. }
                | Error::PermissionDenied(_)
                | Error::PathTraversalBlocked
                | Error::ConfigNotFound(_)
                | Error::CacheNotAvailable
                | Error::TempDirNotAvailable
        )
    }
}
