//! Tauri command handlers
//!
//! This module provides command handlers for the BitChord frontend to backend communication.
//! Commands are organized into logical groups: Library, Sync, and Playback.

use tauri::State;
use tracing::info;
use std::path::PathBuf;

use bitchord_state::AppState;
use bitchord_core::{Library, LibraryStats, Track, Album, Artist};
use bitchord_library::LibraryScanner;
use bitchord_sync::{PartySync, Participant, SyncMessage};

/// Module containing all command handler groups
pub mod handlers {
    use super::*;

    // ========================================================================
    // Library Commands
    // ========================================================================

    #[tauri::command]
    pub async fn scan_library(
        state: State<'_, AppState>,
        path: String,
    ) -> Result<LibraryScanResult, String> {
        let scan_path = PathBuf::from(path);
        if !scan_path.exists() {
            return Err("Path does not exist".into());
        }

        info!("Scanning library at: {:?}", scan_path);

        let mut scanner = LibraryScanner::with_default_config();
        let stats = futures::executor::block_on(scanner.scan(&scan_path))
            .map_err(|e| e.to_string())?;

        let mut library_guard = state.library.write();
        *library_guard = Some(Library::new(scan_path, "Default".into()));
        
        info!("Library scan complete: {} tracks found", stats.tracks_found);
        
        Ok(LibraryScanResult {
            tracks_found: stats.tracks_found,
            bytes_scanned: stats.bytes_scanned,
        })
    }

    #[tauri::command]
    pub async fn get_library_stats(state: State<'_, AppState>) -> Result<LibraryStats, String> {
        let library = state.library.read();
        if let Some(lib) = library.as_ref() {
            Ok(lib.stats.clone())
        } else {
            Err("Library not loaded".into())
        }
    }

    #[tauri::command]
    pub async fn list_tracks(state: State<'_, AppState>) -> Result<Vec<Track>, String> {
        Ok(Vec::new())
    }

    #[tauri::command]
    pub async fn list_albums(state: State<'_, AppState>) -> Result<Vec<Album>, String> {
        Ok(Vec::new())
    }

    #[tauri::command]
    pub async fn list_artists(state: State<'_, AppState>) -> Result<Vec<Artist>, String> {
        Ok(Vec::new())
    }

    // ========================================================================
    // Sync Commands (Party Mode)
    // ========================================================================

    #[tauri::command]
    pub async fn create_sync_session(
        _state: State<'_, AppState>,
        session_name: String,
        participant_name: String,
    ) -> Result<String, String> {
        info!("Creating sync session: {} with participant: {}", session_name, participant_name);
        Ok(uuid::Uuid::new_v4().to_string())
    }

    #[tauri::command]
    pub async fn join_sync_session(
        _state: State<'_, AppState>,
        session_id: String,
        participant_name: String,
    ) -> Result<(), String> {
        info!("Entering sync session: {} as {}", session_id, participant_name);
        Ok(())
    }

    #[tauri::command]
    pub async fn leave_sync_session(
        _state: State<'_, AppState>,
        session_id: String,
    ) -> Result<(), String> {
        info!("Leaving sync session: {}", session_id);
        Ok(())
    }

    #[tauri::command]
    pub async fn broadcast_playback_state(
        _state: State<'_, AppState>,
        _session_id: String,
        _message: SyncMessage,
    ) -> Result<(), String> {
        Ok(())
    }

    // ========================================================================
    // Playback Commands
    // ========================================================================

    #[tauri::command]
    pub async fn play_track(
        state: State<'_, AppState>,
        track_id: uuid::Uuid,
    ) -> Result<(), String> {
        info!("Playing track: {}", track_id);
        let mut playback_guard = state.playback.write();
        playback_guard.current_track = Some(track_id);
        *state.is_playing.write() = true;
        Ok(())
    }

    #[tauri::command]
    pub async fn pause_playback(state: State<'_, AppState>) -> Result<(), String> {
        info!("Pausing playback");
        *state.is_playing.write() = false;
        Ok(())
    }

    #[tauri::command]
    pub async fn stop_playback(state: State<'_, AppState>) -> Result<(), String> {
        info!("Stopping playback");
        *state.is_playing.write() = false;
        let mut playback_guard = state.playback.write();
        playback_guard.current_track = None;
        playback_guard.position_ms = 0;
        Ok(())
    }

    #[tauri::command]
    pub async fn next_track(state: State<'_, AppState>) -> Result<(), String> {
        info!("Skipping to next track");
        Ok(())
    }

    #[tauri::command]
    pub async fn previous_track(state: State<'_, AppState>) -> Result<(), String> {
        info!("Skipping to previous track");
        Ok(())
    }

    #[tauri::command]
    pub async fn seek_to(
        state: State<'_, AppState>,
        position_ms: u64,
    ) -> Result<(), String> {
        info!("Seeking to {}ms", position_ms);
        let mut playback_guard = state.playback.write();
        playback_guard.position_ms = position_ms;
        Ok(())
    }

    #[tauri::command]
    pub async fn set_volume(state: State<'_, AppState>, volume: f32) -> Result<(), String> {
        info!("Setting volume to {}", volume);
        if !(0.0..=1.0).contains(&volume) {
            return Err("Volume must be between 0.0 and 1.0".into());
        }
        let mut playback_guard = state.playback.write();
        playback_guard.volume = volume;
        Ok(())
    }

    #[tauri::command]
    pub async fn get_playback_state(state: State<'_, AppState>) -> Result<PlaybackStateResponse, String> {
        let playback = state.playback.read().clone();
        let is_playing = *state.is_playing.read();
        
        Ok(PlaybackStateResponse {
            current_track: playback.current_track,
            position_ms: playback.position_ms,
            is_playing,
            volume: playback.volume,
            speed: playback.speed,
        })
    }

    #[tauri::command]
    pub async fn toggle_shuffle(state: State<'_, AppState>) -> Result<(), String> {
        info!("Toggling shuffle");
        Ok(())
    }

    #[tauri::command]
    pub async fn toggle_repeat(state: State<'_, AppState>) -> Result<(), String> {
        info!("Toggling repeat");
        Ok(())
    }
}

// Types for responses
#[derive(Debug, Clone, serde::Serialize)]
pub struct LibraryScanResult {
    pub tracks_found: usize,
    pub bytes_scanned: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PlaybackStateResponse {
    pub current_track: Option<uuid::Uuid>,
    pub position_ms: u64,
    pub is_playing: bool,
    pub volume: f32,
    pub speed: f32,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Album {
    pub id: uuid::Uuid,
    pub name: String,
    pub artist: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Artist {
    pub id: uuid::Uuid,
    pub name: String,
}