//! Linux MPRIS 2.0 D-Bus implementation.
//!
//! MPRIS exposes two D-Bus interfaces on the same object path:
//! - `org.mpris.MediaPlayer2` (root)
//! - `org.mpris.MediaPlayer2.Player` (player controls)
//!
//! zbus requires one interface per type, so we split them into `MprisRoot` and
//! `MprisPlayer` and register both at `/org/mpris/MediaPlayer2`.

use zbus::interface;

/// Root MPRIS interface: identity, quit, raise.
#[cfg(target_os = "linux")]
#[derive(Clone)]
pub struct MprisRoot;

#[cfg(target_os = "linux")]
#[interface(name = "org.mpris.MediaPlayer2")]
impl MprisRoot {
    /// The identity of the media player.
    #[zbus(property)]
    fn identity(&self) -> &str {
        "BitChord"
    }

    /// Can the player quit?
    #[zbus(property)]
    fn can_quit(&self) -> bool {
        false
    }

    /// Raise the player's window.
    fn raise(&self) {}

    /// Quit the player.
    fn quit(&self) {}
}

/// Player MPRIS interface: playback status, play/pause, next/previous.
#[cfg(target_os = "linux")]
#[derive(Clone)]
pub struct MprisPlayer;

#[cfg(target_os = "linux")]
#[interface(name = "org.mpris.MediaPlayer2.Player")]
impl MprisPlayer {
    /// The current playback status.
    #[zbus(property)]
    fn playback_status(&self) -> &str {
        "Stopped"
    }

    /// The current playback rate.
    #[zbus(property)]
    fn playback_rate(&self) -> f64 {
        1.0
    }

    /// Play the media.
    fn play(&self) {}

    /// Pause the media.
    fn pause(&self) {}

    /// Toggle between play and pause.
    fn play_pause(&self) {}

    /// Go to the next track.
    fn next(&self) {}

    /// Go to the previous track.
    fn previous(&self) {}
}

/// Manager that owns and registers both MPRIS interfaces on the D-Bus session bus.
#[cfg(target_os = "linux")]
pub struct MprisManager;

#[cfg(target_os = "linux")]
impl MprisManager {
    pub fn new() -> Self {
        Self
    }

    /// Register both MPRIS interfaces at `/org/mpris/MediaPlayer2` on the session bus.
    pub async fn register(self, conn: &zbus::Connection) -> zbus::Result<()> {
        let object_server = conn.object_server();

        let root = MprisRoot;
        let player = MprisPlayer;

        object_server
            .at("/org/mpris/MediaPlayer2", root)
            .await?;
        object_server
            .at("/org/mpris/MediaPlayer2", player)
            .await?;

        Ok(())
    }
}
