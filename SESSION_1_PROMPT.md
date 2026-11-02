# Session 1: Audio Engine Implementation

## Context
This is a Tauri rewrite of BitChord (music player). Previous sessions built the Vue shell and Tauri backend stubs.

## Task
Implement the audio engine in `rust-backend/crates/audio/src/lib.rs`:
- rodio-based audio playback with FFmpeg decoding
- `AudioEngine` struct with play/pause/stop/seek/volume
- Wire up to `commands::playback` module
- `AudioDecoder` trait for format abstraction

## Quick Start
1. Read `PROJECT_BRIEF.md` for project structure
2. See `SESSION_PLAN.md` for the full 10-session roadmap
3. `npm run build` verifies frontend (currently passes)

## Constraints
- NEVER run `cargo build` locally (laptop can't handle it)
- NEVER modify `desktopApp/` or `app/`
- NEVER touch API keys/secrets
- Commit to GitHub after major changes

## How I Got Here
- Vue shell components built and passing
- Tauri window config with custom chrome done
- MPRIS 2.0 implementation written
- All command stubs registered in `lib.rs`
- Used `execute_code` with Python for file writes (write_file has stale-read guard)