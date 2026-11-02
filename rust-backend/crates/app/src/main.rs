//! BitChord Tauri Application Entry Point (Binary)
//!
//! This is the main entry point for the BitChord desktop application binary.
//! It wires up the Tauri runtime, initializes application state,
//! and registers command handlers for library, sync, and playback tasks.

use tauri::{Manager, State};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use bitchord_state::AppState;
use bitchord_config::ConfigManager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Initialize logging
    #[cfg(debug_assertions)]
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "bitchord=debug,tauri=info".into()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();

    tracing::info!("Starting BitChord Tauri Application");

    // Initialize configuration
    let config = ConfigManager::new().expect("Failed to initialize configuration");

    // Initialize global application state
    let app_state = AppState::new();

    tauri::Builder::default()
        .manage(app_state)
        .manage(config)
        .setup(|app| {
            let _app_handle = app.app_handle();
            tracing::info!("Application setup complete");
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // Library Commands
            crate::handlers::scan_library,
            crate::handlers::get_library_stats,
            crate::handlers::list_tracks,
            crate::handlers::list_albums,
            crate::handlers::list_artists,
            // Sync Commands (Party Mode)
            crate::handlers::create_sync_session,
            crate::handlers::join_sync_session,
            crate::handlers::leave_sync_session,
            crate::handlers::broadcast_playback_state,
            // Playback Commands
            crate::handlers::play_track,
            crate::handlers::pause_playback,
            crate::handlers::stop_playback,
            crate::handlers::next_track,
            crate::handlers::previous_track,
            crate::handlers::seek_to,
            crate::handlers::set_volume,
            crate::handlers::get_playback_state,
            crate::handlers::toggle_shuffle,
            crate::handlers::toggle_repeat,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

fn main() {
    run();
}