# Session 1: Audio Engine

## Context
Tauri rewrite of BitChord music player. Previous sessions built Vue shell and Tauri stubs.

## Task
Implement audio engine in `rust-backend/crates/audio/src/lib.rs`:
- rodio-based audio playback with FFmpeg decoding
- `AudioEngine` struct: play/pause/stop/seek/volume/position
- Wire to `commands::playback` module  
- `AudioDecoder` trait for format abstraction (MP3, FLAC, WAV, etc.)
- Use `parking_lot::RwLock` for thread-safe state

## Constraints
- NEVER run `cargo build` locally
- NEVER modify `desktopApp/` or `app/`
- Commit to GitHub after major changes

## Start
Read `PROJECT_BRIEF.md`, then work on `rust-backend/crates/audio/src/lib.rs`.