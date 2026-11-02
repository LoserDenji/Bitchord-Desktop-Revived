# BitChord Tauri Rewrite — Project Brief

## What This Is
BitChord for PC is a music player being rebuilt from Kotlin Compose (1.5GB RAM) to Tauri (Rust + Vue/JS) targeting <400MB RAM.

## Repository Layout
```
BitChord-PC-Continued/
├── src/                     # Vue 3 + TypeScript frontend
│   ├── App.vue              # Shell layout (sidebar, content, player bar)
│   ├── main.ts              # Entry point, imports _colors.css
│   └── components/          # Vue components
│       ├── _colors.css      # CSS custom properties (ported from Compose)
│       ├── BitChordSidebar.vue  # 260px sidebar with rail collapse
│       ├── BitChordTopBar.vue   # Traffic light controls, search
│       ├── BitChordPlayerBar.vue # 56px bottom player bar
│       └── BitChordNavPills.vue  # Fixed bottom nav for narrow windows
├── src-tauri/               # Tauri Rust backend
│   ├── src/
│   │   ├── lib.rs           # All Tauri command handlers
│   │   └── main.rs          # Entry point
│   ├── Cargo.toml           # Tauri dependencies
│   └── tauri.conf.json      # Window config (1280x820, custom chrome, Mica)
├── rust-backend/            # Shared Rust crates (workspace)
│   ├── Cargo.toml           # Workspace manifest
│   └── crates/
│       ├── core/             # Domain models (Track, Album, Artist, etc.)
│       ├── state/            # PlaybackStateResponse, AppState
│       ├── audio/            # Audio playback (stubbed — needs rodio/ffmpeg)
│       ├── commands/         # Command implementations (stubbed)
│       ├── platform/         # MPRIS, Mica, platform-specific features
│       ├── library/          # Library scanning & management
│       ├── db/               # SQLite persistence
│       ├── network/          # HTTP client, WebSocket sync
│       ├── sync/             # Party/sync mode
│       └── config/           # Settings persistence
└── desktopApp/              # ORIGINAL Kotlin Compose app (reference only — DO NOT MODIFY)
```

## Build Commands
```bash
# Frontend (Vue/JS)
npm run build     # Produces dist/ (74KB JS, 6KB CSS)

# Rust backend (requires Rust toolchain)
cd rust-backend && cargo check

# Tauri build (full app)
cd src-tauri && cargo build
# OR
npx tauri build
```

## Key Conventions
- `DesktopPlatform.isMac` / `isWindows` / `isLinux` — platform guards everywhere
- Custom window chrome via Tauri config: `hiddenTitle: true`, `titleBarStyle: "custom"`
- Drag regions: `-webkit-app-region: drag` on container, `no-drag` on interactive elements
- Color tokens in `_colors.css` as CSS custom properties (`--bc-*`)
- Tauri commands use `#[tauri::command]` and are registered in `lib.rs` invoke_handler
- Backend commands in `rust-backend/crates/commands/src/lib.rs` are stubbed — need real implementations

## What's Done
- Vue shell components (Sidebar, TopBar, PlayerBar, NavPills) — build passes
- Color tokens ported from Compose
- Tauri window config with custom chrome
- Window control commands (minimize, maximize, close)
- MPRIS 2.0 implementation (Linux D-Bus) — registered but needs state wiring
- Library, playback, sync command stubs in lib.rs

## What's NOT Done (Feature Gaps)
1. **Audio playback** — `audio/` crate is empty stub; needs rodio + ffmpeg decode
2. **Library persistence** — `db/` crate exists but no real SQLite schema/queries
3. **MPRIS state wiring** — MPRIS registers but doesn't receive live playback updates
4. **Windows Mica/Acrylic** — `set_mica` command is stub
5. **macOS vibrancy** — no DesktopMacFrame equivalent in Tauri
6. **Downloads manager** — no implementation
7. **Lyrics fetching** — no implementation
8. **Discord RPC** — no implementation
9. **Sync/party mode** — `sync/` crate exists but minimal
10. **Search API** — no implementation
11. **Settings/config persistence** — `config/` crate minimal
12. **Tray icon** — referenced in setup but not fully wired

## Constraints
- User is on Windows 11; Rust compilation may fail locally (MSVC linker issues)
- User explicitly forbids local `cargo build` — "NO! DON'T BUILD LOCALLY MY LAPTOP WILL CRASH"
- User wants autonomous continuation with GitHub commits after major changes
- Do NOT modify `desktopApp/` (original Compose app) or `app/` (Android)
- Do NOT touch API keys/secrets

## Memory Notes
- User gets frustrated when assistant ends turn without making a tool call
- Always complete the action in the same turn
- User prefers .md plans over inline work — they want to delegate to another agent
- `write_file` has stale-read guard — must `read_file` first or use `execute_code`/`patch`
- `patch` tool works for targeted edits; `execute_code` with Python for file writes