# Session 9: Discord Rich Presence

## Context
Tauri rewrite of BitChord. Add Discord integration.

## Task
Implement Discord Rich Presence:

**File:** `rust-backend/crates/platform/src/discord.rs` (or new file)
- `DiscordClient` struct wrapping `discord_rpc` crate
- `initialize(app_id: &str)` - setup connection
- `update_presence(track: Option<&Track>)` - update status
- `set_playing(track)` - show now playing
- `clear_presence()` - when stopped

## Rich Presence Details
- Large image: album art or app logo
- Small image: play/pause indicator
- State: "Playing" or "Paused"
- Details: "Artist - Title"
- Start timestamp for elapsed time

## Constraints
- NEVER run `cargo build` locally
- ONLY when Discord is installed
- NEVER modify `desktopApp/` or `app/`
- Commit after basic RPC works

## Start
Read `PROJECT_BRIEF.md`, then work on discord integration.