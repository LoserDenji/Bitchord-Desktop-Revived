# BitChord Desktop — Agent Guide

## Project overview

BitChord for PC is a Kotlin Multiplatform / Compose Desktop player for Windows, macOS
and Linux, styled after Apple Music. It is built from the Android client's shared
modules (`shared`, `sharedUi`) plus a `desktopApp` that adds the desktop shell.

The Android app (`app/`) is **not developed here** — this is desktop-only. Do not
modify `app/`, the Go backend, or the shared data layer's Android targets.

## Repository

- Remote: `https://github.com/LoserDenji/Bitchord-Desktop-Revived`
- Original Android project: `https://github.com/kushagrasinghx/BitChord`

## CI

- `.github/workflows/release.yml` — builds all three platforms on tag push.
  macOS uses `macos-15` (arm64) and `macos-15-intel` (x64). `macos-14` is retired.
- `.github/workflows/check.yml` — runs tests on every push.
- Android (`android.yml`) and backend (`deploy-backend.yml`) workflows are **removed**
  — desktop-only.

## Releasing

```bash
git tag v1.8.35
git push origin v1.8.35
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
