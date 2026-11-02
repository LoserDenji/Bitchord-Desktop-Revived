//! Platform-specific integrations for BitChord Tauri backend.
//!
//! This crate provides platform-specific features:
//! - MPRIS 2.0 for Linux media control
//! - Mica/Acrylic for Windows
//! - Vibrancy for macOS

#[cfg(target_os = "linux")]
pub mod mpris;

#[cfg(target_os = "linux")]
pub use mpris::MprisManager;

#[cfg(not(target_os = "linux"))]
pub struct MprisManager;

#[cfg(not(target_os = "linux"))]
impl MprisManager {
    pub fn new() -> Self {
        Self
    }

    pub fn update_state(&self, _playing: bool) {}
}
