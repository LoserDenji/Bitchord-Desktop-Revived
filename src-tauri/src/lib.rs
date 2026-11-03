//! BitChord Tauri Application
//!
//! Main entry point for the Tauri desktop application.
//! Wires up all command handlers and initializes the app state.

use tauri::Manager;
use std::sync::Arc;

#[cfg(target_os = "macos")]
use objc;

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

// Window control commands
#[tauri::command]
fn minimize_window(window: tauri::Window) -> Result<(), String> {
    window.minimize()
}

#[tauri::command]
fn toggle_maximize(window: tauri::Window) -> Result<(), String> {
    if window.is_maximized() {
        window.unmaximize()
    } else {
        window.maximize()
    }
}

#[tauri::command]
fn close_window(window: tauri::Window) -> Result<(), String> {
    window.close()
}

// Library commands
#[tauri::command]
async fn scan_library(state: tauri::State<'_, bitchord_state::AppState>, path: String) -> Result<bitchord_commands::LibraryScanResult, String> {
    bitchord_commands::library::scan_library(&state, path).await
}

#[tauri::command]
fn get_library_stats(state: tauri::State<'_, bitchord_state::AppState>) -> Result<bitchord_core::LibraryStats, String> {
    bitchord_commands::library::get_library_stats(&state)
}

#[tauri::command]
fn list_tracks(state: tauri::State<'_, bitchord_state::AppState>) -> Result<Vec<bitchord_core::Track>, String> {
    bitchord_commands::library::list_tracks(&state)
}

#[tauri::command]
fn list_albums(state: tauri::State<'_, bitchord_state::AppState>) -> Result<Vec<bitchord_core::Album>, String> {
    bitchord_commands::library::list_albums(&state)
}

#[tauri::command]
fn list_artists(state: tauri::State<'_, bitchord_state::AppState>) -> Result<Vec<bitchord_core::Artist>, String> {
    bitchord_commands::library::list_artists(&state)
}

// Sync commands
#[tauri::command]
fn create_sync_session(state: tauri::State<'_, bitchord_state::AppState>, session_name: String, participant_name: String) -> Result<String, String> {
    bitchord_commands::sync::create_sync_session(&state, session_name, participant_name)
}

#[tauri::command]
fn join_sync_session(state: tauri::State<'_, bitchord_state::AppState>, session_id: String, participant_name: String) -> Result<(), String> {
    bitchord_commands::sync::join_sync_session(&state, session_id, participant_name)
}

#[tauri::command]
fn leave_sync_session(state: tauri::State<'_, bitchord_state::AppState>, session_id: String) -> Result<(), String> {
    bitchord_commands::sync::leave_sync_session(&state, session_id)
}

#[tauri::command]
fn broadcast_playback_state(state: tauri::State<'_, bitchord_state::AppState>, session_id: String, message: bitchord_sync::SyncMessage) -> Result<(), String> {
    bitchord_commands::sync::broadcast_playback_state(&state, session_id, message)
}

// Playback commands
#[tauri::command]
fn play_track(
    state: tauri::State<'_, bitchord_state::AppState>,
    #[cfg(target_os = "linux")] mpris: tauri::State<'_, bitchord_platform::MprisManager>,
    track_id: uuid::Uuid,
) -> Result<(), String> {
    bitchord_commands::playback::play_track(&state, track_id)?;

    #[cfg(target_os = "linux")]
    {
        let lib = state.library.read();
        let track = lib.as_ref().and_then(|l| l.find_track(track_id));
        mpris.update_playback_state(true, 0u64, track);
    }

    Ok(())
}

#[tauri::command]
fn pause_playback(
    state: tauri::State<'_, bitchord_state::AppState>,
    #[cfg(target_os = "linux")] mpris: tauri::State<'_, bitchord_platform::MprisManager>,
) -> Result<(), String> {
    bitchord_commands::playback::pause_playback(&state)?;

    #[cfg(target_os = "linux")]
    {
        let playback = state.playback.read();
        let position = playback.position_ms;
        let track_id = playback.current_track;
        drop(playback);
        let lib = state.library.read();
        let track = track_id.and_then(|id| lib.as_ref().and_then(|l| l.find_track(id)));
        mpris.update_playback_state(false, position, track);
    }

    Ok(())
}

#[tauri::command]
fn stop_playback(
    state: tauri::State<'_, bitchord_state::AppState>,
    #[cfg(target_os = "linux")] mpris: tauri::State<'_, bitchord_platform::MprisManager>,
) -> Result<(), String> {
    bitchord_commands::playback::stop_playback(&state)?;

    #[cfg(target_os = "linux")]
    {
        mpris.update_playback_state(false, 0u64, None);
    }

    Ok(())
}

#[tauri::command]
fn next_track(state: tauri::State<'_, bitchord_state::AppState>) -> Result<(), String> {
    bitchord_commands::playback::next_track(&state)
}

#[tauri::command]
fn previous_track(state: tauri::State<'_, bitchord_state::AppState>) -> Result<(), String> {
    bitchord_commands::playback::previous_track(&state)
}

#[tauri::command]
fn seek_to(
    state: tauri::State<'_, bitchord_state::AppState>,
    #[cfg(target_os = "linux")] mpris: tauri::State<'_, bitchord_platform::MprisManager>,
    position_ms: u64,
) -> Result<(), String> {
    bitchord_commands::playback::seek_to(&state, position_ms)?;

    #[cfg(target_os = "linux")]
    {
        let track_id = state.playback.read().current_track;
        drop(state.playback.read());
        let lib = state.library.read();
        let track = track_id.and_then(|id| lib.as_ref().and_then(|l| l.find_track(id)));
        mpris.update_playback_state(*state.is_playing.read(), position_ms, track);
    }

    Ok(())
}

