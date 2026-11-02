# Session 3: MPRIS State Wiring

## Context
Tauri rewrite of BitChord. MPRIS implementation exists but is stubbed.

## Task
Connect MPRIS to live playback state:

**File 1:** `rust-backend/crates/platform/src/mpris.rs`
- Add `SharedMprisState` struct with Arc<RwLock<PlaybackState>>
- Update `MprisManager` to hold state reference
- Implement `update_playback_state(playing, position, track)`

**File 2:** `src-tauri/src/lib.rs`
- Add `update_mpris_state` command handler
- Register MPRIS manager in Tauri state
- Call update on play/pause/stop/track change

## Key Interfaces Needed
- `playback_status`: "Playing" | "Paused" | "Stopped"
- `metadata`: xesam:title, xesam:artist, mpris:album, mpris:artworkUrl
- `position`: microseconds
- `volume`: 0.0-1.0

## Constraints
- NEVER run `cargo build` locally
- NEVER modify `desktopApp/` or `app/`
- Commit after MPRIS shows live state

## Start
Read `PROJECT_BRIEF.md` and `SESSION_PLAN.md`, then work.