//! BitChord Tauri Application Library
//!
//! Library crate for the BitChord desktop music player.
//! Provides state management and command handling for the Tauri frontend.

pub use bitchord_commands::handlers;

#[cfg(feature = "logging")]
pub fn init_logging() {
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
    
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "bitchord=debug,tauri=info".into()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_placement() {
        // Placeholder test
    }
}