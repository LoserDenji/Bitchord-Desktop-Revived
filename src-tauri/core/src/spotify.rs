//! Spotify with the user's own open.spotify.com session. Port of
//! `web/extension/lib/spotify.ts` (which ports DesktopSpotifyToken,
//! SpotifyLibrary.kt and SpotifyCanvasQuery.kt).
//!
//! The `sp_dc` cookie is handed over by the agent webview once, kept in the
//! store, and sent as an explicit `Cookie` header on the one token request
//! that needs it — the same thing the extension's session rule did.

use std::sync::Arc;
use std::time::Duration;

use hmac::{Hmac, Mac};
use regex::Regex;
use serde_json::{json, Value};

use crate::store::Store;
use crate::types::{SpotifyAuthState, SpotifyPage, SpotifyPlaylist, SpotifyTrack, SpotifyUser};
use crate::yt::now_ms;

const WEB_PLAYER: &str = "https://open.spotify.com";
const CDN: &str = "https://open.spotifycdn.com/cdn/build/web-player/";
const PATHFINDER: &str = "https://api-partner.spotify.com/pathfinder/v2/query";
const CLIENT_TOKEN: &str = "https://clienttoken.spotify.com/v1/clienttoken";

/// Liked Songs has no playlist uri; SpotifyLibrary.LIKED_ID.
pub const LIKED_ID: &str = "liked";
const LIKED_COVER: &str = "https://misc.scdn.co/liked-songs/liked-songs-640.png";

const OPERATIONS: &[&str] = &["libraryV3", "fetchPlaylist", "fetchLibraryTracks", "canvas", "searchTracks", "profileAttributes"];

/// The last hashes known to work (SpotifyLibrary.kt, SpotifyCanvasQuery.kt, and the bundle on 2026-10-16).
fn known_hash(operation: &str) -> &'static str {
    match operation {
        "libraryV3" => "390c78e5b951029bad359785e69b07b536a509c581cbcd0aded5e5067f187455",
        "fetchPlaylist" => "21ea11a6ac75a5f3e01fd365d29c18e0a4a8b7dc09f89bbb5d73910190c6acaf",
        "fetchLibraryTracks" => "087278b20b743578a6262c2b0b4bcd20d879c503cc359a2285baf083ef944240",
        "canvas" => "575138ab27cd5c1b3e54da54d0a7cc8d85485402de26340c2145f0f6bb5e7a9f",
        "searchTracks" => "b02683192a98dde7966b5e6655a79eeb62713eab703eda9902c932818dd52751",
        "profileAttributes" => "08ffb4730af3746e04a8301396f20875dbbce10c75243803091a9274eacc8ac0",
        _ => "",
    }
}

/// DesktopSpotifyToken.TOTP_SECRETS: the fallback when the bundle can't be read.
const KNOWN_SECRET_VERSION: i64 = 12;
const KNOWN_SECRET_CODES: &[i64] = &[107, 81, 49, 57, 67, 93, 87, 81, 69, 67, 40, 93, 48, 50, 46, 91, 94, 113, 41, 47];

const FOUND_TTL_MS: i64 = 12 * 60 * 60_000;
const MISSED_TTL_MS: i64 = 30 * 60_000;
const RETRY_MS: i64 = 60_000;

pub struct SpotifyService {
    pub client: reqwest::Client,
    store: Arc<Store>,
    access: tokio::sync::Mutex<Option<AccessToken>>,
    client_tok: tokio::sync::Mutex<Option<(String, i64)>>,
    retry_after: tokio::sync::Mutex<i64>,
    profile_cache: tokio::sync::Mutex<Option<SpotifyUser>>,
}

#[derive(Debug, Clone)]
struct AccessToken {
    token: String,
    expires_at: i64,
    client_id: Option<String>,
    /// The `sp_dc` it was minted from: another one means another sign-in.
    cookie: String,
}

pub struct AuthError(pub String);

impl SpotifyService {
    pub fn new(store: Arc<Store>) -> SpotifyService {
        let client = reqwest::Client::builder()
            .user_agent(concat!(
                "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) ",
                "Chrome/138.0.0.0 Safari/537.36"
            ))
            .timeout(Duration::from_secs(30))
            .build()
            .expect("spotify client");
        SpotifyService {
            client,
            store,
            access: tokio::sync::Mutex::new(None),
            client_tok: tokio::sync::Mutex::new(None),
            retry_after: tokio::sync::Mutex::new(0),
            profile_cache: tokio::sync::Mutex::new(None),
        }
    }

