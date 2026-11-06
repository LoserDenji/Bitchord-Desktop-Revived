//! The bridge dispatcher — the port of `web/extension/background.ts`
//! `handle()`. Every method the page speaks is served here, identically on
//! the wire, so the page's own `detectExtension` finds this build and every
//! feature it gates on capabilities turns on.

use std::sync::Arc;

use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedSender;

use crate::discord::DiscordService;
use crate::http;
use crate::spotify::SpotifyService;
use crate::store::Store;
use crate::types::*;
use crate::yt::YtService;

/// Where the core pushes `BridgeEvent`s (the extension's `broadcast`).
pub type EventSink = UnboundedSender<BridgeEvent>;

pub struct Core {
    pub store: Arc<Store>,
    pub yt: YtService,
    pub spotify: SpotifyService,
    pub discord: DiscordService,
    anon: http::SharedClient,
    /// The page's own fetches (the extension's DNR rules, and the CORS fallback).
    page: reqwest::Client,
    event_tx: EventSink,
}

impl Core {
    /// Builds the services and hands the event stream back: the shell pumps
    /// it into the page (the extension's `broadcast`).
    pub fn new(data_dir: &std::path::Path) -> (Core, tokio::sync::mpsc::UnboundedReceiver<BridgeEvent>) {
        let store = Arc::new(Store::load(data_dir));
        let (event_tx, events) = tokio::sync::mpsc::unbounded_channel();
        let yt = YtService::new(store.clone());
        let spotify = SpotifyService::new(store.clone());
        let discord = DiscordService::new(store.clone(), event_tx.clone());
        let core = Core {
            yt,
            spotify,
            discord,
            anon: Arc::new(http::anon_client()),
            page: http::page_client(),
            event_tx,
            store,
        };
        (core, events)
    }

    pub fn sink(&self) -> EventSink {
        self.event_tx.clone()
    }

    /// The page's own fetch to YouTube (the extension's DNR rules), or — with
    /// `fallback` — any https host that answered its fetch with no CORS at
    /// all. Lives on the core because the headers the rules set are exactly
    /// the ones a page cannot set itself.
    pub async fn page_fetch(&self, request: &HttpRequest, fallback: bool) -> Result<HttpResponse, (String, String)> {
        http::page_fetch(&self.page, request, fallback).await
    }

    /// The lyric providers the page can't reach itself; this build serves them
    /// all, so every requested origin is granted.
    pub fn granted_origins(origins: &[String]) -> Vec<String> {
        origins.to_vec()
    }

    /// Handles one bridge request, returning the response envelope. Never panics
    /// into the page: every failure is a BridgeError.
    pub async fn call(&self, request: BridgeRequest) -> Value {
        if request.protocol != BRIDGE_PROTOCOL {
            return error_response(
                &request.id,
                "protocol",
                &format!(
                    "The page speaks bridge protocol {}; this build speaks {BRIDGE_PROTOCOL}. Update whichever is older.",
                    request.protocol
                ),
            );
        }
        match self.dispatch(&request).await {
            Ok(result) => ok_response(&request.id, result),
            Err((message, code)) => error_response(&request.id, &code, &message),
        }
    }

