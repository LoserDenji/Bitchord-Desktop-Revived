fn main() {
    // The eight desktop commands' permissions live in
    // `permissions/desktop-commands.toml` — the documented
    // application-permissions route. `AppManifest::new().commands([…])` would
    // autogenerate `allow-<command>` permissions, but Tauri's ACL identifiers
    // cannot contain underscores, so those generated names are invalid and
    // tauri-build panics on them. The `main` and `agent` capabilities
    // reference the hyphenated identifiers; without them the agent window
    // (which runs on remote service origins) could never invoke a single
    // desktop command: Tauri rejects *every* IPC call from a remote URL
    // unless a `remote` capability grants it.
    tauri_build::build();
}
