//! BitChord Tauri Application
//!
//! Main entry point for the Tauri desktop application.
//! Wires up all command handlers and initializes the app state.

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            // Initialize app state
            let state = bitchord_state::AppState::new();
            app.manage(state);
            
            // Initialize logging in debug mode
            #[cfg(debug_assertions)]
            {
                let _ = app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                );
            }
            
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_platform,
            greet,
            bitchord_commands::handlers::scan_library,
            bitchord_commands::handlers::get_library_stats,
            bitchord_commands::handlers::list_tracks,
            bitchord_commands::handlers::list_albums,
            bitchord_commands::handlers::list_artists,
            bitchord_commands::handlers::create_sync_session,
            bitchord_commands::handlers::join_sync_session,
            bitchord_commands::handlers::leave_sync_session,
            bitchord_commands::handlers::broadcast_playback_state,
            bitchord_commands::handlers::play_track,
            bitchord_commands::handlers::pause_playback,
            bitchord_commands::handlers::stop_playback,
            bitchord_commands::handlers::next_track,
            bitchord_commands::handlers::previous_track,
            bitchord_commands::handlers::seek_to,
            bitchord_commands::handlers::set_volume,
            bitchord_commands::handlers::get_playback_state,
            bitchord_commands::handlers::toggle_shuffle,
            bitchord_commands::handlers::toggle_repeat,
        ])
        .run(tauri::generate_context!())
        .expect("error while building tauri application");
}

#[tauri::command]
fn get_platform() -> String {
    if cfg!(target_os = "windows") {
        "windows".to_string()
    } else if cfg!(target_os = "macos") {
        "macos".to_string()
    } else if cfg!(target_os = "linux") {
        "linux".to_string()
    } else {
        "unknown".to_string()
    }
}

#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! Welcome to BitChord.", name)
}