    async fn dispatch(&self, request: &BridgeRequest) -> BridgeResult {
        match request.method.as_str() {
            "ping" => Ok(json!({
                "version": env!("CARGO_PKG_VERSION"),
                "protocol": BRIDGE_PROTOCOL,
                "browser": "chromium",
                "capabilities": CAPABILITIES,
            })),
            "yt.fetch" => {
                let Some(params) = request.params.as_object() else {
                    return Err(("Bad params".into(), "internal".into()));
                };
                let Some(http_request) = params.get("url").cloned().map(|_| ()) else {
                    return Err(("Bad params".into(), "internal".into()));
                };
                let _ = http_request;
                let wire = serde_json::json!(request.params);
                let Some(request_http) = HttpRequest::from_wire(wire) else {
                    return Err(("Bad params".into(), "internal".into()));
                };
                let auth = params.get("auth").and_then(|a| a.as_bool()).unwrap_or(false);
                if auth {
                    let response = self.yt.yt_fetch_signed(&request_http).await?;
                    Ok(serde_json::to_value(response).expect("response"))
                } else {
                    let response = http::yt_fetch_anon(&self.anon, &request_http).await?;
                    Ok(serde_json::to_value(response).expect("response"))
                }
            }
            "yt.authState" => {
                let refresh = request
                    .params_as::<serde_json::Map<String, Value>>()
                    .ok()
                    .and_then(|p| p.get("refresh").and_then(|r| r.as_bool()))
                    .unwrap_or(false);
                let state = self.yt.auth_state(refresh).await;
                Ok(serde_json::to_value(state).expect("state"))
            }
            "yt.selectChannel" => {
                let channel_id = request
                    .params_as::<serde_json::Map<String, Value>>()
                    .ok()
                    .and_then(|p| p.get("channelId").and_then(|c| c.as_str()).map(|s| s.to_string()))
                    .ok_or_else(|| ("Bad params".into(), "internal".into()))?;
                let state = self.yt.select_channel(&channel_id).await.map_err(|m| (m, "internal".to_string()))?;
                let _ = self.event_tx.send(BridgeEvent::YtAuthState(state.clone()));
                Ok(serde_json::to_value(state).expect("state"))
            }
            "yt.setConnected" => {
                let connected = request
                    .params_as::<serde_json::Map<String, Value>>()
                    .ok()
                    .and_then(|p| p.get("connected").and_then(|c| c.as_bool()))
                    .unwrap_or(false);
                let state = self.yt.set_connected(connected).await;
                let _ = self.event_tx.send(BridgeEvent::YtAuthState(state.clone()));
                Ok(serde_json::to_value(state).expect("state"))
            }
            "addon.fetch" => {
                let Some(request_http) = HttpRequest::from_wire(serde_json::json!(request.params)) else {
                    return Err(("Bad params".into(), "internal".into()));
                };
                let response = http::addon_fetch(&self.anon, &request_http).await?;
                Ok(serde_json::to_value(response).expect("response"))
            }
            "lyrics.fetch" => {
                let Some(request_http) = HttpRequest::from_wire(serde_json::json!(request.params)) else {
                    return Err(("Bad params".into(), "internal".into()));
                };
                let granted: Vec<String> = url_origins(&request_http.url);
                let response = http::lyrics_fetch(&self.anon, &request_http, &granted).await?;
                Ok(serde_json::to_value(response).expect("response"))
            }
            "lyrics.permissions" => {
                let origins = request
                    .params
                    .get("origins")
                    .and_then(|o| o.as_array())
                    .map(|list| list.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect::<Vec<_>>())
                    .unwrap_or_default();
                Ok(json!({ "granted": Self::granted_origins(&origins) }))
            }
            "lyrics.requestPermissions" => Ok(json!({})),
            "spotify.authState" => {
                let refresh = request
                    .params_as::<serde_json::Map<String, Value>>()
                    .ok()
                    .and_then(|p| p.get("refresh").and_then(|r| r.as_bool()))
                    .unwrap_or(false);
                if refresh {
                    self.spotify.invalidate_session();
                }
                let state = self.spotify.auth_state().await;
                Ok(serde_json::to_value(state).expect("state"))
            }
            "spotify.setConnected" => {
                let connected = request
                    .params_as::<serde_json::Map<String, Value>>()
                    .ok()
                    .and_then(|p| p.get("connected").and_then(|c| c.as_bool()))
                    .unwrap_or(false);
                let state = self.spotify.set_connected(connected).await;
                let _ = self.event_tx.send(BridgeEvent::SpotifyAuthState(state.clone()));
                Ok(serde_json::to_value(state).expect("state"))
            }
            "spotify.signIn" => Ok(json!({})), // the desktop sign-in window is opened by the shell command
            "spotify.playlists" => {
                let offset = request
                    .params_as::<serde_json::Map<String, Value>>()
                    .ok()
                    .and_then(|p| p.get("offset").and_then(|o| o.as_i64()))
                    .unwrap_or(0);
                let page = self.spotify.playlists(offset).await?;
                Ok(serde_json::to_value(page).expect("page"))
            }
            "spotify.tracks" => {
                let params = request.params_as::<serde_json::Map<String, Value>>().map_err(|m| (m, "internal".into()))?;
                let playlist_id = params.get("playlistId").and_then(|p| p.as_str()).ok_or(("Bad params".into(), "internal".into()))?.to_string();
                let offset = params.get("offset").and_then(|o| o.as_i64()).unwrap_or(0);
                let page = self.spotify.tracks(&playlist_id, offset).await?;
                Ok(serde_json::to_value(page).expect("page"))
            }
            "spotify.playlist" => {
                let playlist_id = request
                    .params_as::<serde_json::Map<String, Value>>()
                    .ok()
                    .and_then(|p| p.get("playlistId").and_then(|c| c.as_str()).map(|s| s.to_string()))
                    .ok_or_else(|| ("Bad params".into(), "internal".into()))?;
                let (name, owner, image_url, total) = self.spotify.playlist_info(&playlist_id).await?;
                Ok(json!({ "name": name, "owner": owner, "imageUrl": image_url, "total": total }))
            }
            "spotify.searchTracks" => {
                let query = request
                    .params_as::<serde_json::Map<String, Value>>()
                    .ok()
                    .and_then(|p| p.get("query").and_then(|q| q.as_str()).map(|s| s.to_string()))
                    .ok_or_else(|| ("Bad params".into(), "internal".into()))?;
                let tracks = self.spotify.search_tracks(&query).await?;
                Ok(serde_json::to_value(tracks).expect("tracks"))
            }
            "spotify.canvas" => {
                let track_id = request
                    .params_as::<serde_json::Map<String, Value>>()
                    .ok()
                    .and_then(|p| p.get("trackId").and_then(|c| c.as_str()).map(|s| s.to_string()))
                    .ok_or_else(|| ("Bad params".into(), "internal".into()))?;
                let url = self.spotify.canvas(&track_id).await?;
                Ok(json!({ "url": url }))
            }
            "discord.state" => {
                let state = self.discord.state().await;
                Ok(serde_json::to_value(state).expect("state"))
            }
            "discord.connect" => Ok(json!({})), // the agent window is opened by the shell command
            "discord.disconnect" => {
                let state = self.discord.disconnect().await;
                Ok(serde_json::to_value(state).expect("state"))
            }
            "discord.setEnabled" => {
                let enabled = request
                    .params_as::<serde_json::Map<String, Value>>()
                    .ok()
                    .and_then(|p| p.get("enabled").and_then(|c| c.as_bool()))
                    .unwrap_or(false);
                let state = self.discord.set_enabled(enabled).await;
                Ok(serde_json::to_value(state).expect("state"))
            }
            "discord.presence" => {
                let params = request.params_as::<DiscordPresenceParams>().map_err(|m| (m, "internal".into()))?;
                self.discord.set_presence(params.track, params.options).await;
                Ok(json!({}))
            }
            // The Dynamic Island's preference. The store is the single truth
            // (None = the shipping default, on); the shell's window lives or
            // dies by what lands here, which commands.rs applies once the
            // response is on its way back to the page.
            "island.state" => {
                let enabled = self.store.read(|d| d.island_enabled) != Some(false);
                Ok(json!({ "enabled": enabled }))
            }
            "island.setEnabled" => {
                let enabled = request
                    .params_as::<serde_json::Map<String, Value>>()
                    .ok()
                    .and_then(|p| p.get("enabled").and_then(|e| e.as_bool()))
                    .ok_or_else(|| ("Bad params".into(), "internal".into()))?;
                self.store.with(|d| d.island_enabled = Some(enabled));
                self.store.save();
                Ok(json!({ "enabled": enabled }))
            }
            other => Err((
                format!("Unknown bridge method {other}"),
                "unsupported".into(),
            )),
        }
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct DiscordPresenceParams {
    track: Option<DiscordNowPlaying>,
    options: DiscordPresenceOptions,
}

/// The origins a URL may be served from, for lyrics permissions (all granted here).
fn url_origins(url: &str) -> Vec<String> {
    url::Url::parse(url)
        .map(|u| vec![u.origin().ascii_serialization()])
        .unwrap_or_default()
}
