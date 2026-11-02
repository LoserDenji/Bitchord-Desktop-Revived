# Session 8: Lyrics Fetcher

## Context
Tauri rewrite of BitChord. Add lyrics display.

## Task
Implement lyrics fetching from multiple services:

**File:** `rust-backend/crates/network/src/lib.rs`
- `fetch_lyrics(title: &str, artist: &str)` - main entry point
- `fetch_lrcLib(title, artist)` - LRCLib API
- `fetch_musixmatch(title, artist)` - Musixmatch API
- `fetch genius(title, artist)` - Genius API fallback

## Response Type
```rust
pub struct Lyrics {
    pub provider: String,
    pub language: String,
    pub lyrics: String,
    pub sync_type: SyncType, // Unsynced, Synced
    pub lines: Vec<LyricsLine>, // if synced
}
```

## Constraints
- NEVER run `cargo build` locally
- NEVER modify `desktopApp/` or `app/`
- NEVER hardcode API keys
- Commit after lyrics fetch works

## Start
Read `PROJECT_BRIEF.md`, then work on `network/src/lib.rs`.