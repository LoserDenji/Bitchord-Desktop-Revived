//! Discord rich presence. Port of `web/extension/lib/discord.ts` (which ports
//! com.my.kizzy.gateway.DiscordWebSocket and DiscordRPC.updateSong).
//!
//! This is user-token automation, as on the phone and the desktop: there is no
//! API for a third-party app to set a user's presence, so the gateway is
//! opened with the account's own token, read from a discord.com page in the
//! agent webview only when the user clicks "Connect Discord". The token is
//! kept in the store and never sent to the app page.

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::tungstenite::Message;

use crate::store::Store;
use crate::types::{DiscordNowPlaying, DiscordPresenceOptions, DiscordState, DiscordUser};
use crate::yt::now_ms;

/// BitChord's own Discord application (DiscordApplication.kt).
pub const APPLICATION_ID: &str = "1543679166967193620";
const GATEWAY: &str = "wss://gateway.discord.gg/?v=9&encoding=json";
const API: &str = "https://discord.com/api/v9";
const PROJECT_URL: &str = "https://github.com/kushagrasinghx/BitChord";
const DEFAULT_BUTTON_1: &str = "Listen on YouTube Music";
const DEFAULT_BUTTON_2: &str = "Visit BitChord";
const APP_NAME: &str = "BitChord";

const INITIAL_RECONNECT_MS: u64 = 1_000;
const MAX_RECONNECT_MS: u64 = 60_000;
const SESSION_WAIT_MS: u64 = 20_000;
/// A cleared card with nothing new for this long: the socket goes too.
const IDLE_CLOSE_MS: u64 = 5 * 60_000;

const OP_DISPATCH: u8 = 0;
const OP_HEARTBEAT: u8 = 1;
const OP_IDENTIFY: u8 = 2;
const OP_PRESENCE_UPDATE: u8 = 3;
const OP_RESUME: u8 = 6;
const OP_RECONNECT: u8 = 7;
const OP_INVALID_SESSION: u8 = 9;
const OP_HELLO: u8 = 10;
const OP_HEARTBEAT_ACK: u8 = 11;

/// Commands into the gateway loop.
#[derive(Debug, Clone)]
pub enum Command {
    SetPresence(Value),
    ClearActivity,
}

