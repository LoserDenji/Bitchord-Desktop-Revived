//! BitChord Tauri Application
//!
//! Main entry point for the Tauri desktop application.
//! Wires up all command handlers and initializes the app state.

use tauri::Manager;

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
            scan_library,
            get_library_stats,
            list_tracks,
            list_albums,
            list_artists,
            create_sync_session,
            join_sync_session,
            leave_sync_session,
            broadcast_playback_state,
            play_track,
            pause_playback,
            stop_playback,
            next_track,
            previous_track,
            seek_to,
            set_volume,
            get_playback_state,
            toggle_shuffle,
            toggle_repeat,
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

// Library commands
#[tauri::command]
async fn scan_library(state: tauri::State<'_,_ bitchord_state::AppState>, path: String) -> Result<bitchord_commands::LibraryScanResult, String> {
    bitchord_commands::library::scan_library(&state, path).await
}

#[tauri::command]
fn get_library_stats(state: tauri::State<'_,_ bitchord_state::AppState>) -> Result<bitchord_core::LibraryStats, String> {
    bitchord_commands::library::get_library_stats(&state)
}

#[tauri::command]
fn list_tracks(state: tauri::State<'_,_ bitchord_state::AppState>) -> Result<Vec<bitchord_core::Track>, String> {
    bitchord_commands::library::list_tracks(&state)
}

#[tauri::command]
fn list_albums(state: tauri::State<'_,_ bitchord_state::AppState>) -> Result<Vec<bitchord_core::Album>, String> {
    bitchord_commands::library::list_albums(&state)
}

#[tauri::command]
fn list_artists(state: tauri::State<'_,_ bitchord_state::AppState>) -> Result<Vec<bitchord_core::Artist>, String> {
    bitchord_commands::library::list_artists(&state)
}

// Sync commands
#[tauri::command]
fn create_sync_session(state: tauri::State<'_,_ bitchord_state::AppState>, session_name: String, participant_name: String) -> Result<String, String> {
    bitchord_commands::sync::create_sync_session(&state, session_name, participant_name)
}

#[tauri::command]
fn join_sync_session(state: tauri::State<'_,_ bitchord_state::AppState>, session_id: String, participant_name: String) -> Result<(), String> {
    bitchord_commands::sync::join_sync_session(&state, session_id, participant_name)
}

#[tauri::command]
fn leave_sync_session(state: tauri::State<'_,_ bitchord_state::AppState>, session_id: String) -> Result<(), String> {
    bitchord_commands::sync::leave_sync_session(&state, session_id)
}

#[tauri::command]
fn broadcast_playback_state(state: tauri::State<'_,_ bitchord_state::AppState>, session_id: String, message: bitchord_sync::SyncMessage) -> Result<(), String> {
    bitchord_commands::sync::broadcast_playback_state(&state, session_id, message)
}

// Playback commands
#[tauri::command]
fn play_track(state: tauri::State<'_,_ bitchord_state::AppState>, track_id: uuid::Uuid) -> Result<(), String> {
    bitchord_commands::playback::play_track(&state, track_id)
}

#[tauri::command]
fn pause_playback(state: tauri::State<'_,_ bitchord_state::AppState>) -> Result<(), String> {
    bitchord_commands::playback::pause_playback(&state)
}

#[tauri::command]
fn stop_playback(state: tauri::State<'_,_ bitchord_state::AppState>) -> Result<(), String> {
    bitchord_commands::playback::stop_playback(&state)
}

#[tauri::command]
fn next_track(state: tauri::State<'_,_ bitchord_state::AppState>) -> Result<(), String> {
    bitchord_commands::playback::next_track(&state)
}

#[tauri::command]
fn previous_track(state: tauri::State<'_,_ bitchord_state::AppState>) -> Result<(), String> {
    bitchord_commands::playback::previous_track(&state)
}

#[tauri::command]
fn seek_to(state: tauri::State<'_,_ bitchord_state::AppState>, position_ms: u64) -> Result<(), String> {
    bitchord_commands::playback::seek_to(&state, position_ms)
}

#[tauri::command]
fn set_volume(state: tauri::State<'_,_ bitchord_state::AppState>, volume: f32) -> Result<(), String> {
    bitchord_commands::playback::set_volume(&state, volume)
}

#[tauri::command]
fn get_playback_state(state: tauri::State<'_,_ bitchord_state::AppState>) -> Result<bitchord_state::PlaybackStateResponse, String> {
    bitchord_commands::playback::get_playback_state(&state)
}

#[tauri::command]
fn toggle_shuffle(state: tauri::State<'_,_ bitchord_state::AppState>) -> Result<(), String> {
    bitchord_commands::playback::toggle_shuffle(&state)
}

#[tauri::command]
fn toggle_repeat(state: tauri::State<'_,_ bitchord_state::AppState>) -> Result<(), String> {
    bitchord_commands::playback::toggle_repeat(&state)
}
