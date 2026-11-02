# Session 7: Downloads Manager

## Context
Tauri rewrite of BitChord. Add download functionality.

## Task
Implement download queue with progress tracking:

**File:** `rust-backend/crates/commands/src/lib.rs`
- `download_track(track_id: Uuid, dest: PathBuf)` - start download
- `download_album(album_id: Uuid, dest: PathBuf)` - download all tracks
- `get_download_status(job_id: Uuid)` - return progress

## Implementation
- `DownloadQueue` with channel sender/receiver
- Progress: bytes_read / total_size / percent
- Tauri events: `download-progress`, `download-complete`
- Cancel/pause/resume support
- Concurrent downloads (limit 3)

## Storage
- Temp directory for active downloads
- User-chosen directory for completed
- Resume from where left off

## Constraints
- NEVER run `cargo build` locally
- NEVER modify `desktopApp/` or `app/`
- Commit after queue works

## Start
Read `PROJECT_BRIEF.md`, then work on download queue in `commands/src/lib.rs`.