/// Status the gateway task reports; mirrored into the service's shared state.
type StatusCallback = Arc<dyn Fn(&'static str, Option<String>) + Send + Sync>;

#[derive(Default)]
struct GatewayShared {
    sequence: u64,
    session_id: Option<String>,
    resume_url: Option<String>,
    heartbeat_interval: u64,
    /// Replayed onto every new session: Discord discards a presence with the session that set it.
    last_presence: Option<Value>,
    /// What this session already has, so the replay and a push don't both send it.
    delivered: Option<String>,
    ready: bool,
}

impl GatewayShared {
    fn forget_session(&mut self) {
        self.resume_url = None;
        self.session_id = None;
        self.sequence = 0;
    }
}

/// The gateway (DiscordWebSocket.kt): connect, pump, back off 1 s doubling to
/// 60 s, the heartbeat watchdog that drops a socket whose heartbeat went
/// unanswered, and the replay of the last presence onto a new session.
pub struct Gateway {
    shared: Arc<Mutex<GatewayShared>>,
    commands: mpsc::UnboundedSender<Command>,
    loop_handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl Gateway {
    pub fn spawn(token: String, status: StatusCallback) -> Gateway {
        let shared = Arc::new(Mutex::new(GatewayShared::default()));
        let (commands, command_rx) = mpsc::unbounded_channel();
        let handle = tokio::spawn(Self::loop_run(token, shared.clone(), command_rx, status));
        Gateway { shared, commands, loop_handle: Mutex::new(Some(handle)) }
    }

    pub async fn is_ready(&self) -> bool {
        self.shared.lock().await.ready
    }

    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }

    pub async fn close(&self) {
        // The loop owns no Close command path (it runs forever until the
        // handle is aborted): dropping the task closes the socket.
        if let Some(handle) = self.loop_handle.lock().await.take() {
            handle.abort();
        }
        *self.shared.lock().await = GatewayShared::default();
    }

    async fn loop_run(
        token: String,
        shared: Arc<Mutex<GatewayShared>>,
        mut commands: mpsc::UnboundedReceiver<Command>,
        status: StatusCallback,
    ) {
        let mut backoff = INITIAL_RECONNECT_MS;
        'outer: loop {
            status("connecting", None);
            let url = shared.lock().await.resume_url.clone().unwrap_or_else(|| GATEWAY.to_string());
            let (socket, _response) = match tokio_tungstenite::connect_async(&url).await {
                Ok(pair) => pair,
                Err(_) => {
                    status("connecting", None);
                    tokio::time::sleep(Duration::from_millis(backoff)).await;
                    backoff = (backoff * 2).min(MAX_RECONNECT_MS);
                    continue;
                }
            };
            backoff = INITIAL_RECONNECT_MS;
            let (mut write, mut read) = socket.split();
            let (tx, mut rx) = mpsc::unbounded_channel::<Message>();
            let writer = tokio::spawn(async move {
                while let Some(message) = rx.recv().await {
                    if write.send(message).await.is_err() {
                        break;
                    }
                }
            });

            let mut awaiting_ack = false;
            let mut last_ack_at = now_ms();
            let mut heartbeat_interval: u64 = 0;
            let mut opened = false;
            let mut immediate_retry = false;
            let mut refused = false;

            // Discord asks for the first heartbeat after a random fraction of the interval.
            let mut next_beat = tokio::time::Instant::now() + Duration::from_millis(1_000);

            loop {
                tokio::select! {
                    command = commands.recv() => {
                        match command {
                            None => break,
                            Some(Command::SetPresence(presence)) => {
                                let mut guard = shared.lock().await;
                                guard.last_presence = Some(presence.clone());
                                if guard.ready {
                                    let key = presence.to_string();
                                    if guard.delivered.as_deref() != Some(key.as_str()) {
                                        let frame = Message::text(json!({ "op": OP_PRESENCE_UPDATE, "d": presence }).to_string());
                                        if tx.send(frame).is_ok() {
                                            guard.delivered = Some(key);
                                        }
                                    }
                                }
                            }
                            Some(Command::ClearActivity) => {
                                let presence = json!({ "activities": [], "afk": true, "since": 0, "status": "online" });
                                let mut guard = shared.lock().await;
                                guard.last_presence = Some(presence.clone());
                                if guard.ready {
                                    let key = presence.to_string();
                                    let frame = Message::text(json!({ "op": OP_PRESENCE_UPDATE, "d": presence }).to_string());
                                    if tx.send(frame).is_ok() {
                                        guard.delivered = Some(key);
                                    }
                                }
                            }
                        }
                    }
                    frame = read.next() => {
                        match frame {
                            None | Some(Err(_)) => break,
                            Some(Ok(Message::Close(frame))) => {
                                // 4004: authentication failed. Retrying with the same token never helps.
                                if frame.as_ref().map(|f| u16::from(f.code)) == Some(4004) {
                                    refused = true;
                                }
                                break;
                            }
                            Some(Ok(Message::Text(text))) => {
                                let Ok(payload) = serde_json::from_str::<Value>(&text) else { continue };
                                if let Some(s) = payload.get("s").and_then(|s| s.as_u64()) {
                                    shared.lock().await.sequence = s;
                                }
                                let op = payload.get("op").and_then(|o| o.as_u64()).unwrap_or(u64::MAX) as u8;
                                match op {
                                    OP_DISPATCH => {
                                        let t = payload.get("t").and_then(|t| t.as_str()).unwrap_or_default();
                                        if t == "READY" {
                                            let d = payload.get("d").cloned().unwrap_or(Value::Null);
                                            let mut guard = shared.lock().await;
                                            guard.session_id = d.get("session_id").and_then(|s| s.as_str()).map(|s| s.to_string());
                                            guard.resume_url = d.get("resume_gateway_url").and_then(|r| r.as_str()).map(|r| format!("{r}/?v=9&encoding=json"));
                                            guard.ready = true;
                                            drop(guard);
                                            opened = true;
                                            awaiting_ack = false;
                                            last_ack_at = now_ms();
                                            status("ready", None);
                                            let replay = shared.lock().await.last_presence.clone();
                                            if let Some(presence) = replay {
                                                let key = presence.to_string();
                                                let frame = Message::text(json!({ "op": OP_PRESENCE_UPDATE, "d": presence }).to_string());
                                                if tx.send(frame).is_ok() {
                                                    shared.lock().await.delivered = Some(key);
                                                }
                                            }
                                        }
                                    }
                                    OP_HELLO => {
                                        heartbeat_interval = payload.get("d").and_then(|d| d.get("heartbeat_interval")).and_then(|i| i.as_u64()).unwrap_or(41_250);
                                        shared.lock().await.heartbeat_interval = heartbeat_interval;
                                        let guard = shared.lock().await;
                                        let (seq, session) = (guard.sequence, guard.session_id.clone());
                                        drop(guard);
                                        let out = if seq > 0 {
                                            json!({ "op": OP_RESUME, "d": { "token": token, "session_id": session, "seq": seq } })
                                        } else {
                                            json!({ "op": OP_IDENTIFY, "d": identify_payload(&token) })
                                        };
                                        let _ = tx.send(Message::text(out.to_string()));
                                        next_beat = tokio::time::Instant::now() + Duration::from_millis(heartbeat_interval * (rand_u64(1000, 2000) as u64 / 1000));
                                    }
                                    OP_HEARTBEAT => {
                                        let seq = shared.lock().await.sequence;
                                        let _ = tx.send(Message::text(json!({ "op": OP_HEARTBEAT, "d": if seq > 0 { Value::from(seq) } else { Value::Null } }).to_string()));
                                    }
                                    OP_HEARTBEAT_ACK => {
                                        awaiting_ack = false;
                                        last_ack_at = now_ms();
                                    }
                                    OP_RECONNECT => {
                                        immediate_retry = true;
                                        break;
                                    }
                                    OP_INVALID_SESSION => {
                                        shared.lock().await.forget_session();
                                        let _ = tx.send(Message::text(json!({ "op": OP_IDENTIFY, "d": identify_payload(&token) }).to_string()));
                                    }
                                    _ => {}
                                }
                            }
                            Some(Ok(_)) => {}
                        }
                    }
                    _ = tokio::time::sleep_until(next_beat) => {
                        // The watchdog: a heartbeat unanswered for a whole interval means the socket is gone.
                        if opened {
                            if awaiting_ack {
                                break;
                            }
                            awaiting_ack = true;
                            let seq = shared.lock().await.sequence;
                            let _ = tx.send(Message::text(json!({ "op": OP_HEARTBEAT, "d": if seq > 0 { Value::from(seq) } else { Value::Null } }).to_string()));
                        }
                        next_beat = tokio::time::Instant::now() + Duration::from_millis(if heartbeat_interval > 0 { heartbeat_interval } else { 41_250 });
                    }
                }
            }

            writer.abort();
            {
                let mut guard = shared.lock().await;
                guard.ready = false;
                guard.delivered = None;
                // An attempt that never opened is as often a stale resume URL as a dead network.
                if !opened {
                    guard.forget_session();
                }
            }
            if refused {
                status("error", Some("Discord refused the token. Connect the account again.".into()));
                return;
            }
            if immediate_retry {
                continue;
            }
            status("connecting", None);
            tokio::time::sleep(Duration::from_millis(backoff)).await;
            backoff = (backoff * 2).min(MAX_RECONNECT_MS);
            let _ = last_ack_at;
            let _ = awaiting_ack;
            continue 'outer;
        }
    }
}

