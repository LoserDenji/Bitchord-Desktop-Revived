//! The bridge contract, ported from `web/shared/src/bridge/protocol.ts`.
//!
//! Every call is a typed request/response pair inside a versioned envelope,
//! identical on the wire to what the browser extension speaks, so the page's
//! own client (`detectExtension` → `relayBridge`) works against this build
//! without a single change.

use serde::{Deserialize, Serialize};

/// Bumped on any breaking change to an envelope or a method's shape.
pub const BRIDGE_PROTOCOL: u32 = 1;

/// Tags every window message so the relay ignores everything else on the page.
pub const BRIDGE_CHANNEL: &str = "bitchord-bridge";

/// What this build can serve; `ping` reports it. `island` is the desktop
/// shell's Dynamic Island (the overlay window and its preference) — the
/// settings page gates its toggle on it, and a browser extension simply never
/// reports it.
pub const CAPABILITIES: &[&str] = &["yt", "yt.auth", "yt.stats", "addon", "lyrics", "spotify", "discord", "island"];

/// A plain HTTP exchange, serialisable across the message channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpRequest {
    pub url: String,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub headers: Option<std::collections::HashMap<String, String>>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub body_encoding: Option<String>, // rename handled in shim layer; see from_wire
}

impl HttpRequest {
    /// The page sends camelCase (`bodyEncoding`); serde picks it up here.
    pub fn from_wire(v: serde_json::Value) -> Option<HttpRequest> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Wire {
            url: String,
            #[serde(default)]
            method: Option<String>,
            #[serde(default)]
            headers: Option<std::collections::HashMap<String, String>>,
            #[serde(default)]
            body: Option<String>,
            #[serde(default)]
            body_encoding: Option<String>,
        }
        serde_json::from_value::<Wire>(v)
            .ok()
            .map(|w| HttpRequest {
                url: w.url,
                method: w.method,
                headers: w.headers,
                body: w.body,
                body_encoding: w.body_encoding,
            })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: std::collections::HashMap<String, String>,
    /// UTF-8 text, or base64 when `body_encoding` is `"base64"`.
    pub body: String,
    /// Sent as camelCase on the wire.
    #[serde(rename = "bodyEncoding")]
    pub body_encoding: &'static str,
    /// The final URL after redirects.
    pub url: String,
}

/// One identity requests can act as: a Google login's own channel, or one of its brand channels.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct YtChannel {
    /// Opaque and stable: what `yt.selectChannel` takes.
    pub id: String,
    pub name: String,
    /// The @handle or byline YouTube shows under the name.
    pub handle: Option<String>,
    pub avatar_url: Option<String>,
    /// A brand channel (sent as `X-Goog-PageId`) rather than the login's own.
    pub brand: bool,
}

/// A Google login, as `X-Goog-AuthUser` addresses it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct YtAccount {
    pub auth_user: String,
    pub name: String,
    pub email: Option<String>,
    pub avatar_url: Option<String>,
    pub channels: Vec<YtChannel>,
}

/// Who signed-in YouTube requests act as. Names and pictures only: the
/// cookies and the signature never leave the core.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct YtAuthState {
    /// The app holds a YouTube session (a SAPISID cookie).
    pub browser_signed_in: bool,
    /// The user lets BitChord use it; "Disconnect" turns this off without signing out of Google.
    pub connected: bool,
    /// True when signed-in requests will be made: signed in, connected, and a channel chosen.
    pub signed_in: bool,
    pub accounts: Vec<YtAccount>,
    /// The channel in use, from `accounts`; null when not signed in.
    pub active: Option<ActiveChannel>,
    /// Signed in but no account could be read: what YouTube answered, for display.
    pub problem: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveChannel {
    pub id: String,
    pub name: String,
    pub handle: Option<String>,
    pub avatar_url: Option<String>,
    pub brand: bool,
    pub auth_user: String,
}

