# AGENTS.md

## Overview
BitChord Desktop Revived (`bitchord`) is a high-performance native desktop music client built in Rust using `egui` / `eframe` (with optional `wgpu` rendering and embedded `wry` WebViews). It connects to YouTube Music, JioSaavn, Apple Music, Spotify transfers, and custom listening-together / jam sync services.

---

## Directory & Architecture Map

### Core Rust Application (`src/`)
- `src/main.rs`: Entry point, application initialization, event loop setup, and background worker threads.
- `src/appicon.rs`, `src/tray.rs`, `src/media.rs`, `src/deeplink.rs`: Operating system integrations (system tray, OS media controls via `souvlaki`, deep link URI handlers, app icons).
- `src/audio.rs`, `src/stream.rs`, `src/dash.rs`, `src/opus.rs`, `src/hires.rs`: Audio pipeline, streaming decoders (`rodio`, `symphonia`, `filmcraft-ac3` for Dolby Atmos), DASH manifest parsing, and high-resolution playback engines.
- `src/innertube.rs`, `src/ytm.rs`, `src/ytmapi.rs`, `src/providers.rs`, `src/apple.rs`, `src/lastfm.rs`, `src/sponsorblock.rs`: Music provider integrations (YouTube Music InnerTube API, JioSaavn DES encryption, Apple Music, Last.fm scrobbling, SponsorBlock API).
- `src/lyrics.rs`, `src/translate.rs`, `src/bidi.rs`: Lyrics parsing (LRC/TTML), line-by-line translation, and Bidirectional (BiDi) text layout engine.
- `src/jam.rs`: Listen Together / Jam synchronization client (WebSocket-based party playback).
- `src/vault.rs`, `src/login.rs`, `src/turnstile.rs`, `src/jsc.rs`: Authentication, OS credential storage (`keyring`), YouTube PoToken solving, embedded QuickJS JS engine (`rquickjs`).
- `src/ui/`: UI components rendered with `egui`:
  - `src/ui/mod.rs`: Main UI coordinator and state context.
  - `src/ui/shell.rs`, `src/ui/titlebar.rs`, `src/ui/island.rs`: Window frame, custom titlebar, dynamic island overlay.
  - `src/ui/nowplaying.rs`, `src/ui/playback.rs`, `src/ui/queue.rs`, `src/ui/karaoke.rs`: Player UI, lyrics overlay, and playback controls.
  - `src/ui/pages.rs`, `src/ui/collections.rs`, `src/ui/account.rs`, `src/ui/integrations.rs`: Application screens and settings views.
- `src/transfer/`: Playlist and music library transfer utilities (Spotify import/export).

### Jam Sync Server (`server/`)
- `server/src/lib.rs`: Cloudflare Workers / Rust WebSocket relay server for Listen Together session state.
- `server/smoke/`: Integration/smoke test runner for the sync server.

### Packaging & OS Distribution
- `linux/`: Linux desktop integration scripts (`bitchord.desktop`, `package.sh`, `install.sh`).
- `macos/`: macOS bundle configuration (`Info.plist`, `bundle.sh`).
- `installer/`: Inno Setup installer script (`bitchord.iss`) for Windows.
- `assets/`: Static fonts, SVG glyphs, theme presets, quality badges, and JS solver scripts.

---

## Strict Build & Development Rules

### 1. No Local Builds Allowed
- **CRITICAL**: Do **NOT** run `cargo build`, `cargo run`, `npm run build`, or any heavy local build commands on this machine.
- All compilation, testing, and release verification must be delegated to GitHub Actions workflows (`.github/workflows/check.yml` and `.github/workflows/release.yml`).

### 2. Triggering Verification & Release Builds
- To verify changes or produce a release, commit your changes, create a git tag, and push to GitHub:
  ```bash
  git tag v2.x.x
  git push origin v2.x.x
  ```
- Make sure `gh` CLI or git credentials are authenticated before pushing.

### 3. File Editing Guidelines
- **Large Files (>7KB)**: Due to editor tool constraints on large files, use terminal heredoc to rewrite large UI or source files rather than single-line or small multi-line patch replacements if a patch fails:
  ```bash
  cat > src/ui/some_file.rs << 'HEREDOC'
  // file content
  HEREDOC
  ```
- **Scope**: Touch only the relevant code required for the task. Do not make unrelated refactors or reformatting edits.

### 4. Code Standards & Dependencies
- Core language: Rust (2024 edition).
- GUI framework: `egui` 0.37 / `eframe` 0.37.
- Async runtime: `tokio` (multi-thread runtime).
- Always verify crate additions in `Cargo.toml` before adding imports. Match existing dependencies and feature flags carefully.
