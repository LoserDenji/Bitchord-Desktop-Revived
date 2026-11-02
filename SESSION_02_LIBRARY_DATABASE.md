# Session 2: Library & Database

## Context
Tauri rewrite of BitChord music player. Audio engine completed.

## Task
Implement library scanning and database persistence:

**File 1:** `rust-backend/crates/db/src/lib.rs`
- Create SQLite schema (tracks, albums, artists, playlists, play_history)
- Fields: id, path, title, artist, album, duration, genre, track_number
- Add search indexes on artist, album, title

**File 2:** `rust-backend/crates/library/src/lib.rs`
- Implement library scanning with `walkdir` + `lofty` for tag parsing
- `scan_library(path: &Path)` - recursive scan, extract metadata
- `search(query: &str, limit: usize)` - full-text search
- CRUD: `get_track`, `create_track`, `update_track`, `delete_track`

## Constraints
- NEVER run `cargo build` locally
- NEVER modify `desktopApp/` or `app/`
- Commit after schema and scanning work

## Start
Read `PROJECT_BRIEF.md`, then work on `rust-backend/crates/db/src/lib.rs`.