/// The app's Spotify session. Names only, never tokens.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpotifyAuthState {
    /// The app holds a Spotify session (an `sp_dc` cookie).
    pub browser_signed_in: bool,
    pub connected: bool,
    /// A token was minted for the account: library, playlists and canvas will work.
    pub signed_in: bool,
    pub user: Option<SpotifyUser>,
    /// Signed in but no token came back: what went wrong, for display.
    pub problem: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpotifyUser {
    pub name: String,
    pub avatar_url: Option<String>,
}

/// A playlist in the Spotify library; `id` is `liked` for Liked Songs.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpotifyPlaylist {
    pub id: String,
    pub name: String,
    pub owner: Option<String>,
    pub image_url: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpotifyTrack {
    /// The Spotify track id (base-62).
    pub id: String,
    pub title: String,
    /// Credited artists, comma-separated.
    pub artist: String,
    pub album: Option<String>,
    pub duration_ms: i64,
    pub image_url: Option<String>,
}

/// One page of a Spotify list: `raw` counts the entries before filtering, which is what advances the offset.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpotifyPage<T> {
    pub items: Vec<T>,
    pub total: i64,
    pub raw: usize,
}

/// Discord rich presence.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscordState {
    /// This build can publish a presence at all.
    pub available: bool,
    /// An account is connected (its token is held by the core).
    pub connected: bool,
    /// The user turned presence on.
    pub enabled: bool,
    pub user: Option<DiscordUser>,
    pub gateway: DiscordGatewayStatus,
    pub problem: Option<String>,
}

pub type DiscordGatewayStatus = &'static str; // off | connecting | ready | error

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscordUser {
    pub name: String,
    pub username: String,
    pub avatar_url: Option<String>,
}

/// What the card shows, and how (DiscordRPC.updateSong's options).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscordPresenceOptions {
    pub activity_type: String, // listening | playing | watching | competing
    pub status: String,        // online | idle | dnd | invisible
    /// Lead with the song title rather than the artist in member lists.
    pub use_details: bool,
    /// Replaces "BitChord" after "Listening to"; empty keeps it.
    pub activity_name: String,
    pub button1_text: String,
    pub button1_visible: bool,
    pub button2_text: String,
    pub button2_visible: bool,
}

/// The track being played, as the page reports it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscordNowPlaying {
    pub video_id: String,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub artwork_url: Option<String>,
    pub position_ms: i64,
    pub duration_ms: i64,
    pub playback_rate: f64,
}

/// Pushed to the page over the event channel.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", content = "data", rename_all = "snake_case")]
pub enum BridgeEvent {
    YtAuthState(YtAuthState),
    SpotifyAuthState(SpotifyAuthState),
    DiscordState(DiscordState),
}

/// A signed-in request is the same exchange with `auth` set; the core adds the session.
#[derive(Debug, Clone)]
pub struct YtFetchParams {
    pub http: HttpRequest,
    /// Send it as the chosen channel (cookies, SAPISIDHASH, X-Goog-AuthUser/PageId).
    pub auth: bool,
}

// ---- Envelope ------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeRequest {
    pub protocol: u32,
    pub id: String,
    pub method: String,
    /// Raw params, decoded per method (camelCase fields).
    pub params: serde_json::Value,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeError {
    pub code: String,
    pub message: String,
}

pub type BridgeResult = Result<serde_json::Value, (String, String)>; // Ok(result) | Err((code, message))

impl BridgeRequest {
    pub fn params_as<T: for<'de> Deserialize<'de>>(&self) -> Result<T, String> {
        serde_json::from_value(self.params.clone()).map_err(|e| e.to_string())
    }
}

pub fn ok_response(id: &str, result: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "channel": BRIDGE_CHANNEL,
        "protocol": BRIDGE_PROTOCOL,
        "direction": "to-page",
        "id": id,
        "ok": true,
        "result": result,
    })
}

pub fn error_response(id: &str, code: &str, message: &str) -> serde_json::Value {
    serde_json::json!({
        "channel": BRIDGE_CHANNEL,
        "protocol": BRIDGE_PROTOCOL,
        "direction": "to-page",
        "id": id,
        "ok": false,
        "error": { "code": code, "message": message },
    })
}
