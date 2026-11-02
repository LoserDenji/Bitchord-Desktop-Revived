# BitChord Tauri Rewrite — Session Plan

## Goal
Implement every feature from the Compose app in the Tauri/Rust rewrite. Must feel like the same app but faster (<400MB RAM vs 1.5GB).

## Current State
- Vue shell components built and passing (`npm run build` exit 0)
- Tauri window config with custom chrome
- MPRIS 2.0 implementation written (Linux D-Bus)
- All command stubs registered in `lib.rs`
- 12 major feature gaps identified

## Session Plan (what to do each session)

### Session 1: Audio Engine (HIGH PRIORITY)
**File:** `rust-backend/crates/audio/src/lib.rs`
- Implement rodio-based audio playback with FFmpeg decoding
- Add `AudioEngine` struct with play/pause/stop/seek/volume
- Wire up to `commands::playback` module
- Add `AudioDecoder` trait for format abstraction

### Session 2: Library & Database (HIGH PRIORITY)
**Files:** `rust-backend/crates/db/src/lib.rs`, `rust-backend/crates/library/src/lib.rs`
- Create SQLite schema (tracks, albums, artists, playlists, play_history)
- Implement library scanning with `walkdir` + tag parsing (`lofty`)
- Implement CRUD operations for all entities
- Wire up `commands::library` module

### Session 3: MPRIS State Wiring (MEDIUM)
**Files:** `rust-backend/crates/platform/src/mpris.rs`, `src-tauri/src/lib.rs`
- Connect MPRIS to live playback state
- Add `update_playback_state` command that syncs MPRIS + frontend
- Register MPRIS on app startup (Linux only)

### Session 4: Windows Platform (MEDIUM)
**Files:** `src-tauri/src/lib.rs`, `src-tauri/Cargo.toml`
- Implement real Mica/Acrylic via `tauri-windows` or raw Win32
- Add `set_mica` command with actual DWM calls
- Wire up Windows-specific tray icon

### Session 5: macOS Platform (MEDIUM)
**Files:** `src-tauri/src/lib.rs`, `src-tauri/Cargo.toml`
- Add macOS vibrancy via Tauri's `NSVisualEffectView`
- Implement Dock icon bounce, ProMotion dragging
- Add `set_vibrancy` command

### Session 6: Sync / Party Mode (MEDIUM)
**Files:** `rust-backend/crates/sync/src/lib.rs`, `rust-backend/crates/network/src/lib.rs`
- Implement WebSocket sync server/client
- Add sync message broadcasting
- Wire up `commands::sync` module

### Session 7: Downloads Manager (LOW)
**Files:** `rust-backend/crates/commands/src/lib.rs`
- Add download queue with progress tracking
- Wire up to frontend via Tauri events

### Session 8: Lyrics (LOW)
**Files:** `rust-backend/crates/network/src/lib.rs`
- Add lyrics fetching from Musixmatch/LRCLib API
- Wire up to frontend

### Session 9: Discord RPC (LOW)
**Files:** New crate or add to `platform/`
- Add Discord Rich Presence integration
- Show current track in Discord status

### Session 10: Settings & Config (MEDIUM)
**Files:** `rust-backend/crates/config/src/lib.rs`
- Implement settings persistence (SQLite or JSON)
- Add settings UI to frontend
- Wire up config commands

## How to Run
1. Read `PROJECT_BRIEF.md` first for full context
2. Pick a session from above
3. Implement the feature, then run `npm run build` to verify frontend
4. Commit to GitHub with descriptive message
5. Move to next session

## Critical Rules
- NEVER modify `desktopApp/` or `app/`
- NEVER touch API keys/secrets
- NEVER run `cargo build` locally (user forbids it — "MY LAPTOP WILL CRASH")
- Use `patch` for targeted edits, `execute_code` with Python for file writes
- Always run `npm run build` after frontend changes
- Commit to GitHub after each major change
- User is monitoring elsewhere — work autonomously, don't stop until at least one major feature is done

## Prompt for Delegation
"Work on the BitChord Tauri rewrite. Read PROJECT_BRIEF.md for context. Pick the highest priority session from the Session Plan in SESSION_PLAN.md and implement it. Use `patch` for edits, `execute_code` with Python for file writes. Run `npm run build` to verify. Commit to GitHub when done. Never modify desktopApp/ or app/, never touch secrets, never run cargo build locally."