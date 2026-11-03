//! Linux MPRIS 2.0 D-Bus implementation.
//!
//! MPRIS exposes two D-Bus interfaces on the same object path:
//! - `org.mpris.MediaPlayer2` (root)
//! - `org.mpris.MediaPlayer2.Player` (player controls)
//!
//! zbus requires one interface per type, so we split them into `MprisRoot` and
//! `MprisPlayer` and register both at `/org/mpris/MediaPlayer2`.

use anyhow::Result;
use bitchord_core::Track;
use parking_lot::RwLock;
use std::sync::Arc;
use tracing::info;

/// Current MPRIS playback status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackStatus {
    Playing,
    Paused,
    Stopped,
}

impl std::fmt::Display for PlaybackStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_'>) -> std::fmt::Result {
        match self {
            PlaybackStatus::Playing => write!(f, "Playing"),
            PlaybackStatus::Paused => write!(f, "Paused"),
            PlaybackStatus::Stopped => write!(f, "Stopped"),
        }
    }
}

/// Thread-safe MPRIS state holder
#[derive(Debug, Clone)]
pub struct MprisState {
    pub playback_status: RwLock<PlaybackStatus>,
    pub position: RwLock<u64>,
    pub track: RwLock<Option<Track>>,
    pub playback_rate: RwLock<f64>,
}

impl MprisState {
    pub fn new() -> Self {
        Self {
            playback_status: RwLock::new(PlaybackStatus::Stopped),
            position: RwLock::new(0),
            track: RwLock::new(None),
            playback_rate: RwLock::new(1.0),
        }
    }

    pub fn update_playback(&self, playing: bool) {
        *self.playback_status.write() = if playing {
            PlaybackStatus::Playing
        } else {
            PlaybackStatus::Paused
        };
    }

    pub fn update_position(&self, pos_ms: u64) {
        *self.position.write() = pos_ms;
    }

    pub fn update_track(&self, track: Option<Track>) {
        *self.track.write() = track;
    }
}

/// Root MPRIS interface: identity, quit, raise.
#[cfg(target_os = "linux")]
#[derive(Clone)]
pub struct MprisRoot {
    pub(crate) state: Arc<MprisState>,
}

#[cfg(target_os = "linux")]
#[zbus::dbus_interface(name = "org.mpris.MediaPlayer2")]
impl MprisRoot {
    /// The identity of the media player.
    #[dbus_interface(property)]
    fn identity(&self) -> &str {
        "BitChord"
    }

    /// Can the player quit?
    #[dbus_interface(property)]
    fn can_quit(&self) -> bool {
        false
    }

    /// Whether the player supports playback
    #[dbus_interface(property)]
    fn supported_media_session_types(&self) -> &[&str] {
        &[]
    }

    /// Raise the player's window.
    fn raise(&self) {
        info!("MPRIS: Raise requested");
    }

    /// Quit the player.
    fn quit(&self) {
        info!("MPRIS: Quit requested");
    }
}

/// Player MPRIS interface: playback status, play/pause, next/previous.
#[cfg(target_os = "linux")]
#[derive(Clone)]
pub struct MprisPlayer {
    pub(crate) state: Arc<MprisState>,
}

#[cfg(target_os = "linux")]
#[zbus::dbus_interface(name = "org.mpris.MediaPlayer2.Player")]
impl MprisPlayer {
    /// Current playback status.
    #[dbus_interface(property)]
    fn playback_status(&self) -> String {
        self.state.playback_status.read().to_string()
    }

    /// Current playback rate.
    #[dbus_interface(property)]
    fn playback_rate(&self) -> f64 {
        *self.state.playback_rate.read()
    }

    /// Volume control (0.0 to 1.0).
    #[dbus_interface(property)]
    fn volume(&self) -> f64 {
        1.0
    }

    /// Current position in microseconds.
    #[dbus_interface(property)]
    fn position(&self) -> i64 {
        *self.state.position.read() as i64 * 1000
    }

    /// Minimum valid seek position in microseconds.
    #[dbus_interface(property)]
    fn minimum_progress(&self) -> i64 {
        0
    }

    /// Maximum valid seek position in microseconds.
    #[dbus_interface(property)]
    fn maximum_progress(&self) -> i64 {
        i64::MAX
    }

    /// Track metadata.
    #[dbus_interface(property)]
    fn metadata(&self) -> zbus::exported_value::ExportedValue {
        let track = self.state.track.read();
        if let Some(ref t) = *track {
            let mut map = std::collections::HashMap::new();
            map.insert("xesam:title", &t.title);
            map.insert("xesam:artist", &vec![t.artist.clone()]);
            map.insert("mpris:album", &t.album);
            return zbus::exported_value::ExportedValue::new(map);
        }
        zbus::exported_value::ExportedValue::new(std::collections::HashMap::new())
    }

    /// Play the media.
    fn play(&self) {
        info!("MPRIS: Play requested");
    }

    /// Pause the media.
    fn pause(&self) {
        info!("MPRIS: Pause requested");
    }

    /// Toggle between play and pause.
    fn play_pause(&self) {
        info!("MPRIS: Play/Pause requested");
    }

    /// Go to the next track.
    fn next(&self) {
        info!("MPRIS: Next requested");
    }

    /// Go to the previous track.
    fn previous(&self) {
        info!("MPRIS: Previous requested");
    }

    /// Stop playback.
    fn stop(&self) {
        info!("MPRIS: Stop requested");
    }

    /// Set position (seek).
    fn set_position(&self, _position: i64) {
        info!("MPRIS: Set position requested");
    }
}

/// Manager that owns and registers both MPRIS interfaces on the D-Bus session bus.
#[cfg(target_os = "linux")]
pub struct MprisManager {
    pub state: Arc<MprisState>,
}

#[cfg(target_os = "linux")]
impl MprisManager {
    pub fn new() -> Self {
        Self {
            state: Arc::new(MprisState::new()),
        }
    }

    /// Update playback state
    pub fn update_playback(&self, playing: bool) {
        self.state.update_playback(playing);
    }

    /// Update position
    pub fn update_position(&self, pos_ms: u64) {
        self.state.update_position(pos_ms);
    }

    /// Update current track
    pub fn update_track(&self, track: Option<Track>) {
        self.state.update_track(track);
    }

    /// Update all playback state at once
    pub fn update_playback_state(&self, playing: bool, position_ms: u64, track: Option<Track>) {
        self.state.update_playback(playing);
        self.state.update_position(position_ms);
        self.state.update_track(track);
    }

    /// Register both MPRIS interfaces at `/org/mpris/MediaPlayer2` on the session bus.
    pub async fn register(self, conn: &zbus::Connection) -> Result<()> {
        let object_server = conn.object_server();

        let root = MprisRoot {
            state: self.state.clone(),
        };
        let player = MprisPlayer {
            state: self.state.clone(),
        };

        object_server
            .at("/org/mpris/MediaPlayer2", root)
            .await?;
        object_server
            .at("/org/mpris/MediaPlayer2/Player", player)
            .await?;

        info!("MPRIS: Registered on D-Bus at /org/mpris/MediaPlayer2");

        Ok(())
    }
}

// Non-Linux stub implementations
#[cfg(not(target_os = "linux"))]
#[derive(Clone)]
pub struct MprisManager;

#[cfg(not(target_os = "linux"))]
impl MprisManager {
    pub fn new() -> Self {
        Self
    }

    pub fn update_state(&self, _playing: bool) {}
}