#[tauri::command]
fn set_volume(state: tauri::State<'_, bitchord_state::AppState>, volume: f32) -> Result<(), String> {
    bitchord_commands::playback::set_volume(&state, volume)
}

#[tauri::command]
fn get_playback_state(state: tauri::State<'_, bitchord_state::AppState>) -> Result<bitchord_state::PlaybackStateResponse, String> {
    bitchord_commands::playback::get_playback_state(&state)
}

#[tauri::command]
fn toggle_shuffle(state: tauri::State<'_, bitchord_state::AppState>) -> Result<(), String> {
    bitchord_commands::playback::toggle_shuffle(&state)
}

#[tauri::command]
fn toggle_repeat(state: tauri::State<'_, bitchord_state::AppState>) -> Result<(), String> {
    bitchord_commands::playback::toggle_repeat(&state)
}

// Platform-specific commands

#[cfg(target_os = "linux")]
#[tauri::command]
fn mpris_update(
    state: tauri::State<'_, bitchord_state::AppState>,
) -> Result<(), String> {
    let playback = state.playback.read();
    let position = playback.position_ms;
    let track_id = playback.current_track;
    drop(playback);
    let lib = state.library.read();
    let track = track_id.and_then(|id| lib.as_ref().and_then(|l| l.find_track(id)));
    let is_playing = *state.is_playing.read();
    if let Some(mpris) = state.mpris.as_ref() {
        mpris.update_playback_state(is_playing, position, track);
    }
    Ok(())
}

#[cfg(target_os = "windows")]
#[tauri::command]
fn set_mica(window: tauri::Window, enable: bool) -> Result<(), String> {
    if enable {
        // Enable Mica effect with tinted backdrop
        let _ = window.set_background_color(tauri::Color(255, 255, 255, 15));
        
        // Set DwmSetWindowAttribute for true Mica effect
        unsafe {
            use std::ffi::c_void;
            use windows::Win32::Foundation::HANDLE;
            use windows::Win32::UI::WindowsAndMessaging::{
                DwmSetWindowAttribute, DWMWA_MICA_EFFECT, DWMWA_WINDOW_CORNER_PREFERRED,
                DWMWN_WINDOW_CORNER_PREFERENCE_ROUND,
            };
            
            let handle = window.hwnd();
            if handle.is_null() {
                return Ok(());
            }
            
            // Set corner preference to rounded
            let corner_pref: u32 = DWMWN_WINDOW_CORNER_PREFERENCE_ROUND;
            let _ = DwmSetWindowAttribute(
                handle as HANDLE,
                DWMWA_WINDOW_CORNER_PREFERRED,
                &corner_pref as *const u32 as *const c_void,
                std::mem::size_of::<u32>() as u32,
            );
            
            // Enable Mica effect
            let mica_enabled: u32 = 1;
            let _ = DwmSetWindowAttribute(
                handle as HANDLE,
                DWMWA_MICA_EFFECT,
                &mica_enabled as *const u32 as *const c_void,
                std::mem::size_of::<u32>() as u32,
            );
        }
    } else {
        let _ = window.set_background_color(tauri::Color(255, 255, 255, 255));
    }
    Ok(())
}

// macOS-specific commands

#[cfg(target_os = "macos")]
#[tauri::command]
fn set_vibrancy(window: tauri::Window, style: &str) -> Result<(), String> {
    use tauri::Vibrancy;
    
    let vibrancy = match style.to_lowercase().as_str() {
        "fullscreen" => Vibrancy::Fullscreen,
        "medium" => Vibrancy::Medium,
        "ultrathin" => Vibrancy::UltraThin,
        "regular" => Vibrancy::Regular,
        "sidebar" => Vibrancy::Sidebar,
        _ => Vibrancy::None,
    };
    
    window.set_vibrancy(vibrancy).map_err(|e| e.to_string())
}

#[cfg(target_os = "macos")]
#[tauri::command]
fn dock_bounce(_window: tauri::Window) -> Result<(), String> {
    Ok(())
}

#[cfg(target_os = "macos")]
#[tauri::command]
fn activate_app() -> Result<(), String> {
    unsafe {
        let _ = objc::msg_send![objc::class!(NSApplication), sharedApplication];
    }
    
    Ok(())
}

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let mut state = bitchord_state::AppState::new();
            
            // Initialize sync manager
            let sync_manager = Arc::new(bitchord_sync::SyncManager::new());
            *state.sync.write() = Some(sync_manager);
            
            app.manage(state);

            #[cfg(target_os = "linux")]
            {
                let mpris = bitchord_platform::MprisManager::new();
                app.manage(mpris);
            }

            #[cfg(debug_assertions)]
            {
                let _ = app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                );
            }

            let win = app.get_webview_window("main").expect("main window");
            let _ = win.set_title("BitChord");
            let _ = win.set_decorations(false);
            let _ = win.set_skip_taskbar(false);

            #[cfg(target_os = "macos")]
            {
                let _ = app.handle().activate();
            }

            let _ = app.tray_icon();

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_platform,
            minimize_window,
            toggle_maximize,
            close_window,
            scan_library,
            get_library_stats,
            list_tracks,
            list_albums,
            list_artists,
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
            #[cfg(target_os = "linux")]
            mpris_update,
            #[cfg(target_os = "windows")]
            set_mica,
            #[cfg(target_os = "macos")]
            set_vibrancy,
            #[cfg(target_os = "macos")]
            dock_bounce,
            #[cfg(target_os = "macos")]
                        activate_app,
                        create_sync_session,
                        join_sync_session,
                        leave_sync_session,
                        broadcast_playback_state,
                    ])
        .run(tauri::generate_context!())
        .expect("error while building tauri application");
}
