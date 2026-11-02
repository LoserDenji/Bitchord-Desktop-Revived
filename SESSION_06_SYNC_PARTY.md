# Session 6: Sync / Party Mode

## Context
Tauri rewrite of BitChord. Implement multi-user sync.

## Task
Implement WebSocket-based sync system:

**File 1:** `rust-backend/crates/sync/src/lib.rs`
- `SyncSession` struct with ID, participants, state
- `WebSocketServer` for host mode
- `WebSocketClient` for joiner mode
- Broadcast: play_state, position, skip, vote

**File 2:** `rust-backend/crates/network/src/lib.rs`
- `connect_to_session(session_id: &str)` - establish WebSocket
- `broadcast_message(msg: SyncMessage)` - send to all
- `handle_incoming()` - process messages
- Serde: `SyncMessage { play, pause, next, prev, set_position, vote_skip }`

## Protocol
- WS on port 8443 (configurable)
- JSON messages
- Heartbeat every 5s
- Reconnect on disconnect

## Constraints
- NEVER run `cargo build` locally
- NEVER modify `desktopApp/` or `app/`
- Commit after basic sync works

## Start
Read `PROJECT_BRIEF.md`, then work on `rust-backend/crates/sync/src/lib.rs`.