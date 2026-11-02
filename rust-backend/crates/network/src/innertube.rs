//! YouTube Music InnerTube client.
//!
//! InnerTube is YouTube Music's internal API (aka `youtubei`). The client
//! speaks to authenticated endpoints using the `WEB_REMIX` client identity.
//!
//! This module provides a minimal async client that wraps the raw HTTP calls.
//! Full response parsing and domain model mapping is in [`crate::innertube::models`].

use std::sync::Arc;
use std::time::Duration;

use reqwest::Client;
use serde_json::Value;
use tokio::sync::RwLock;
use tracing::{debug, warn};

use crate::error::{NetworkError, Result};

/// The Base URL for InnerTube requests.
pub const MUSIC_BASE: &str = "https://music.youtube.com/youtubei/v1";
/// The origin header value expected by YouTube.
pub const MUSIC_ORIGIN: &str = "https://music.youtube.com";
/// The user agent string matching the web client.
pub const WEB_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/145.0.0.0 Safari/537.36";

/// Default retry delay for transient errors.
const RETRY_BACKOFF: Duration = Duration::from_millis(500);
/// Maximum number of retry attempts for transport errors.
const MAX_RETRIES: u32 = 3;

/// Session state extracted from the music.youtube.com shell.
#[derive(Debug, Clone, Default)]
pub struct SessionScope {
    /// The `onBehalfOfUser` value for the selected account.
    pub data_sync_id: Option<String>,
    /// The brand channel ID (`DELEGATED_SESSION_ID`).
    pub page_id: Option<String>,
    /// Which cookie jar entry this session belongs to (`SESSION_INDEX`).
    pub auth_user: String,
    /// The client version from the shell.
    pub client_version: Option<String>,
}

/// Current session cookie and associated state.
#[derive(Debug, Clone)]
pub struct InnerTubeSession {
    /// The HTTP-only cookie value containing SAPISID, etc.
    pub cookie: Option<String>,
    /// The visitor data string (unique per session).
    pub visitor_data: Option<String>,
    /// Whether visitor_data is bound to the signed-in session.
    pub visitor_data_bound: bool,
    /// Resolved session scope (extracted from shell).
    pub scope: SessionScope,
}

impl Default for InnerTubeSession {
    fn default() -> Self {
        Self {
            cookie: None,
            visitor_data: None,
            visitor_data_bound: false,
            scope: SessionScope {
                auth_user: String::from("0"),
                ..Default::default()
            },
        }
    }
}

impl InnerTubeSession {
    pub fn new(cookie: Option<String>) -> Self {
        Self {
            cookie,
            visitor_data: None,
            visitor_data_bound: false,
            scope: SessionScope {
                auth_user: String::from("0"),
                ..Default::default()
            },
        }
    }

    pub fn is_signed_in(&self) -> bool {
        self.cookie.is_some()
    }
}

/// The main InnerTube client for YouTube Music API calls.
#[derive(Debug, Clone)]
pub struct InnerTubeClient {
    client: Client,
    session: Arc<RwLock<InnerTubeSession>>,
}

impl InnerTubeClient {
    /// Create a new client with a loaded session cookie.
    pub fn new(cookie: Option<String>) -> Result<Self> {
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(30))
            .user_agent(WEB_USER_AGENT)
            .build()?;