fn rand_u64(min: u64, max: u64) -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.subsec_nanos() as u64).unwrap_or(0);
    min + nanos % (max - min).max(1)
}

/// Identify.toIdentifyPayload, with the web client's properties.
fn identify_payload(token: &str) -> Value {
    json!({
        "token": token,
        "capabilities": 16381,
        "properties": { "os": "Linux", "browser": "Chrome", "device": "" },
        "presence": { "status": "online", "since": 0, "activities": [], "afk": false },
        "compress": false,
        "client_state": {
            "guild_versions": {},
            "highest_last_message_id": "0",
            "read_state_version": 0,
            "user_guild_settings_version": -1,
            "user_settings_version": -1,
            "private_channels_version": "0",
            "api_code_version": 0,
        },
    })
}

fn activity_type_id(name: &str) -> u8 {
    match name {
        "playing" => 0,
        "watching" => 3,
        "competing" => 5,
        _ => 2, // listening
    }
}

fn resolve_variables(text: &str, track: &DiscordNowPlaying) -> String {
    text.replace("{song_name}", &track.title)
        .replace("{artist_name}", &track.artist)
        .replace("{album_name}", track.album.as_deref().unwrap_or(""))
}

fn status_text(status: &str) -> &'static str {
    match status {
        "idle" => "idle",
        "dnd" => "dnd",
        "invisible" => "invisible",
        _ => "online",
    }
}

