//! User settings configuration
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UserSettings {
    /// General playback settings
    pub playback: PlaybackSettings,
    
    /// Library paths and scanning
    pub library: LibrarySettings,
    
    /// Appearance and UI preferences
    pub appearance: AppearanceSettings,
    
    /// Network and streaming preferences
    pub network: NetworkSettings,
    
    /// Notifications and alerts
    pub notifications: NotificationSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PlaybackSettings {
    /// Volume level (0-100)
    pub volume: u8,
    
    /// Default playback speed
    pub default_speed: super::player::PlaybackSpeed,
    
    /// Crossfade duration in milliseconds
    pub crossfade_ms: u32,
    
    /// Gapless playback enabled
    pub gapless: bool,
    
    /// ReplayGain enabled
    pub replay_gain: bool,
    
    /// Normalize volume
    pub normalize: bool,
    
    /// Sleep timer enabled
    pub sleep_timer_enabled: bool,
    
    /// Sleep timer duration in seconds
    pub sleep_timer_seconds: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LibrarySettings {
    /// Music library folders to scan
    pub watch_folders: Vec<PathBuf>,
    
    /// Ignore patterns for scanning
    pub ignore_patterns: Vec<String>,
    
    /// Include hidden files
    pub include_hidden: bool,
    
    /// Auto-scan on startup
    pub auto_scan: bool,
    
    /// Scan interval in seconds (0 = disabled)
    pub scan_interval: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppearanceSettings {
    /// Theme: light, dark, system
    pub theme: String,
    
    /// Language: en, hi, etc.
    pub language: String,
    
    /// Show album art in notifications
    pub show_notifications: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NetworkSettings {
    /// Cache directory for downloads
    pub cache_dir: Option<PathBuf>,
    
    /// Maximum cache size in MB
    pub max_cache_mb: u64,
    
    /// Streaming quality: low, medium, high, original
    pub streaming_quality: String,
    
    /// Proxy settings
    pub proxy: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NotificationSettings {
    /// Show when track changes
    pub track_change: bool,
    
    /// Show when playlist ends
    pub playlist_end: bool,
    
    /// Show when library is updated
    pub library_update: bool,
}