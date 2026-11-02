//! Platform-specific integrations
//!
//! Provides Linux MPRIS 2.0 implementation and OS-specific hooks.

#[cfg(target_os = "linux")]
pub mod mpris;

#[cfg(not(target_os = "linux"))]
pub mod mpris {
    pub struct MprisManager;
    impl MprisManager {
        pub fn new() -> Self { Self }
        pub fn update(&self, _state: crate::bitchord_core::Track) {}
    }
}

pub use mpris::MprisManager;