// ---- The service ----------------------------------------------------------------

pub struct DiscordService {
    pub client: reqwest::Client,
    store: Arc<Store>,
    gateway: Mutex<Option<Arc<Gateway>>>,
    gateway_status: Arc<Mutex<&'static str>>,
    gateway_problem: Arc<Mutex<Option<String>>>,
    /// Holds the idle timer so a new activity can cancel it.
    idle_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// The sink every state change is announced on.
    sink: crate::bridge::EventSink,
    artwork_cache: Mutex<std::collections::HashMap<String, Option<String>>>,
}

impl DiscordService {
    pub fn new(store: Arc<Store>, sink: crate::bridge::EventSink) -> DiscordService {
        let client = reqwest::Client::builder()
            .user_agent(concat!(
                "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) ",
                "Chrome/138.0.0.0 Safari/537.36"
            ))
            .build()
            .expect("discord client");
        DiscordService {
            client,
            store,
            gateway: Mutex::new(None),
            gateway_status: Arc::new(Mutex::new("off")),
            gateway_problem: Arc::new(Mutex::new(None)),
            idle_task: Mutex::new(None),
            sink,
            artwork_cache: Mutex::new(std::collections::HashMap::new()),
        }
    }

    pub async fn state(&self) -> DiscordState {
        let (held, enabled) = self.store.read(|d| (d.discord_account.clone(), d.discord_enabled));
        DiscordState {
            available: true,
            connected: held.is_some(),
            enabled,
            user: held.map(|a| a.user),
            gateway: *self.gateway_status.lock().await,
            problem: self.gateway_problem.lock().await.clone(),
        }
    }

    async fn announce(&self) {
        let state = self.state().await;
        let _ = self.sink.send(crate::types::BridgeEvent::DiscordState(state));
    }

