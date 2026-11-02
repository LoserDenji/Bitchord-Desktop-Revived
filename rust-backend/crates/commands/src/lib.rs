//! Command handlers for BitChord backend
//!
//! This module provides the core logic for commands.
//! Tauri command wrappers are defined in src-tauri/src/lib.rs.

use tracing::info;
use std::path::PathBuf;

use bitchord_state::{AppState, PlaybackStateResponse};
use bitchord_core::{LibraryStats, Track, Album, Artist};
use bitchord_library::LibraryScanner;
use bitchord_sync::SyncMessage;

/// Types for responses
#[derive(Debug, Clone, serde::Serialize)]
pub struct LibraryScanResult {
    pub tracks_found: usize,
    pub bytes_scanned: u64,
}

/// Library commands implementation
pub mod library {
    use super::*;

    pub async fn scan_library(
            _state: &AppState,
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

        info!("Library scan complete: {} tracks found", stats.tracks_found);
        
        Ok(LibraryScanResult {
            tracks_found: stats.tracks_found,
            bytes_scanned: stats.bytes_scanned,
        })
    }

    pub fn get_library_stats(state: &AppState) -> Result<LibraryStats, String> {
        let library = state.library.read();
        if let Some(lib) = library.as_ref() {
            Ok(lib.stats.clone())
        } else {
            Err("Library not loaded".into())
        }
    }

    pub fn list_tracks(_state: &AppState) -> Result<Vec<Track>, String> {
        Ok(Vec::new())
    }

    pub fn list_albums(_state: &AppState) -> Result<Vec<Album>, String> {
        Ok(Vec::new())
    }

    pub fn list_artists(_state: &AppState) -> Result<Vec<Artist>, String> {
        Ok(Vec::new())
    }
}

/// Sync commands implementation (Party Mode)
pub mod sync {
    use super::*;

    pub fn create_sync_session(
        _state: &AppState,
        session_name: String,
        participant_name: String,
    ) -> Result<String, String> {
        info!("Creating sync session: {} with participant: {}", session_name, participant_name);
        Ok(uuid::Uuid::new_v4().to_string())
    }

    pub fn join_sync_session(
        _state: &AppState,
        session_id: String,
        participant_name: String,
    ) -> Result<(), String> {
        info!("Entering sync session: {} as {}", session_id, participant_name);
        Ok(())
    }

    pub fn leave_sync_session(
        _state: &AppState,
        session_id: String,
    ) -> Result<(), String> {
        info!("Leaving sync session: {}", session_id);
        Ok(())
    }

    pub fn broadcast_playback_state(
        _state: &AppState,
        _session_id: String,
        _message: SyncMessage,
    ) -> Result<(), String> {
        Ok(())
    }
}

/// Playback commands implementation
pub mod playback {
    use super::*;

    pub fn play_track(state: &AppState, track_id: uuid::Uuid) -> Result<(), String> {
        info!("Playing track: {}", track_id);
        let mut playback_guard = state.playback.write();
        playback_guard.current_track = Some(track_id);
        *state.is_playing.write() = true;
        Ok(())
    }

    pub fn pause_playback(state: &AppState) -> Result<(), String> {
        info!("Pausing playback");
        *state.is_playing.write() = false;
        Ok(())
    }

    pub fn stop_playback(state: &AppState) -> Result<(), String> {
        info!("Stopping playback");
        *state.is_playing.write() = false;
        let mut playback_guard = state.playback.write();
        playback_guard.current_track = None;
        playback_guard.position_ms = 0;
        Ok(())
    }

    pub fn next_track(_state: &AppState) -> Result<(), String> {
        info!("Skipping to next track");
        Ok(())
    }

    pub fn previous_track(_state: &AppState) -> Result<(), String> {
        info!("Skipping to previous track");
        Ok(())
    }

    pub fn seek_to(state: &AppState, position_ms: u64) -> Result<(), String> {
        info!("Seeking to {}ms", position_ms);
        let mut playback_guard = state.playback.write();
        playback_guard.position_ms = position_ms;
        Ok(())
    }

    pub fn set_volume(state: &AppState, volume: f32) -> Result<(), String> {
        info!("Setting volume to {}", volume);
        if !(0.0..=1.0).contains(&volume) {
            return Err("Volume must be between 0.0 and 1.0".into());
        }
        let mut playback_guard = state.playback.write();
        playback_guard.volume = volume;
        Ok(())
    }

    pub fn get_playback_state(state: &AppState) -> Result<PlaybackStateResponse, String> {
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

    pub fn toggle_shuffle(_state: &AppState) -> Result<(), String> {
        info!("Toggling shuffle");
        Ok(())
    }

    pub fn toggle_repeat(_state: &AppState) -> Result<(), String> {
        info!("Toggling repeat");
        Ok(())
    }
}
