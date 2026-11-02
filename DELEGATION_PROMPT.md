# BitChord Tauri Rewrite — Delegation Prompt

## Prompt for Claude Code / Next Agent

```
You are now taking over the BitChord Tauri rewrite from the current assistant.

PROJECT CONTEXT:
- Location: C:\Users\singh\BitChord-PC-Continued
- Goal: Rebuild Kotlin Compose desktop app (1.5GB RAM) as Tauri (Rust + Vue/JS) with <400MB RAM
- Branch: main
- User constraint: "NO! DON'T BUILD LOCALLY MY LAPTOP WILL CRASH" - never run cargo build

PRIORITY ORDER:
1. SESSION_PLAN.md defines 10 sessions. Start with Session 1: Audio Engine
2. PROJECT_BRIEF.md has full repo structure and conventions

READ FIRST:
- PROJECT_BRIEF.md - full project reference
- SESSION_PLAN.md - what to implement each session

WORKFLOW:
1. Read the target file before editing (stale-read guard on write_file)
2. Use `patch` for targeted edits
3. Use `execute_code` with Python for file writes
4. Run `npm run build` after frontend changes
5. Commit to GitHub after major changes
6. NEVER modify desktopApp/ or app/
7. NEVER touch API keys/secrets

SESSION 1 (START HERE): Audio Engine
- File: rust-backend/crates/audio/src/lib.rs
- Implement rodio-based playback with FFmpeg/ondio decoding
- Add AudioEngine, AudioDecoder, format support
- Wire to commands::playback

START WORKING NOW. Do not stop until the audio crate has a working implementation that compiles (check with cargo check or at least has the structure in place).
```

## Quick Reference: File Locations

| Feature | Rust Crate | Key Files |
|---------|------------|-----------|
| Audio | `audio/` | `src/lib.rs` |
| Library | `library/` | `src/lib.rs` |
| DB | `db/` | `src/lib.rs` |
| Commands | `commands/` | `src/lib.rs` |
| Platform | `platform/` | `src/mpris.rs`, `src/linux.rs`, `src/windows.rs` |
| Network | `network/` | `src/lib.rs` |
| Sync | `sync/` | `src/lib.rs` |
| Config | `config/` | `src/lib.rs` |

| Component | Frontend | Key Files |
|-----------|----------|-----------|
| Shell | `src/` | `App.vue`, `main.ts` |
| Color tokens | `src/components/` | `_colors.css` |
| Sidebar | `src/components/` | `BitChordSidebar.vue` |
| TopBar | `src/components/` | `BitChordTopBar.vue` |
| PlayerBar | `src/components/` | `BitChordPlayerBar.vue` |
| NavPills | `src/components/` | `BitChordNavPills.vue` |

| Tauri Backend | Path |
|---------------|------|
| Commands | `src-tauri/src/lib.rs` |
| Config | `src-tauri/tauri.conf.json` |
| Cargo.toml | `src-tauri/Cargo.toml` |