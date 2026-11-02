//! Linux MPRIS 2.0 D-Bus implementation
#[cfg(target_os = "linux")]
use zbus::ConnectionBuilder;
use std::sync::Arc;

pub struct MprisManager {
    // We'll add state handling here
}

impl MprisManager {
    pub fn new() -> Self {
        Self {}
    }
}

// Minimal implementation skeleton for MPRIS
#[cfg(target_os = "linux")]
struct MprisPlayer {
    // state
}

#[cfg(target_os = "linux")]
#[zbus::interface(name = "org.mpris.MediaPlayer2")]
impl MprisPlayer {
    #[zbus::property]
    fn identity(&self) -> &str { "BitChord" }

    #[zbus::property]
    fn can_quit(&self) -> bool { false }

    fn raise(&self) {}
    fn quit(&self) {}
}

#[cfg(target_os = "linux")]
#[zbus::interface(name = "org.mpris.MediaPlayer2.Player")]
impl MprisPlayer {
    #[zbus::property]
    fn playback_status(&self) -> &str { "Stopped" }
    
    fn play(&self) {}
    fn pause(&self) {}
    fn play_pause(&self) {}
    fn next(&self) {}
    fn previous(&self) {}
}