    async fn sp_dc(&self) -> Option<String> {
        self.store.read(|d| d.sp_dc.clone())
    }

    pub fn invalidate_session(&self) {
        if let Ok(mut access) = self.access.try_lock() {
            *access = None;
        }
        if let Ok(mut client) = self.client_tok.try_lock() {
            *client = None;
        }
        if let Ok(mut retry) = self.retry_after.try_lock() {
            *retry = 0;
        }
        if let Ok(mut profile) = self.profile_cache.try_lock() {
            *profile = None;
        }
        // The store's cache of the web player survives: it is not tied to a session.
    }

    pub async fn set_connected(&self, connected: bool) -> SpotifyAuthState {
        self.store.with(|d| d.spotify_connected = connected);
        self.store.save();
        self.invalidate_session();
        self.auth_state().await
    }

    // ---- The web player's own values ----------------------------------------

    /// A JavaScript string literal's body, unescaped (spotify.unescapeJs).
    pub fn unescape_js(body: &str) -> String {
        let mut out = String::with_capacity(body.len());
        let mut chars = body.chars().peekable();
        while let Some(c) = chars.next() {
            if c != '\\' {
                out.push(c);
                continue;
            }
            match chars.next() {
                Some('u') => {
                    let rest: String = chars.clone().collect();
                    let hex_len = if rest.starts_with('{') {
                        rest.find('}').map(|i| i + 1).unwrap_or(0)
                    } else {
                        4
                    };
                    if hex_len > 0 {
                        let hex: String = rest.chars().skip(if rest.starts_with('{') { 1 } else { 0 }).take(hex_len - if rest.starts_with('{') { 2 } else { 0 }).collect();
                        if let Ok(code) = u32::from_str_radix(&hex, 16) {
                            if let Some(ch) = char::from_u32(code) {
                                out.push(ch);
                            }
                            for _ in 0..hex_len {
                                chars.next();
                            }
                            continue;
                        }
                    }
                    out.push('u');
                }
                Some('x') => {
                    let rest: String = chars.clone().collect();
                    if rest.len() >= 2 {
                        if let Ok(code) = u8::from_str_radix(&rest[..2], 16) {
                            out.push(code as char);
                            chars.next();
                            chars.next();
                            continue;
                        }
                    }
                    out.push('x');
                }
                Some(esc) => {
                    out.push(match esc {
                        'n' => '\n',
                        'r' => '\r',
                        't' => '\t',
                        'b' => '\u{8}',
                        'f' => '\u{c}',
                        'v' => '\u{b}',
                        '0' => '\0',
                        other => other,
                    });
                }
                None => out.push('\\'),
            }
        }
        out
    }

