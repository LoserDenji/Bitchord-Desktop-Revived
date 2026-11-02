# BitChord Desktop — Agent Guide

## Project overview

This is a Tauri (Rust + Vue/JS) rewrite of BitChord, NOT the original Kotlin/Compose app.

**OLD (reference only):** Kotlin Multiplatform / Compose Desktop player for Windows, macOS, Linux. Built from Android client's shared modules.

**NEW (current work):** Tauri rewrite targeting <400MB RAM vs 1.5GB original.

## Repository Structure

- `src/` — Vue 3 + TypeScript frontend
- `src-tauri/` — Tauri Rust backend  
- `rust-backend/` — Shared Rust workspace (commands, audio, db, etc.)
- `desktopApp/` — ORIGINAL Compose app (DO NOT MODIFY)
- `PROJECT_BRIEF.md` — Full project reference
- `SESSION_PLAN.md` — What to implement each session
- `DELEGATION_PROMPT.md` — Prompt for delegating to another agent

## Repository

- Remote: `https://github.com/LoserDenji/Bitchord-Desktop-Revived`
- Original Android project: `https://github.com/kushagrasinghx/BitChord`

## CI

- `.github/workflows/release.yml` — builds all three platforms on tag push.
  macOS uses `macos-15` (arm64) and `macos-15-intel` (x64). `macos-14` is retired.
- `.github/workflows/check.yml` — runs tests on every push.
- Android (`android.yml`) and backend (`deploy-backend.yml`) workflows are **removed**
  — desktop-only.

## Releasing/Compiling (DO NOT COMPILE/BUILD ON LOCAL MACHINE SEND TO GITHUB TO DO IT)

```bash
git tag v*.*.*
git push origin v*.*.*
```

The release workflow builds packages, runs tests, and creates a **draft** release.
Review and publish it from the GitHub Releases page.

## Platform notes

- **macOS**: `DesktopMacFrame.kt` provides native window chrome, vibrancy backdrop
  (NSVisualEffectView via JNA), ProMotion dragging, and Dock activation. It is guarded
  by `DesktopPlatform.isMac` everywhere — no macOS code runs on other platforms.
- **Windows**: `DesktopWindowsFrame.kt` handles DWM Mica/Acrylic. `DesktopSmtc.kt`
  is WinRT-based and only builds on Windows.
- **Linux**: MPRIS via `DesktopMpris.kt` (DBus). No native window frame — the system
  draws it.

## Known flaky tests

- `DesktopBeatModelTest` — skips itself on macOS headless CI (AWT/ORT conflict).
- `DesktopDownloadQueueTest.cancelAll` — polls for StateFlow to settle (up to 500 ms).

## Conventions

- Internal desktop code lives in `desktopApp/src/main/kotlin/com/music/bitchord/desktop/`.
- `DesktopPlatform` is the single source of truth for `isWindows` / `isLinux` / `isMac`.
- `isMac` guards must be present on every macOS-specific code path; grep for them
  before claiming a feature works on macOS.
- Don't touch `app/`, the Go backend, or shared data-layer Android targets.

## Updating this guide

When something in the project changes that affects how work is done here, update
this file. Things that should trigger an update:

- **New platform support or removal** — update the platform notes.
- **CI changes** — new/removed workflows, runner changes, tag conventions.
- **Build command changes** — new Gradle tasks, changed flags.
- **New flaky test or fix for one** — add to the known-flaky-tests section.
- **Toolchain changes** — JDK/Gradle/CMake version bumps.
- **Repository or remote changes** — update the repository section.
- **Conventions added or removed** — update the conventions section.

Keep it short and factual. One line per fact. No prose, no "how to" instructions
for things that are obvious from the code.
