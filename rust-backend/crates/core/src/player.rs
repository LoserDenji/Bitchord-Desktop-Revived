//! Playback control state
use serde::{Deserialize, Serialize};

/// Playback speed multiplier (0.5x to 2.0x)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlaybackSpeed {
    X05,
    X075,
    X10,
    X115,
    X125,
    X15,
    X175,
    X20,
}

impl Default for PlaybackSpeed {
    fn default() -> Self {
        PlaybackSpeed::X10
    }
}

impl PlaybackSpeed {
    pub fn multiplier(&self) -> f32 {
        match self {
            PlaybackSpeed::X05 => 0.5,
            PlaybackSpeed::X075 => 0.75,
            PlaybackSpeed::X10 => 1.0,
            PlaybackSpeed::X115 => 1.15,
            PlaybackSpeed::X125 => 1.25,
            PlaybackSpeed::X15 => 1.5,
            PlaybackSpeed::X175 => 1.75,
            PlaybackSpeed::X20 => 2.0,
        }
    }
}

/// Repeat mode for playback
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum RepeatMode {
    #[default]
    None,
    One,
    All,
}

/// Shuffle mode for playback
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ShuffleMode {
    #[default]
    Off,
    Tracks,
    Albums,
    Artists,
}