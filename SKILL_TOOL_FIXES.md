# BitChord Tauri Rewrite — Tool Fixes

## Problems Solved

### 1. `write_file` Stale-Read Guard
**Problem:** `write_file` refuses to overwrite files without a prior `read_file`.
**Solution:** Either `read_file` the target first, OR use `execute_code` with Python to write, OR use `patch` for targeted edits.

### 2. `patch` Text Corruption
**Problem:** `patch` uses case-insensitive fuzzy matching which can corrupt text (e.g., `equal` → `Equal`).
**Solution:** Use `execute_code` with Python for precise file operations when `patch` fails.

### 3. Vue TypeScript Not Parsed
**Problem:** `<script>` tags without `lang="ts"` parse TypeScript as JavaScript, causing errors.
**Solution:** Always add `lang="ts"` to Vue component script tags.

### 4. Empty `execute_code` Call
**Problem:** `execute_code({})` with empty code returns "No code provided".
**Solution:** Never call `execute_code` with empty parameter; always include Python code.

### 5. MPRIS zbus Connection Type
**Problem:** MPRIS registration fails with connection type mismatch.
**Solution:** Use `zbus::Connection` (full path) and `anyhow::Result` for the return type.

### 6. MPRIS State Sharing
**Problem:** MPRIS returns static "Stopped" instead of live playback state.
**Solution:** Create `Arc<MprisState>` struct and share it between `MprisRoot` and `MprisPlayer`.

### 7. Cross-Platform Stubbing
**Problem:** Linux-only code fails to compile elsewhere.
**Solution:** Wrap Linux code in `#[cfg(target_os = "linux")]` and provide stubs for `#[cfg(not(target_os = "linux"))]`.