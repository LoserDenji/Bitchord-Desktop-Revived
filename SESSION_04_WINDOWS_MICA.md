# Session 4: Windows Platform (Mica)

## Context
Tauri rewrite of BitChord. Windows window chrome configured.

## Task
Implement Windows Mica/Acrylic effects:

**File 1:** `src-tauri/src/lib.rs`
- Add `set_mica(window: Window, enabled: bool)` command
- Use `window.set_background_color()` with acrylic color
- Optional: Win32 DwmSetWindowAttribute for true Mica

**File 2:** `src-tauri/Cargo.toml`
- Add `tauri-plugin-windows` or Windows API crate
- Feature-gate Windows-specific deps

## Mica Config
- `MicaType::Custom` with `BackdropMaterial`
- Tinted with `Color::from_rgba(255, 255, 255, 15)`
- Dark mode support

## Constraints
- NEVER run `cargo build` locally
- ONLY Windows-specific code
- Commit after Mica works

## Start
Read `PROJECT_BRIEF.md`, then work on `src-tauri/src/lib.rs`.