    /// `{secret:'…',version:61}` as the bundle declares them, newest first (spotify.secretsIn).
    fn secrets_in(script: &str) -> Vec<(i64, Vec<i64>)> {
        let re = fancy_regex::Regex::new(r#"\{secret:(['"])((?:\\.|(?!\1).)*)\1,version:(\d+)\}"#).unwrap();
        let mut found = Vec::new();
        for caps in re.captures_iter(script).flatten() {
            let version: i64 = caps.get(3).and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
            let literal = caps.get(2).map(|m| m.as_str()).unwrap_or_default();
            let codes: Vec<i64> = Self::unescape_js(literal).chars().map(|c| c as i64).collect();
            found.push((version, codes));
        }
        found.sort_by(|a, b| b.0.cmp(&a.0));
        found
    }

    /// `"canvas","query","<64 hex>"`: SpotifyCanvasQuery.findQueryHash.
    fn query_hash_in(script: &str, operation: &str) -> Option<String> {
        let pattern = format!(r#"["']{operation}["']\s*,\s*["']query["']\s*,\s*["']([0-9a-f]{{64}})["']"#);
        Regex::new(&pattern).ok()?.captures(script)?.get(1).map(|m| m.as_str().to_string())
    }

    async fn text_of(&self, url: &str) -> Option<String> {
        let response = self.client.get(url).send().await.ok()?;
        if response.status().is_success() {
            response.text().await.ok()
        } else {
            None
        }
    }

    async fn read_web_player(&self) -> crate::store::WebPlayer {
        let html = self.text_of(&format!("{WEB_PLAYER}/")).await.unwrap_or_default();
        let mut client_version = String::new();
        let re = Regex::new(r#"<script id="appServerConfig" type="text/plain">([^<]+)</script>"#).unwrap();
        if let Some(config) = re.captures(&html).and_then(|c| c.get(1)) {
            if let Ok(bytes) = crate::http::from_base64(config.as_str()) {
                if let Ok(parsed) = serde_json::from_slice::<Value>(&bytes) {
                    client_version = parsed.get("clientVersion").and_then(|v| v.as_str()).unwrap_or("").to_string();
                }
            }
        }
        let bundle_re = Regex::new(r"https://open\.spotifycdn\.com/cdn/build/web-player/web-player\.[0-9a-f]+\.js").unwrap();
        let bundle_url = bundle_re.find(&html).map(|m| m.as_str().to_string());
        let bundle = match bundle_url {
            Some(url) => self.text_of(&url).await.unwrap_or_default(),
            None => String::new(),
        };
        // searchTracks is declared in the search route's chunk, which the player
        // loads on demand; its file name comes from the bundle's chunk table.
        let mut search_chunk = String::new();
        let id_re = Regex::new(r#"(\d+):"xpui-routes-search""#).unwrap();
        if let Some(id) = id_re.captures(&bundle).and_then(|c| c.get(1)).map(|m| m.as_str().to_string()) {
            let chunk_re = Regex::new(&format!(r#"\b{id}:"([0-9a-f]{{8}})""#)).unwrap();
            for part in chunk_re.captures_iter(&bundle) {
                if let Some(p) = part.get(1) {
                    if let Some(chunk) = self.text_of(&format!("{CDN}xpui-routes-search.{}.js", p.as_str())).await {
                        search_chunk = chunk;
                        break;
                    }
                }
            }
        }
        let secrets = Self::secrets_in(&bundle);
        let mut hashes = std::collections::HashMap::new();
        for op in OPERATIONS {
            let hash = Self::query_hash_in(&bundle, op).or_else(|| Self::query_hash_in(&search_chunk, op));
            hashes.insert(op.to_string(), hash.unwrap_or_else(|| known_hash(op).to_string()));
        }
        let complete = !secrets.is_empty() && !client_version.is_empty();
        let (secret_version, secret_codes) = secrets
            .first()
            .map(|(v, codes)| (*v, codes.clone()))
            .unwrap_or((KNOWN_SECRET_VERSION, KNOWN_SECRET_CODES.to_vec()));
        crate::store::WebPlayer { client_version: if client_version.is_empty() { "1.2.77.0".into() } else { client_version }, secret_version, secret_codes, hashes, complete, at: now_ms() }
    }

    async fn web_player(&self, force: bool) -> crate::store::WebPlayer {
        if !force {
            let cached = self.store.read(|d| d.web_player.clone());
            if let Some(player) = cached {
                let ttl = if player.complete { FOUND_TTL_MS } else { MISSED_TTL_MS };
                if now_ms() - player.at < ttl {
                    return player;
                }
            }
        }
        let player = self.read_web_player().await;
        self.store.with(|d| d.web_player = Some(player.clone()));
        self.store.save();
        player
    }

    // ---- TOTP (DesktopSpotifyToken.totp) ------------------------------------

    /// Six digits over SHA-1, thirty-second steps, from the secret's codes XORed with their index.
    pub fn totp(codes: &[i64], epoch_ms: i64) -> String {
        let key: String = codes
            .iter()
            .enumerate()
            .map(|(i, code)| {
                let c = code ^ (((i % 33) + 9) as i64);
                char::from_u32(c as u32).unwrap_or('?').to_string()
            })
            .collect();
        let counter = ((epoch_ms / 1000 / 30) as u64).to_be_bytes();
        let mut mac = <Hmac<sha1::Sha1> as Mac>::new_from_slice(key.as_bytes()).expect("hmac");
        mac.update(&counter);
        let hash = mac.finalize().into_bytes();
        let offset = (hash[hash.len() - 1] & 0x0f) as usize;
        let binary = ((hash[offset] & 0x7f) as u32) << 24
            | (hash[offset + 1] as u32) << 16
            | (hash[offset + 2] as u32) << 8
            | (hash[offset + 3] as u32);
        format!("{:06}", binary % 1_000_000)
    }

    // ---- Tokens ---------------------------------------------------------------

    async fn mint(&self) -> Result<AccessToken, AuthError> {
        if !self.store.read(|d| d.spotify_connected) {
            return Err(AuthError("Spotify is disconnected".into()));
        }
        let Some(cookie) = self.sp_dc().await.filter(|c| !c.is_empty()) else {
            return Err(AuthError("Not signed in to Spotify in this app".into()));
        };
        if now_ms() < *self.retry_after.lock().await {
            return Err(AuthError("Spotify would not mint a token; trying again shortly".into()));
        }
        for force in [false, true] {
            let player = self.web_player(force).await;
            let code = Self::totp(&player.secret_codes, now_ms());
            let query = [
                ("reason", "init"),
                ("productType", "web-player"),
                ("totp", &code),
                ("totpServer", &code),
                ("totpVer", &player.secret_version.to_string()),
            ];
            let url = format!("{WEB_PLAYER}/api/token?{}", {
                let mut s = String::new();
                for (i, (k, v)) in query.iter().enumerate() {
                    if i > 0 {
                        s.push('&');
                    }
                    s.push_str(&format!("{k}={v}"));
                }
                s
            });
            let response = self
                .client
                .get(&url)
                .header("Cookie", format!("sp_dc={cookie}"))
                .header("Referer", "https://open.spotify.com/")
                .send()
                .await;
            let response = match response {
                Ok(r) => r,
                Err(_) => break,
            };
            let status = response.status().as_u16();
            let body: Value = response.json().await.unwrap_or(json!({}));
            // The web player reads this flag to learn its secret has been retired.
            if body.get("totpVerExpired").and_then(|v| v.as_bool()).unwrap_or(false) && !force {
                continue;
            }
            if status == 401 {
                return Err(AuthError("Your Spotify session has expired. Sign in again at open.spotify.com.".into()));
            }
            if body.get("isAnonymous").and_then(|v| v.as_bool()).unwrap_or(false) {
                return Err(AuthError("Not signed in to Spotify in this app".into()));
            }
            if let Some(token) = body.get("accessToken").and_then(|v| v.as_str()) {
                return Ok(AccessToken {
                    token: token.to_string(),
                    expires_at: body.get("accessTokenExpirationTimestampMs").and_then(|v| v.as_i64()).unwrap_or(now_ms() + 3_600_000),
                    client_id: body.get("clientId").and_then(|v| v.as_str()).map(|s| s.to_string()),
                    cookie,
                });
            }
            if force {
                break;
            }
        }
        *self.retry_after.lock().await = now_ms() + RETRY_MS;
        Err(AuthError("Spotify would not mint a token".into()))
    }

    async fn access_token(&self) -> Result<AccessToken, AuthError> {
        let cookie = self.sp_dc().await.unwrap_or_default();
        // Checked on every use rather than left to cookie watchers: a token
        // from another sign-in must never be used.
        let cached = self.access.lock().await.clone();
        if let Some(access) = &cached {
            if access.cookie != cookie {
                self.invalidate_session();
            } else if now_ms() < access.expires_at - 30_000 {
                return Ok(access.clone());
            }
        }
        let token = self.mint().await?;
        *self.access.lock().await = Some(token.clone());
        Ok(token)
    }

    /// SpotifyToken.clientToken: best-effort, a request without it is still sent.
    async fn client_token(&self) -> Option<String> {
        {
            let client = self.client_tok.lock().await;
            if let Some((token, expires_at)) = client.as_ref() {
                if now_ms() < *expires_at - 30_000 {
                    return Some(token.clone());
                }
            }
        }
        let bearer = self.access_token().await.ok()?;
        let client_id = bearer.client_id.clone()?;
        let player = self.web_player(false).await;
        let device_id = {
            let stored = self.store.read(|d| d.device_id.clone());
            match stored {
                Some(id) => id,
                None => {
                    let id = uuid_like();
                    self.store.with(|d| d.device_id = Some(id.clone()));
                    self.store.save();
                    id
                }
            }
        };
        let body = json!({
            "client_data": {
                "client_version": player.client_version,
                "client_id": client_id,
                "js_sdk_data": {
                    "device_brand": "unknown",
                    "device_model": "unknown",
                    "os": "linux",
                    "os_version": "unknown",
                    "device_id": device_id,
                    "device_type": "computer",
                },
            },
        });
        let response = self
            .client
            .post(CLIENT_TOKEN)
            // The bare type, deliberately: clienttoken 400s on a charset suffix.
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .body(body.to_string())
            .send()
            .await
            .ok()?;
        let parsed: Value = response.json().await.ok()?;
        let token = parsed
            .get("granted_token")
            .and_then(|g| g.get("token"))
            .and_then(|t| t.as_str())?
            .to_string();
        let expires_after = parsed
            .get("granted_token")
            .and_then(|g| g.get("expires_after_seconds"))
            .and_then(|e| e.as_i64())
            .unwrap_or(3600);
        *self.client_tok.lock().await = Some((token.clone(), now_ms() + expires_after * 1000));
        Some(token)
    }

    // ---- Pathfinder ------------------------------------------------------------

    async fn gql(&self, operation: &str, variables: Value) -> Result<Value, AuthError> {
        self.gql_inner(operation, variables, false).await
    }

    async fn gql_inner(&self, operation: &str, variables: Value, retried: bool) -> Result<Value, AuthError> {
        let bearer = self.access_token().await?;
        let player = self.web_player(false).await;
        let client_tok = self.client_token().await;
        let mut builder = self
            .client
            .post(PATHFINDER)
            .header("Authorization", format!("Bearer {}", bearer.token))
            .header("Accept", "application/json")
            .header("Content-Type", "application/json;charset=UTF-8")
            .header("App-Platform", "WebPlayer");
        if let Some(token) = client_tok {
            builder = builder.header("Client-Token", token);
        }
        let body = json!({
            "variables": variables,
            "operationName": operation,
            "extensions": { "persistedQuery": { "version": 1, "sha256Hash": player.hashes.get(operation).cloned().unwrap_or_default() } },
        });
        let response = builder.body(body.to_string()).send().await.map_err(|e| AuthError(e.to_string()))?;
        if response.status().as_u16() == 401 && !retried {
            self.invalidate_session();
            return Box::pin(self.gql_inner(operation, variables, true)).await;
        }
        let parsed: Option<Value> = response.json().await.ok();
        let errors: Vec<String> = parsed
            .as_ref()
            .and_then(|p| p.get("errors"))
            .and_then(|e| e.as_array())
            .map(|list| list.iter().filter_map(|e| e.get("message").and_then(|m| m.as_str())).map(|s| s.to_string()).collect())
            .unwrap_or_default();
        // A rebuilt web player retires the old hash: look it up again once.
        if errors.iter().any(|m| m.to_lowercase().contains("persistedquerynotfound")) && !retried {
            self.web_player(true).await;
            return Box::pin(self.gql_inner(operation, variables, true)).await;
        }
        let Some(body) = parsed else {
            return Err(AuthError(format!("Spotify {operation} answered with something other than JSON")));
        };
        if !errors.is_empty() && body.get("data").is_none() {
            return Err(AuthError(errors.join("; ")));
        }
        Ok(body)
    }
}

fn uuid_like() -> String {
    let mut bytes = [0u8; 16];
    getrandom_bytes(&mut bytes);
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    hex
}

fn getrandom_bytes(buf: &mut [u8]) {
    use std::time::{SystemTime, UNIX_EPOCH};
    let mut seed = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0x9e3779b97f4a7c15);
    for byte in buf.iter_mut() {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        *byte = (seed & 0xff) as u8;
    }
}

// ---- Parsing (SpotifyLibrary.kt) ------------------------------------------------

fn obj(v: &Value) -> Option<&serde_json::Map<String, Value>> {
    v.as_object()
}

fn path<'a>(v: &'a Value, keys: &[&str]) -> &'a Value {
    let mut at = v;
    for key in keys {
        at = obj(at).and_then(|o| o.get(*key)).unwrap_or(&Value::Null);
    }
    at
}

/// The widest source: Spotify lists sizes in no fixed order (SpotifyLibrary.largestSource).
fn largest(sources: &Value) -> Option<String> {
    let mut best: Option<(String, f64)> = None;
    for source in sources.as_array()? {
        let url = path(source, &["url"]).as_str()?.to_string();
        let width = path(source, &["width"]).as_f64().unwrap_or(0.0);
        if best.as_ref().map(|(_, w)| width > *w).unwrap_or(true) {
            best = Some((url, width));
        }
    }
    best.map(|(url, _)| url)
}

/// The smallest source at least `min` wide, else the widest: list rows don't need 640px.
fn sized(sources: &Value, min: f64) -> Option<String> {
    let mut list: Vec<(String, f64)> = sources
        .as_array()?
        .iter()
        .filter_map(|s| {
            let url = path(s, &["url"]).as_str()?.to_string();
            let width = path(s, &["width"]).as_f64().unwrap_or(0.0);
            Some((url, width))
        })
        .collect();
    list.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    list.iter().find(|(_, width)| *width >= min).or_else(|| list.last()).map(|(url, _)| url.clone())
}

fn artists_of(data: &Value) -> String {
    path(data, &["artists", "items"])
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|a| path(a, &["profile", "name"]).as_str().map(|s| s.trim().to_string()))
                .filter(|n| !n.is_empty())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
}

fn track_of(data: &Value, uri: Option<&str>) -> Option<SpotifyTrack> {
    let title = path(data, &["name"]).as_str()?.trim().to_string();
    let uri = uri?;
    let album = path(data, &["albumOfTrack"]);
    Some(SpotifyTrack {
        id: uri.split(':').next_back().unwrap_or_default().to_string(),
        title,
        artist: artists_of(data),
        album: album.get("name").and_then(|n| n.as_str()).map(|s| s.to_string()),
        duration_ms: path(data, &["trackDuration", "totalMilliseconds"])
            .as_i64()
            .or_else(|| path(data, &["duration", "totalMilliseconds"]).as_i64())
            .unwrap_or(0),
        image_url: sized(path(album, &["coverArt", "sources"]), 300.0),
    })
}

pub fn parse_playlist_page(root: &Value) -> Result<SpotifyPage<SpotifyPlaylist>, String> {
    let library = path(root, &["data", "me", "libraryV3"]);
    let library = obj(library).ok_or("Spotify library response was empty")?;
    let items_raw = library.get("items").and_then(|i| i.as_array()).cloned().unwrap_or_default();
    let mut items = Vec::new();
    for entry in &items_raw {
        let wrapper = match entry.get("item").and_then(|i| i.as_object()) {
            Some(w) => w,
            None => continue,
        };
        let typename = wrapper.get("__typename").and_then(|t| t.as_str()).unwrap_or_default().to_lowercase();
        if !typename.contains("playlist") {
            continue;
        }
        let data = wrapper
            .get("data")
            .and_then(|d| d.as_object())
            .map(|o| Value::Object(o.clone()))
            .ok_or("Spotify library playlist entry had no data")?;
        if data.get("__typename").and_then(|t| t.as_str()) != Some("Playlist") {
            continue;
        }
        let uri = wrapper.get("_uri").and_then(|u| u.as_str());
        let name = data.get("name").and_then(|n| n.as_str());
        let (Some(uri), Some(name)) = (uri, name) else { continue };
        if name.trim().is_empty() {
            continue;
        }
        let images = path(&data, &["images", "items"]).as_array().cloned().unwrap_or_default();
        let image_url = images.first().and_then(|first| largest(&first.get("sources").cloned().unwrap_or(Value::Null)));
        items.push(SpotifyPlaylist {
            id: uri.split(':').next_back().unwrap_or_default().to_string(),
            name: name.to_string(),
            owner: path(&data, &["ownerV2", "data", "name"]).as_str().map(|s| s.to_string()),
            image_url,
        });
    }
    let total = library.get("totalCount").and_then(|t| t.as_i64()).unwrap_or(items_raw.len() as i64);
    Ok(SpotifyPage { items, total, raw: items_raw.len() })
}

pub fn parse_track_page(root: &Value) -> Result<SpotifyPage<SpotifyTrack>, String> {
    let content = path(root, &["data", "playlistV2", "content"]);
    let content = obj(content).ok_or("Spotify playlist response was empty")?;
    let items_raw = content.get("items").and_then(|i| i.as_array()).cloned().unwrap_or_default();
    let mut items = Vec::new();
    for entry in &items_raw {
        let data = path(entry, &["itemV2", "data"]);
        let uri = data.get("uri").and_then(|u| u.as_str()).or_else(|| path(entry, &["itemV2", "data", "_uri"]).as_str());
        if let Some(track) = track_of(data, uri) {
            items.push(track);
        }
    }
    let total = content.get("totalCount").and_then(|t| t.as_i64()).unwrap_or(items_raw.len() as i64);
    Ok(SpotifyPage { items, total, raw: items_raw.len() })
}

pub fn parse_liked_page(root: &Value) -> Result<SpotifyPage<SpotifyTrack>, String> {
    let tracks = path(root, &["data", "me", "library", "tracks"]);
    let tracks = obj(tracks).ok_or("Spotify liked songs response was empty")?;
    let items_raw = tracks.get("items").and_then(|i| i.as_array()).cloned().unwrap_or_default();
    let mut items = Vec::new();
    for entry in &items_raw {
        let wrapper = entry.get("track").cloned().unwrap_or(Value::Null);
        let data = wrapper.get("data").cloned().unwrap_or(Value::Null);
        let uri = wrapper
            .get("_uri")
            .and_then(|u| u.as_str())
            .or_else(|| data.get("uri").and_then(|u| u.as_str()));
        if let Some(track) = track_of(&data, uri) {
            items.push(track);
        }
    }
    let total = tracks.get("totalCount").and_then(|t| t.as_i64()).unwrap_or(items_raw.len() as i64);
    Ok(SpotifyPage { items, total, raw: items_raw.len() })
}

pub fn parse_search_tracks(root: &Value) -> Vec<SpotifyTrack> {
    path(root, &["data", "searchV2", "tracksV2", "items"])
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|entry| {
                    let data = path(entry, &["item", "data"]);
                    track_of(data, data.get("uri").and_then(|u| u.as_str()))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// SpotifyCanvasQuery.parse: a playable `.mp4`, or null.
pub fn parse_canvas(root: &Value) -> Option<String> {
    let url = path(root, &["data", "trackUnion", "canvas", "url"]).as_str()?.to_string();
    let re = Regex::new(r"(?i)\.mp4(?:[?#]|$)").unwrap();
    if url.starts_with("https://") && re.is_match(&url) {
        Some(url)
    } else {
        None
    }
}

impl SpotifyService {
    fn page_error(e: String) -> (String, String) {
        (e, "internal".into())
    }

    /// The library's playlists, 50 at a time; Liked Songs leads the first page.
    pub async fn playlists(&self, offset: i64) -> Result<SpotifyPage<SpotifyPlaylist>, (String, String)> {
        let root = self
            .gql(
                "libraryV3",
                json!({
                    "filters": ["Playlists"],
                    "order": Value::Null,
                    "textFilter": "",
                    "features": ["LIKED_SONGS", "YOUR_EPISODES_V2", "PRERELEASES", "EVENTS"],
                    "limit": 50,
                    "offset": offset,
                    "flatten": true,
                    "expandedFolders": [],
                    "folderUri": Value::Null,
                    "includeFoldersWhenFlattening": false,
                }),
            )
            .await
            .map_err(|AuthError(m)| (m, "unauthenticated".into()))?;
        let mut page = parse_playlist_page(&root).map_err(Self::page_error)?;
        // Liked Songs is not a playlist to the library query; it always comes first.
        if offset == 0 {
            page.items.insert(
                0,
                SpotifyPlaylist { id: LIKED_ID.into(), name: "Liked Songs".into(), owner: None, image_url: Some(LIKED_COVER.into()) },
            );
        }
        Ok(page)
    }

    /// A playlist's (or Liked Songs') tracks, 100 at a time.
    pub async fn tracks(&self, playlist_id: &str, offset: i64) -> Result<SpotifyPage<SpotifyTrack>, (String, String)> {
        if playlist_id == LIKED_ID {
            let root = self.gql("fetchLibraryTracks", json!({ "offset": offset, "limit": 100 })).await.map_err(|AuthError(m)| (m, "unauthenticated".into()))?;
            return parse_liked_page(&root).map_err(Self::page_error);
        }
        let root = self
            .gql(
                "fetchPlaylist",
                json!({ "uri": format!("spotify:playlist:{playlist_id}"), "offset": offset, "limit": 100, "enableWatchFeedEntrypoint": false }),
            )
            .await
            .map_err(|AuthError(m)| (m, "unauthenticated".into()))?;
        parse_track_page(&root).map_err(Self::page_error)
    }

    /// A playlist's name and cover at full size (the library list only carries a thumbnail).
    pub async fn playlist_info(&self, playlist_id: &str) -> Result<(String, Option<String>, Option<String>, i64), (String, String)> {
        if playlist_id == LIKED_ID {
            let root = self.gql("fetchLibraryTracks", json!({ "offset": 0, "limit": 1 })).await.map_err(|AuthError(m)| (m, "unauthenticated".into()))?;
            let page = parse_liked_page(&root).map_err(Self::page_error)?;
            return Ok(("Liked Songs".into(), None, Some(LIKED_COVER.into()), page.total));
        }
        let root = self
            .gql(
                "fetchPlaylist",
                json!({ "uri": format!("spotify:playlist:{playlist_id}"), "offset": 0, "limit": 1, "enableWatchFeedEntrypoint": false }),
            )
            .await
            .map_err(|AuthError(m)| (m, "unauthenticated".into()))?;
        let playlist = path(&root, &["data", "playlistV2"]);
        let images = path(playlist, &["images", "items"]).as_array().cloned().unwrap_or_default();
        Ok((
            playlist.get("name").and_then(|n| n.as_str()).unwrap_or("Playlist").to_string(),
            path(playlist, &["ownerV2", "data", "name"]).as_str().map(|s| s.to_string()),
            images.first().and_then(|first| largest(&first.get("sources").cloned().unwrap_or(Value::Null))),
            path(playlist, &["content", "totalCount"]).as_i64().unwrap_or(0),
        ))
    }

    pub async fn search_tracks(&self, query: &str) -> Result<Vec<SpotifyTrack>, (String, String)> {
        let root = self
            .gql(
                "searchTracks",
                json!({
                    "searchTerm": query,
                    "offset": 0,
                    "limit": 10,
                    "numberOfTopResults": 5,
                    "includeAudiobooks": false,
                    "includePreReleases": false,
                    "includeAuthors": false,
                }),
            )
            .await
            .map_err(|AuthError(m)| (m, "unauthenticated".into()))?;
        Ok(parse_search_tracks(&root))
    }

    /// A track's canvas by Spotify id. Signed in only: anonymous tokens are shown none.
    pub async fn canvas(&self, track_id: &str) -> Result<Option<String>, (String, String)> {
        let root = self
            .gql("canvas", json!({ "trackUri": format!("spotify:track:{track_id}") }))
            .await
            .map_err(|AuthError(m)| (m, "unauthenticated".into()))?;
        Ok(parse_canvas(&root))
    }

    pub async fn auth_state(&self) -> SpotifyAuthState {
        let cookie = self.sp_dc().await.unwrap_or_default();
        let browser_signed_in = !cookie.is_empty();
        let connected = self.store.read(|d| d.spotify_connected);
        let base = SpotifyAuthState { browser_signed_in, connected, signed_in: false, user: None, problem: None };
        if !browser_signed_in || !connected {
            return base;
        }
        match self.profile().await {
            Ok(user) => SpotifyAuthState { signed_in: true, user: Some(user), ..base },
            Err(AuthError(message)) => SpotifyAuthState { problem: Some(message), ..base },
        }
    }

    async fn profile(&self) -> Result<SpotifyUser, AuthError> {
        if let Some(user) = self.profile_cache.lock().await.clone() {
            return Ok(user);
        }
        let root = self.gql("profileAttributes", json!({})).await?;
        let profile = path(&root, &["data", "me", "profile"]);
        let name = profile
            .get("name")
            .and_then(|n| n.as_str())
            .or_else(|| profile.get("username").and_then(|u| u.as_str()))
            .unwrap_or("Spotify")
            .to_string();
        let avatar_url = sized(path(profile, &["avatar", "sources"]), 64.0);
        let user = SpotifyUser { name, avatar_url };
        *self.profile_cache.lock().await = Some(user.clone());
        Ok(user)
    }
}
