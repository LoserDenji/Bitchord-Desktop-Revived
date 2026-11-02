//! Audio playback engine and library scanner/watcher tools

use anyhow::Result;
use bitchord_core::{PlaybackSpeed, TrackId};

pub mod scanner;
pub mod watcher;

pub use scanner::AudioScanner;
pub use watcher::AudioWatcher;

/// Audio player trait
#[async_trait::async_trait]
pub trait AudioPlayer: Send + Sync {
    async fn play(&self, track_id: TrackId) -> Result<()>;
    async fn pause(&self) -> Result<()>;
    async fn stop(&self) -> Result<()>;
    async fn next(&self) -> Result<()>;
    async fn previous(&self) -> Result<()>;
    async fn seek(&self, position_ms: u64) -> Result<()>;
    async fn set_volume(&self, volume: f32) -> Result<()>;
    async fn set_speed(&self, speed: PlaybackSpeed) -> Result<()>;
    async fn get_position(&self) -> Result<u64>;
    async fn is_playing(&self) -> Result<bool>;
}

/// Mock audio player for testing
pub struct MockAudioPlayer;

#[async_trait::async_trait]
impl AudioPlayer for MockAudioPlayer {
    async fn play(&self, _track_id: TrackId) -> Result<()> {
        Ok(())
    }
    async fn pause(&self) -> Result<()> {
        Ok(())
    }
    async fn stop(&self) -> Result<()> {
        Ok(())
    }
    async fn next(&self) -> Result<()> {
        Ok(())
    }
    async fn previous(&self) -> Result<()> {
        Ok(())
    }
    async fn seek(&self, _position_ms: u64) -> Result<()> {
        Ok(())
    }
    async fn set_volume(&self, _volume: f32) -> Result<()> {
        Ok(())
    }
    async fn set_speed(&self, _speed: PlaybackSpeed) -> Result<()> {
        Ok(())
    }
    async fn get_position(&self) -> Result<u64> {
        Ok(0)
    }
    async fn is_playing(&self) -> Result<bool> {
        Ok(false)
    }
}