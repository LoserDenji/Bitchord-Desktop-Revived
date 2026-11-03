//! Platform-specific integrations for BitChord Tauri backend.
//!
//! This crate provides platform-specific features:
//! - MPRIS 2.0 for Linux media control
//! - Mica/Acrylic for Windows
//! - Vibrancy for macOS

#[cfg(target_os = "linux")]
mod mpris;

#[cfg(target_os = "linux")]
pub use mpris::{MprisManager, MprisState, PlaybackStatus};

#[cfg(not(target_os = "linux"))]
mod mpris_stub;

#[cfg(not(target_os = "linux"))]
pub use mpris_stub::MprisManager;