    /// ExternalAssets.kt: Discord's CDN mirror of an http image, as an `mp:` asset; cached (ArtworkCache).
    async fn external_asset(&self, token: &str, image_url: &str) -> Option<String> {
        if let Some(cached) = self.artwork_cache.lock().await.get(image_url).cloned() {
            return cached;
        }
        let result = async {
            let response = self
                .client
                .post(format!("{API}/applications/{APPLICATION_ID}/external-assets"))
                .header("Authorization", token)
                .json(&json!({ "urls": [image_url] }))
                .send()
                .await
                .ok()?;
            let parsed: Value = response.json().await.ok()?;
            let path = parsed.as_array()?.first()?.get("external_asset_path").and_then(|p| p.as_str())?.to_string();
            Some(format!("mp:{path}"))
        }
        .await;
        let mut cache = self.artwork_cache.lock().await;
        if cache.len() > 50 {
            cache.clear();
        }
        cache.insert(image_url.to_string(), result.clone());
        result
    }

    /// DiscordRPC.updateSong: timestamps with the rate divided out, the rate
    /// tag, buttons with {song_name} variables, external-asset artwork.
    pub async fn build_presence(&self, token: &str, track: &DiscordNowPlaying, options: &DiscordPresenceOptions) -> Value {
        let now = now_ms();
        let rate = if track.playback_rate > 0.0 { track.playback_rate } else { 1.0 };
        let watch_url = format!("https://music.youtube.com/watch?v={}", track.video_id);
        let mut buttons: Vec<(String, String)> = Vec::new();
        if options.button1_visible {
            let label = resolve_variables(
                if options.button1_text.trim().is_empty() { DEFAULT_BUTTON_1 } else { &options.button1_text },
                track,
            );
            buttons.push((label, watch_url.clone()));
        }
        if options.button2_visible {
            let label = resolve_variables(
                if options.button2_text.trim().is_empty() { DEFAULT_BUTTON_2 } else { &options.button2_text },
                track,
            );
            buttons.push((label, PROJECT_URL.to_string()));
        }
        let art = track.artwork_url.as_deref().filter(|u| u.starts_with("http"));
        // An unreachable mirror keeps the raw URL, as RpcImage.ExternalImage does.
        let large_image = match art {
            Some(url) => Some(self.external_asset(token, url).await.unwrap_or_else(|| url.to_string())),
            None => None,
        };
        let mut activity = json!({
            "name": if options.activity_name.trim().is_empty() { APP_NAME } else { options.activity_name.trim() },
            "details": if rate != 1.0 { format!("{} [{:.2}x]", track.title, rate) } else { track.title.clone() },
            "details_url": watch_url,
            "state": track.artist,
            "state_url": Value::Null,
            "type": activity_type_id(&options.activity_type),
            "status_display_type": if options.use_details { 2 } else { 1 },
            "timestamps": {
                "start": now - (track.position_ms as f64 / rate).round() as i64,
                "end": now + ((track.duration_ms - track.position_ms).max(0) as f64 / rate).round() as i64,
            },
            "assets": { "large_image": large_image, "small_image": Value::Null, "large_text": Value::Null, "small_text": Value::Null },
            "url": Value::Null,
        });
        if !buttons.is_empty() {
            let object = activity.as_object_mut().unwrap();
            object.insert("buttons".into(), Value::Array(buttons.iter().map(|(label, _)| Value::String(label.clone())).collect()));
            object.insert("metadata".into(), json!({ "button_urls": buttons.iter().map(|(_, url)| Value::String(url.clone())).collect::<Vec<_>>() }));
            object.insert("application_id".into(), Value::String(APPLICATION_ID.into()));
        }
        json!({
            "activities": [activity],
            "afk": true,
            "since": now,
            "status": status_text(&options.status),
        })
    }

