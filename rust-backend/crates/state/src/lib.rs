//! Application state management
use parking_lot::RwLock;
use std::sync::Arc;

use bitchord_core::{Library, UserSettings};

/// Global application state
#[derive(Debug)]
pub struct AppState {
    /// User settings
    pub settings: RwLock<UserSettings>,
    
    /// Music library
    pub library: RwLock<Option<Library>>,
    
    /// Playback state
    pub playback: RwLock<PlaybackState>,
    
    /// Queue state
    pub queue: RwLock<Vec<Uuid>>,
    
    /// Current track position
    pub current_position: RwLock<u64>,
    
    /// Is playback active
    pub is_playing: RwLock<bool>,
}

/// Playback state information
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct PlaybackState {
    /// Current track ID
    pub current_track: Option<Uuid>,
    /// Playback position in milliseconds
    pub position_ms: u64,
    /// Playback speed multiplier
    pub speed: f32,
    /// Is playback active
    pub is_playing: bool,
    /// Volume level (0.0 - 1.0)
    pub volume: f32,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            settings: RwLock::new(UserSettings::default()),
            library: RwLock::new(None),
            playback: RwLock::new(PlaybackState::default()),
            queue: RwLock::new(Vec::new()),
            current_position: RwLock::new(0),
            is_playing: RwLock::new(false),
        }
    }
}

impl AppState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
}