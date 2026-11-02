# Session 5: macOS Platform (Vibrancy)

## Context
Tauri rewrite of BitChord. Need native macOS window effects.

## Task
Implement macOS vibrancy and native window features:

**File 1:** `src-tauri/src/lib.rs`
- Add `set_vibrancy(window: Window, style: Vibrancy)` command
- Use `NSVisualEffectView` via `tauri-plugin` or objc crate
- Support: UIBackingType, Material, BlendMode

**Features to implement:**
- Titlebar vibrancy (fullscreen, medium, ultra-thin materials)
- Dock icon bounce on track change
- ProMotion dragging (120Hz support)
- App activation on click

## Constraints
- NEVER run `cargo build` locally
- ONLY macOS-specific code ( `#[cfg(target_os = "macos")]`)
- Commit after vibrancy works

## Start
Read `PROJECT_BRIEF.md`, then work on macOS-specific code in `src-tauri/src/lib.rs`.