    async fn status_callback(&self) -> StatusCallback {
        let status = self.gateway_status.clone();
        let problem = self.gateway_problem.clone();
        let sink = self.sink.clone();
        Arc::new(move |new_status: &'static str, new_problem: Option<String>| {
            let resolved = match (new_status, new_problem) {
                ("error", Some(p)) => Some(p),
                ("error", None) => problem.try_lock().ok().and_then(|g| g.clone()),
                _ => None,
            };
            *status.try_lock().unwrap() = new_status;
            *problem.try_lock().unwrap() = resolved.clone();
            let state = DiscordState {
                available: true,
                connected: false,
                enabled: false,
                user: None,
                gateway: new_status,
                problem: resolved,
            };
            let _ = sink.send(crate::types::BridgeEvent::DiscordState(state));
        })
    }

    fn ensure_gateway(&self, token: &str, status: StatusCallback) -> Arc<Gateway> {
        Arc::new(Gateway::spawn(token.to_string(), status))
    }

    /// The page reports what's playing; None takes the card down but leaves the socket up for a while.
    pub async fn set_presence(&self, track: Option<DiscordNowPlaying>, options: DiscordPresenceOptions) {
        let (held, enabled) = self.store.read(|d| (d.discord_account.clone(), d.discord_enabled));
        let Some(account) = held else { return };
        if !enabled {
            return;
        }
        // Cancel any pending idle close.
        if let Some(task) = self.idle_task.lock().await.take() {
            task.abort();
        }
        let Some(track) = track else {
            if let Some(gateway) = self.gateway.lock().await.clone() {
                gateway.send(Command::ClearActivity);
            }
            *self.idle_task.lock().await = Some(tokio::spawn(async {
                tokio::time::sleep(Duration::from_millis(IDLE_CLOSE_MS)).await;
            }));
            return;
        };
        let presence = self.build_presence(&account.token, &track, &options).await;
        let gateway = {
            let mut guard = self.gateway.lock().await;
            match guard.as_ref() {
                Some(existing) => existing.clone(),
                None => {
                    let status = self.status_callback().await;
                    let gateway = self.ensure_gateway(&account.token, status);
                    *guard = Some(gateway.clone());
                    gateway
                }
            }
        };
        gateway.send(Command::SetPresence(presence));
        // Wait a while for a session; a push that times out is replayed when one arrives.
        let deadline = tokio::time::Instant::now() + Duration::from_millis(SESSION_WAIT_MS);
        while tokio::time::Instant::now() < deadline {
            if gateway.is_ready().await {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        gateway.send(Command::SetPresence(self.build_presence(&account.token, &track, &options).await));
    }

    pub async fn set_enabled(&self, on: bool) -> DiscordState {
        self.store.with(|d| d.discord_enabled = on);
        self.store.save();
        if !on {
            self.close_gateway().await;
        }
        self.announce().await;
        self.state().await
    }

    pub async fn disconnect(&self) -> DiscordState {
        self.close_gateway().await;
        *self.gateway_status.lock().await = "off";
        *self.gateway_problem.lock().await = None;
        self.artwork_cache.lock().await.clear();
        self.store.with(|d| d.discord_account = None);
        self.store.save();
        self.announce().await;
        self.state().await
    }

    async fn close_gateway(&self) {
        if let Some(task) = self.idle_task.lock().await.take() {
            task.abort();
        }
        if let Some(gateway) = self.gateway.lock().await.take() {
            gateway.close().await;
        }
    }

    /// KizzyRPC.getUserInfo.
    pub async fn user_info(&self, token: &str) -> Result<DiscordUser, String> {
        let response = self
            .client
            .get(format!("{API}/users/@me"))
            .header("Authorization", token)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if response.status().as_u16() == 401 {
            return Err("Discord refused the token".into());
        }
        let me: Value = response.json().await.map_err(|e| e.to_string())?;
        let id = me.get("id").and_then(|v| v.as_str()).unwrap_or_default().to_string();
        let username = me.get("username").and_then(|v| v.as_str()).unwrap_or_default().to_string();
        let name = me
            .get("global_name")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(&username)
            .to_string();
        let avatar_url = me
            .get("avatar")
            .and_then(|v| v.as_str())
            .map(|avatar| format!("https://cdn.discordapp.com/avatars/{id}/{avatar}.png"));
        Ok(DiscordUser { name, username, avatar_url })
    }
}
