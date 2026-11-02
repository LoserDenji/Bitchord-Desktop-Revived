//! BitChord Tauri Application Entry Point
//!
//! This is the main entry point for the BitChord desktop application.
//! It wires up the Tauri runtime, initializes application state,
//! and registers command handlers for library, sync, and playback tasks.

use tauri::{Manager, State};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use bitchord_state::AppState;
use bitchord_config::ConfigManager;
use bitchord_app::handlers;

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
            handlers::scan_library,
            handlers::get_library_stats,
            handlers::list_tracks,
            handlers::list_albums,
            handlers::list_artists,
            
            // Sync Commands (Party Mode)
            handlers::create_sync_session,
            handlers::join_sync_session,
            handlers::leave_sync_session,
            handlers::broadcast_playback_state,
            
            // Playback Commands
            handlers::play_track,
            handlers::pause_playback,
            handlers::stop_playback,
            handlers::next_track,
            handlers::previous_track,
            handlers::seek_to,
            handlers::set_volume,
            handlers::get_playback_state,
            handlers::toggle_shuffle,
            handlers::toggle_repeat,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

fn main() {
    run();
}