        Ok(Self {
            client,
            session: Arc::new(RwLock::new(InnerTubeSession::new(cookie))),
        })
    }

    /// Create a client without a cookie (anonymous/guest mode).
    pub fn anonymous() -> Result<Self> {
        Self::new(None)
    }

    /// Set or replace the session cookie.
    pub async fn set_cookie(&self, cookie: Option<String>) {
        let mut session = self.session.write().await;
        if session.cookie != cookie {
            // Invalidate cache when cookie changes.
            session.scope = SessionScope::default();
            session.visitor_data = None;
            session.visitor_data_bound = false;
        }
        session.cookie = cookie;
    }

    /// Get a reference to the current session (read-only).
    pub async fn session(&self) -> InnerTubeSession {
        self.session.read().await.clone()
    }

    /// Ensure we have visitor data, minting one if necessary.
    pub async fn ensure_visitor_data(&self, refresh: bool) -> Result<Option<String>> {
        let mut session = self.session.write().await;
        if !refresh && session.visitor_data.is_some() {
            return Ok(session.visitor_data.clone());
        }

        // Mint a new visitor id from the YouTube sw.js_data endpoint.
        match self.fetch_visitor_data().await {
            Ok(id) => {
                session.visitor_data = Some(id.clone());
                debug!("minted visitor id");
                Ok(Some(id))
            }
            Err(e) => {
                warn!("could not mint visitor id: {e}");
                Err(e)
            }
        }
    }

    async fn fetch_visitor_data(&self) -> Result<String> {
        let resp = self
            .client
            .get("https://www.youtube.com/sw.js_data")
            .header("User-Agent", WEB_USER_AGENT)
            .timeout(Duration::from_secs(10))
            .send()
            .await?;

        let text = resp.text().await?;
        // The response starts with '\n}\n' followed by the payload.
        let payload_start = text.find('\n').map(|p| p + 1).unwrap_or(0);
        let payload = text[payload_start..].trim();

        let payload: Value = serde_json::from_str(payload)?;
        let visitor_id = find_visitor_data(&payload);

        visitor_id.ok_or_else(|| {
            NetworkError::Parse(serde_json::from_str::<Value>("{}").unwrap_err())
        })
    }

    /// Browse a collection (albums, artists, playlists, etc.) by its browse ID.
    pub async fn browse(&self, browse_id: &str, params: Option<&str>) -> Result<Value> {
        self.post_music("browse", |body| {
            body.insert("browseId".to_string(), serde_json::Value::String(browse_id.to_string()));
            if let Some(p) = params {
                body.insert("params".to_string(), serde_json::Value::String(p.to_string()));
            }
        })
        .await
    }

    /// Get the next page of a browse continuation.
    pub async fn browse_continuation(&self, token: &str) -> Result<Value> {
        self.post_music("browse", |body| {
            body.insert("continuation".to_string(), serde_json::Value::String(token.to_string()));
        })
        .await
    }

    /// Search YouTube Music.
    pub async fn search(&self, query: &str, params: Option<&str>) -> Result<Value> {
        self.post_music("search", |body| {
            body.insert("query".to_string(), serde_json::Value::String(query.to_string()));
            if let Some(p) = params {
                body.insert("params".to_string(), serde_json::Value::String(p.to_string()));
            }
        })
        .await
    }

    /// Get the current account menu (for signed-in users).
    pub async fn account_menu(&self) -> Result<Value> {
        self.post_music("account/account_menu", |_body| {})
            .await
    }

    /// Get the player response for a video/track.
    pub async fn player(
        &self,
        video_id: &str,
        _signature_timestamp: Option<i64>,
    ) -> Result<Value> {
        let session = self.session.read().await;
        let client_version = session
            .scope
            .client_version
            .clone()
            .unwrap_or_else(|| "1.20250101.01.00".to_string());
        self.post_music("player", |body| {
            body.insert("videoId".to_string(), serde_json::Value::String(video_id.to_string()));
            body.insert("contentCheckOk".to_string(), serde_json::Value::Bool(true));
            body.insert("racyCheckOk".to_string(), serde_json::Value::Bool(true));

            let ctx = serde_json::json!({
                "client": {
                    "clientName": "WEB_REMIX",
                    "clientVersion": client_version,
                    "hl": "en",
                    "gl": "US"
                },
                "request": { "useSsl": true }
            });
            body.insert("context".to_string(), ctx);
        })
        .await
    }

    /// Get the next video for an existing one (radio/autoplay).
    pub async fn next(&self, video_id: &str) -> Result<Value> {
        self.post_music("next", |body| {
            body.insert("videoId".to_string(), serde_json::Value::String(video_id.to_string()));
            body.insert("playlistId".to_string(), serde_json::Value::String(format!("RDAMVM{video_id}")));
            body.insert("isAudioOnly".to_string(), serde_json::Value::Bool(true));
        })
        .await
    }

    /// Post an anonymous search (no account history).
    pub async fn search_unauthenticated(&self, query: &str) -> Result<Value> {
        self.post_music_anonymous("search", |body| {
            body.insert("query".to_string(), serde_json::Value::String(query.to_string()));
        })
        .await
    }

    /// Get search suggestions for typeahead.
    pub async fn search_suggestions(&self, input: &str) -> Result<Value> {
        self.post_music_anonymous("music/get_search_suggestions", |body| {
            body.insert("input".to_string(), serde_json::Value::String(input.to_string()));
        })
        .await
    }

    async fn post_music<F>(&self, endpoint: &str, body_builder: F) -> Result<Value>
    where
        F: FnOnce(&mut serde_json::Map<String, Value>),
    {
        let session = self.session.read().await.clone();
        let client_version = session
            .scope
            .client_version
            .clone()
            .unwrap_or_else(|| "1.20250101.01.00".to_string());

        let mut context = serde_json::json!({
            "client": {
                "clientName": "WEB_REMIX",
                "clientVersion": client_version,
                "hl": "en",
                "gl": "US"
            },
            "user": {
                "lockedSafetyMode": false
            },
            "request": { "useSsl": true }
        });

        if let Some(visitor) = &session.visitor_data {
            context
                .get_mut("client")
                .and_then(|c| c.as_object_mut())
                .map(|c| c.insert("visitorData".to_string(), serde_json::Value::String(visitor.clone())));
        }

        if let Some(_cookie) = &session.cookie {
            if let Some(data_sync_id) = &session.scope.data_sync_id {
                context
                    .get_mut("user")
                    .and_then(|u| u.as_object_mut())
                    .map(|u| u.insert("onBehalfOfUser".to_string(), serde_json::Value::String(data_sync_id.clone())));
            }
        }

        let mut body = serde_json::Map::new();
        body.insert("context".to_string(), context);

        // Allow builder to add endpoint-specific fields.
        body_builder(&mut body);

        let url = format!("{}/{}", MUSIC_BASE, endpoint);
        let response = self
            .client
            .post(&url)
            .header("User-Agent", WEB_USER_AGENT)
            .header("Accept-Language", "en-US,en;q=0.9")
            .header("X-Origin", MUSIC_ORIGIN)
            .header("Origin", MUSIC_ORIGIN)
            .header("Referer", format!("{}/", MUSIC_ORIGIN))
            .header("X-YouTube-Client-Name", "67")
            .header("X-YouTube-Client-Version", &client_version)
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(NetworkError::HttpStatus {
                status: response.status().as_u16(),
                url: url,
            });
        }

        let json = response.json::<Value>().await?;
        // Capture visitor data from response if present.
        if let Some(visitor) = extract_visitor_data(&json) {
            let mut session = self.session.write().await;
            if !session.visitor_data_bound {
                session.visitor_data = Some(visitor);
            }
        }

        Ok(json)
    }

    async fn post_music_anonymous<F>(&self, endpoint: &str, mut body_builder: F) -> Result<Value>
    where
        F: FnOnce(&mut serde_json::Map<String, Value>),
    {
        let session = self.session.read().await.clone();
        let client_version = session
            .scope
            .client_version
            .clone()
            .unwrap_or_else(|| "1.20250101.01.00".to_string());

        let mut context = serde_json::json!({
            "client": {
                "clientName": "WEB_REMIX",
                "clientVersion": client_version,
                "hl": "en",
                "gl": "US"
            },
            "user": {
                "lockedSafetyMode": false
            },
            "request": { "useSsl": true }
        });

        if let Some(visitor) = &session.visitor_data {
            context
                .get_mut("client")
                .and_then(|c| c.as_object_mut())
                .map(|c| c.insert("visitorData".to_string(), serde_json::Value::String(visitor.clone())));
        }

        let mut body = serde_json::Map::new();
        body.insert("context".to_string(), context);

        body_builder(&mut body);

        let url = format!("{}/{}", MUSIC_BASE, endpoint);
        let response = self
            .client
            .post(&url)
            .header("User-Agent", WEB_USER_AGENT)
            .header("X-Origin", MUSIC_ORIGIN)
            .header("Origin", MUSIC_ORIGIN)
            .header("Referer", format!("{}/", MUSIC_ORIGIN))
            .header("X-YouTube-Client-Name", "67")
            .header("X-YouTube-Client-Version", &client_version)
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(NetworkError::HttpStatus {
                status: response.status().as_u16(),
                url: url,
            });
        }

        Ok(response.json::<Value>()
            .await
            .map_err(|e| NetworkError::Parse(serde_json::Error::io(std::io::Error::other(e.to_string()))))?)
    }

    async fn session_read(&self) -> InnerTubeSession {
        self.session.read().await.clone()
    }
}

fn find_visitor_data(element: &Value) -> Option<String> {
    match element {
        Value::Array(arr) => arr.iter().find_map(find_visitor_data),
        Value::String(s) => {
            // Visitor data matches the pattern: Cg[A-Za-z0-9_%-]{40,}
            let re = regex::Regex::new(r"^Cg[A-Za-z0-9_%-]{40,}$").ok()?;
            if re.is_match(s) {
                Some(s.clone())
            } else {
                None
            }
        }
        _ => None,
    }
}

fn extract_visitor_data(json: &Value) -> Option<String> {
    json.get("responseContext")?
        .get("visitorData")?
        .as_str()
        .map(|s| s.to_string())
}