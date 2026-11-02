# Session 10: Settings & Config

## Context
Tauri rewrite of BitChord. Add user settings persistence.

## Task
Implement settings system:

**File:** `rust-backend/crates/config/src/lib.rs`
- `AppConfig` struct with all settings fields
- Settings: theme (light/dark), language, audio quality, cache dir, sync enabled
- `load()` / `save()` using JSON or SQLite
- `default_config()` - sensible defaults

**Frontend:** `src/components/SettingsModal.vue`
- Theme toggle (light/dark/vibrant)
- Language selector
- Audio quality dropdown
- Cache location picker
- Sync options checkbox

## Tauri Commands
- `get_config()` - return AppConfig
- `set_config(Config)` - save new config
- `reset_to_defaults()` - reset

## Constraints
- NEVER run `cargo build` locally
- NEVER modify `desktopApp/` or `app/`
- Commit after settings persist

## Start
Read `PROJECT_BRIEF.md`, then work on `config/src/lib.rs` and SettingsModal.vue.