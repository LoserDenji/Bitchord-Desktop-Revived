//! Persistent state, the port of the extension's `browser.storage.local`
//! keys (`yt.prefs`, `spotify.prefs`, `discord.account`, `discord.enabled`,
//! `spotify.webPlayer`, `spotify.deviceId`) plus the session cookies the
//! agent webview hands over. One JSON file, `0600`, in the app data dir.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

pub const MUSIC_ORIGIN: &str = "https://music.youtube.com";
/// The sign-in half of the YouTube session: SID / HSID / SSID / SAPISID live
/// on this domain, the PSID session cookies on `.youtube.com`. A dump without
/// the Google-domain half is missing half the jar the real browser sends.
pub const GOOGLE_ORIGIN: &str = "https://accounts.google.com";
pub const SPOTIFY_ORIGIN: &str = "https://open.spotify.com";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DiscordAccount {
    pub token: String,
    pub user: crate::types::DiscordUser,
}

impl Default for DiscordAccount {
    fn default() -> Self {
        Self {
            token: String::new(),
            user: crate::types::DiscordUser { name: String::new(), username: String::new(), avatar_url: None },
        }
    }
}

/// The Spotify web player's own values, read out of its bundle
/// (DesktopSpotifyToken / SpotifyLibrary ports). Kept across restarts: the
/// bundle is ~4 MB, so it is fetched at most twice a day.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct WebPlayer {
    pub client_version: String,
    /// Character codes of the newest secret, before the XOR the bundle applies.
    pub secret_version: i64,
    pub secret_codes: Vec<i64>,
    /// Persisted-query hashes per operation.
    pub hashes: HashMap<String, String>,
    /// Whether everything came from the live bundle; a partial read is retried sooner.
    pub complete: bool,
    /// Unix millis the read happened at.
    pub at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct StoreData {
    /// "Disconnect" sets this false; the Google session itself is left alone.
    pub yt_connected: bool,
    pub yt_selected_channel_id: Option<String>,
    /// The YouTube cookies the agent window reported, one full
    /// `name=value; Domain=…; Path=…` line per cookie, kept so a restart stays
    /// signed in without spinning the agent up first.
    pub yt_cookie_dump: Option<String>,
    pub spotify_connected: bool,
    /// The `sp_dc` cookie value the agent page reported.
    pub sp_dc: Option<String>,
    pub discord_account: Option<DiscordAccount>,
    pub discord_enabled: bool,
    pub web_player: Option<WebPlayer>,
    pub device_id: Option<String>,
    /// The Dynamic Island's tray toggle. `None` is the un-chosen default and
    /// means on, like the feature shipping enabled; the tray writes Some on
    /// its first flip.
    pub island_enabled: Option<bool>,
}

impl Default for StoreData {
    fn default() -> Self {
        Self {
            yt_connected: true,
            yt_selected_channel_id: None,
            yt_cookie_dump: None,
            spotify_connected: true,
            sp_dc: None,
            discord_account: None,
            discord_enabled: false,
            web_player: None,
            device_id: None,
            island_enabled: None,
        }
    }
}

pub struct Store {
    path: PathBuf,
    data: Mutex<StoreData>,
}

impl Store {
    pub fn load(dir: &std::path::Path) -> Store {
        let _ = std::fs::create_dir_all(dir);
        let path = dir.join("state.json");
        let data = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
        Store { path, data: Mutex::new(data) }
    }

    pub fn with<R>(&self, f: impl FnOnce(&mut StoreData) -> R) -> R {
        let mut guard = self.data.lock().unwrap();
        f(&mut guard)
    }

    pub fn read<R>(&self, f: impl FnOnce(&StoreData) -> R) -> R {
        let guard = self.data.lock().unwrap();
        f(&guard)
    }

    pub fn save(&self) {
        let guard = self.data.lock().unwrap();
        if let Ok(bytes) = serde_json::to_vec_pretty(&*guard) {
            let tmp = self.path.with_extension("tmp");
            if std::fs::write(&tmp, &bytes).is_ok() {
                let _ = std::fs::rename(&tmp, &self.path);
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600));
        }
    }
}
