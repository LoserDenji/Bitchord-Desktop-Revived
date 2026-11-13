//! Listen Together, the client half: membership, the REST calls, the socket, the
//! clock, and the controls any member may send — a port of ForkChord's
//! `party/listenTogether.ts` + `models.ts` + `clock.ts` + `invite.ts` (which port
//! the phone's ListenTogether.kt). It connects to the same public party server the
//! BitChord web app and the phone use, so the desktop joins their parties.
//!
//! It doesn't touch the player: what it publishes is the party's playhead
//! (`position_now`), and `ui/party.rs` binds that to playback, the way
//! `partySync.ts` does. Nothing acts on "play now" directly: the server holds a
//! position and the server time it was true at, the clock measures this device's
//! offset, and the playhead is arithmetic. Sequence numbers drop stale states, the
//! server re-states the truth every few seconds, and the socket loop assumes the
//! socket will die.
//!
//! Unlike the phone, an app restart doesn't end the party from the server's side:
//! it holds a member's slot for a grace period, so a quick relaunch picks the
//! party back up by joining again.
//!
//! The socket and the HTTP calls live on the backend's tokio runtime; the UI talks
//! to them through channels and is woken when something arrives (the pattern
//! `jam.rs` uses).

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crossbeam_channel::{Receiver, Sender, unbounded};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

/// The party server the web app and the phone use. `YTM_PARTY_SERVER` overrides it,
/// and a custom server typed on the page wins over both.
pub const DEFAULT_SERVER: &str = "https://bitchord.kushagrasingh.in";

/// Party invite links (JamInviteLink.kt): `https://bitchord.kushagrasingh.in/invite/<CODE>`,
/// optionally `?server=<custom server>`, and `bitchord://party/<CODE>`.
pub const INVITE_ORIGIN: &str = "https://bitchord.kushagrasingh.in";

pub const CODE_LEN: usize = 6;
const PING_INTERVAL_MS: i64 = 15_000;
const REPORT_INTERVAL_MS: i64 = 10_000;
const PING_BURST: u32 = 4;
const PING_BURST_GAP_MS: i64 = 300;
/// Failed connects in a row before asking the server whether the membership still
/// exists. A party the server has forgotten is left rather than redialled forever.
const CHECK_AFTER_FAILURES: u32 = 3;
const MAX_BACKOFF_MS: u64 = 30_000;

// ---- The wire format, one-for-one (PartyModels.kt) --------------------------------
//
// The server speaks camelCase; its README is the contract: rename a field on both
// sides, never paper over it here. Every field defaults, so an older server's
// frames fill with empties rather than failing to read.

fn maybe<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(deserializer).unwrap_or(None))
}

/// A song as the party knows it: enough to name and show it. How each device plays
/// it is its own business.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PartyTrack {
    pub video_id: String,
    pub title: String,
    pub artist: String,
    #[serde(deserialize_with = "maybe")]
    pub thumbnail_url: Option<String>,
    #[serde(deserialize_with = "maybe")]
    pub duration_ms: Option<i64>,
    /// Preserves the queue's shared manual/AutoPlay boundary.
    pub from_autoplay: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PartyMember {
    pub member_id: String,
    pub user_id: String,
    pub display_name: String,
    #[serde(deserialize_with = "maybe")]
    pub avatar_url: Option<String>,
    pub is_host: bool,
    /// Holding a socket right now, not whether they are still in.
    pub connected: bool,
    pub joined_at_ms: i64,
    pub last_seen_ms: i64,
}

/// Where the party is, as of a server timestamp: `position_ms` at `anchor_ms`. It
/// becomes a current position only once the reader adds the time since.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PartyPlayback {
    /// Only climbs; a state not newer than the one applied is dropped.
    pub seq: i64,
    pub track: Option<PartyTrack>,
    pub queue_seq: i64,
    pub queue_length: usize,
    pub queue_index: i64,
    pub is_playing: bool,
    pub position_ms: i64,
    pub anchor_ms: i64,
    /// The server's own reading at the instant it sent the frame.
    pub effective_position_ms: i64,
    pub updated_by: Option<String>,
    pub started_by: Option<String>,
    pub started_by_name: Option<String>,
    pub autoplay_enabled: bool,
    pub updated_at_ms: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PartyQueue {
    pub seq: i64,
    pub index: i64,
    pub items: Vec<PartyTrack>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PartySnapshot {
    pub code: String,
    pub created_at_ms: i64,
    pub max_members: usize,
    pub host_only_control: bool,
    pub members: Vec<PartyMember>,
    pub playback: PartyPlayback,
    pub queue: PartyQueue,
    pub server_ms: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PartyPreview {
    pub code: String,
    pub host_name: String,
    pub member_count: usize,
    pub max_members: usize,
    pub is_full: bool,
    pub members: Vec<PartyMember>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PartyMembership {
    pub code: String,
    pub token: String,
    pub you: PartyMember,
    pub party: PartySnapshot,
    pub server_ms: i64,
}

/// A compact local-only record of a live party action.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PartyActivity {
    pub action: String,
    pub by: String,
    pub at_ms: i64,
    pub detail: String,
}

/// Who this device jams as: the id is a hash of the login (or the device), never
/// the login itself.
#[derive(Clone, Debug, PartialEq)]
pub struct JoinRequest {
    pub user_id: String,
    pub device_id: String,
    pub display_name: String,
    pub avatar_url: Option<String>,
    pub max_members: Option<usize>,
    pub autoplay_enabled: Option<bool>,
}

impl JoinRequest {
    fn to_json(&self) -> Value {
        json!({
            "userId": self.user_id,
            "deviceId": self.device_id,
            "displayName": self.display_name,
            "avatarUrl": self.avatar_url,
            "maxMembers": self.max_members,
            "autoplayEnabled": self.autoplay_enabled,
        })
    }
}

// ---- Codes, servers, invites -------------------------------------------------------

/// `code` cleaned up (letters and digits, upper case) unless it isn't six of them.
pub fn clean_code(raw: &str) -> Option<String> {
    let cleaned: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect();
    (cleaned.len() == CODE_LEN).then_some(cleaned)
}

/// A base URL trimmed to a usable http(s) origin; none when it isn't one.
pub fn sanitize_server_url(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let with_scheme = if trimmed.to_ascii_lowercase().starts_with("http://")
        || trimmed.to_ascii_lowercase().starts_with("https://")
    {
        trimmed.to_owned()
    } else {
        format!("https://{trimmed}")
    };
    let url = url::Url::parse(&with_scheme).ok()?;
    (!url.host_str().unwrap_or_default().is_empty()).then(|| with_scheme)
}

/// The server parties are made on: the custom one if set, else BitChord's.
pub fn idle_server(custom: &str) -> String {
    sanitize_server_url(custom).unwrap_or_else(|| DEFAULT_SERVER.to_owned())
}

/// An invite link, a `bitchord://party/` link, or a bare code: the party it names,
/// and the custom server it names, if any.
pub fn parse_invite(value: &str) -> Option<(String, Option<String>)> {
    let value = value.trim().trim_matches('"');
    let (scheme, rest) = value.split_once("://")?;
    let rest = rest.split('#').next().unwrap_or("");
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let server = sanitize_server_url(&query_param(query, "server").unwrap_or_default());
    if scheme.eq_ignore_ascii_case("bitchord") {
        // `party/CODE` — everything after `bitchord://`.
        let (host, code) = path.split_once('/').unwrap_or((path, ""));
        if !host.eq_ignore_ascii_case("party") {
            return None;
        }
        // The param is `code`; the jam links' `c` is accepted too, so a mixed-up link still lands.
        let named = || {
            query_param(query, "code")
                .or_else(|| query_param(query, "c"))
                .unwrap_or_default()
        };
        let code = clean_code(if code.is_empty() { &named() } else { code })?;
        return Some((code, server));
    }
    if scheme.eq_ignore_ascii_case("https") || scheme.eq_ignore_ascii_case("http") {
        // `/invite/<CODE>` on any origin that names one.
        let path = path.trim_end_matches('/');
        let (before, last) = path.rsplit_once('/')?;
        if before.ends_with("/invite")
            && let Some(code) = clean_code(last)
        {
            return Some((code, server));
        }
        return None;
    }
    None
}

/// The link to share (JamInviteLink.url): a custom server's own invite page, else
/// the public site's.
pub fn invite_url(code: &str, custom_server: Option<&str>) -> String {
    let origin = match custom_server.map(sanitize_server_url).flatten() {
        Some(base) if !base.eq_ignore_ascii_case(INVITE_ORIGIN) => base,
        _ => INVITE_ORIGIN.to_owned(),
    };
    format!("{origin}/invite/{}", code.to_ascii_uppercase())
}

fn query_param<'a>(query: &'a str, name: &str) -> Option<&'a str> {
    query.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        (k == name).then_some(v)
    })
}

// ---- The server clock (ServerClock.kt) ----------------------------------------------

/// This device's offset from the party server's clock: NTP cut down to one
/// WebSocket — offset = serverMs − (t0 + t1) / 2, and the sample with the smallest
/// round trip wins (its error is at most half that trip), out of the last
/// [`WINDOW`] taken within [`SAMPLE_TTL_MS`].
const WINDOW: usize = 12;
const SAMPLE_TTL_MS: i64 = 120_000;

#[derive(Clone, Copy)]
struct Sample {
    offset_ms: i64,
    round_trip_ms: i64,
    taken_at_ms: i64,
}

/// The clock's best reading, kept where both the socket and the UI can read it.
#[derive(Default)]
pub struct ServerClock {
    samples: std::sync::Mutex<Vec<Sample>>,
    /// Best offset so far (ms, server minus here), 0 until the first pong.
    offset: AtomicI64,
    round_trip: AtomicI64,
}

impl ServerClock {
    fn record(&self, sent_at_local_ms: i64, server_ms: i64, received_at_local_ms: i64) {
        let round_trip = (received_at_local_ms - sent_at_local_ms).max(0);
        let midpoint = sent_at_local_ms + round_trip / 2;
        let sample = Sample {
            offset_ms: server_ms - midpoint,
            round_trip_ms: round_trip,
            taken_at_ms: received_at_local_ms,
        };
        let best = self
            .samples
            .lock()
            .map(|mut all| {
                all.push(sample);
                while all.len() > WINDOW {
                    all.remove(0);
                }
                let cutoff = received_at_local_ms - SAMPLE_TTL_MS;
                let fresh: Vec<&Sample> = all.iter().filter(|s| s.taken_at_ms >= cutoff).collect();
                let usable: Vec<&Sample> = if fresh.is_empty() {
                    all.iter().collect()
                } else {
                    fresh
                };
                usable.into_iter().min_by_key(|s| s.round_trip_ms).copied()
            })
            .unwrap_or(None);
        if let Some(best) = best {
            self.offset.store(best.offset_ms, Ordering::Relaxed);
            self.round_trip.store(best.round_trip_ms, Ordering::Relaxed);
        }
    }

    /// The best offset and round trip so far, or none until the first pong.
    pub fn reading(&self) -> Option<(i64, i64)> {
        let offset = self.offset.load(Ordering::Relaxed);
        (offset != 0).then(|| (offset, self.round_trip.load(Ordering::Relaxed)))
    }

    fn reset(&self) {
        self.offset.store(0, Ordering::Relaxed);
        self.round_trip.store(0, Ordering::Relaxed);
        if let Ok(mut all) = self.samples.lock() {
            all.clear();
        }
    }
}

/// `performance.timeOrigin + performance.now()`'s stand-in: the wall clock in ms.
/// A stepped wall clock lands in the offset (the browser's monotonic reading is
/// better); party positions drift by exactly the step until the next pong.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ---- Where a party is, read from here ----------------------------------------------

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Connection {
    #[default]
    Offline,
    Connecting,
    Live,
}

/// The party's state as the UI sees it. The client keeps the newest truth the
/// server has stated; the playhead is read on demand.
#[derive(Clone, Debug, Default)]
pub struct State {
    pub code: Option<String>,
    pub you: Option<PartyMember>,
    pub members: Vec<PartyMember>,
    pub max_members: usize,
    pub host_only_control: bool,
    pub playback: PartyPlayback,
    pub queue: PartyQueue,
    pub connection: Connection,
    /// False until the first round trip; the playhead is a guess until then.
    pub clock_synced: bool,
    pub round_trip_ms: i64,
    pub error: Option<String>,
    /// The last 100 actions this session, newest first.
    pub activity: Vec<PartyActivity>,
    /// A resume this device accepted but may not perform yet (the sync owns it).
    pub awaiting_start: bool,
}

impl State {
    /// Only this device's own, empty state: leaving keeps the activity log.
    fn idle(activity: Vec<PartyActivity>) -> Self {
        Self {
            activity,
            ..Default::default()
        }
    }
}

/// True for a listener in a party whose host has taken control of it.
pub fn controls_locked(state: &State) -> bool {
    state.code.is_some() && state.host_only_control && state.you.as_ref().is_none_or(|y| !y.is_host)
}

/// Fills what an older server leaves out, as kotlinx's defaults do: a decoded frame
/// with missing fields reads as an empty one rather than failing.
pub fn playback_of(raw: &Value) -> PartyPlayback {
    serde_json::from_value(raw.clone()).unwrap_or_default()
}

pub fn queue_of(raw: &Value) -> PartyQueue {
    serde_json::from_value(raw.clone()).unwrap_or_default()
}

pub fn snapshot_of(raw: &Value) -> PartySnapshot {
    serde_json::from_value(raw.clone()).unwrap_or_default()
}

pub fn member_of(raw: &Value) -> PartyMember {
    serde_json::from_value(raw.clone()).unwrap_or_default()
}

/// Where this device should be in the current track, right now, in ms; none without
/// a track. Before the first pong it's the server's own reading at send time: a
/// starting point, not a thing to seek to (wait for `clock_synced`).
pub fn position_now(state: &State, offset_ms: i64) -> Option<i64> {
    let track = state.playback.track.as_ref()?;
    if !state.playback.is_playing {
        return Some(state.playback.position_ms);
    }
    let server_now = now_ms() + offset_ms;
    let position = state.playback.position_ms + (server_now - state.playback.anchor_ms).max(0);
    Some(if track.duration_ms.unwrap_or(0) > 0 {
        position.min(track.duration_ms.unwrap())
    } else {
        position
    })
}

/// How far ahead the party's next resume is scheduled (the play lead), or 0 if it's
/// under way.
pub fn ms_until_start(state: &State, offset_ms: i64) -> i64 {
    if !state.playback.is_playing {
        return 0;
    }
    (state.playback.anchor_ms - (now_ms() + offset_ms)).max(0)
}

/// Where `video_id` is in the party's queue (PartyQueueReading.kt): a party queue
/// keeps its history, so the playing track can be in it twice. The state frame's
/// own index wins when it names the track; otherwise the nearest copy, looking
/// forward first. Never "the first from the top": that played the wrong part of
/// the queue.
pub fn party_queue_index_of(
    queue: &PartyQueue,
    playback: &PartyPlayback,
    video_id: Option<&str>,
) -> i64 {
    let Some(video_id) = video_id else { return -1 };
    let hint = if playback.queue_seq == queue.seq {
        playback.queue_index
    } else {
        queue.index
    };
    if hint >= 0
        && (hint as usize) < queue.items.len()
        && queue.items[hint as usize].video_id == video_id
    {
        return hint;
    }
    let mut best: i64 = -1;
    let mut best_distance = i64::MAX;
    for (i, item) in queue.items.iter().enumerate() {
        if item.video_id != video_id {
            continue;
        }
        let i = i as i64;
        let distance = if i >= hint {
            i - hint
        } else {
            (hint - i) * 2 + 1
        };
        if distance < best_distance {
            best = i;
            best_distance = distance;
        }
    }
    best
}

/// What the party plays after `video_id`, or nothing when the queue doesn't hold it.
pub fn party_upcoming_after<'a>(
    queue: &'a PartyQueue,
    playback: &PartyPlayback,
    video_id: Option<&str>,
) -> &'a [PartyTrack] {
    let at = party_queue_index_of(queue, playback, video_id);
    if at < 0 {
        &[]
    } else {
        &queue.items[(at as usize + 1).min(queue.items.len())..]
    }
}

/// Whether `after` is `before` with exactly one item moved (PartySync.detectSingleMove).
pub fn detect_single_move(before: &[String], after: &[String]) -> Option<(usize, usize, String)> {
    if before.len() != after.len() || before.is_empty() || before == after {
        return None;
    }
    let counts = |list: &[String]| -> std::collections::BTreeMap<String, usize> {
        list.iter().fold(
            std::collections::BTreeMap::new(),
            |mut m: std::collections::BTreeMap<String, usize>, s| {
                *m.entry(s.clone()).or_default() += 1;
                m
            },
        )
    };
    if counts(before) != counts(after) {
        return None;
    }
    for from in 0..before.len() {
        let mut without = before.to_vec();
        let item = without.remove(from);
        for to in 0..before.len() {
            if from == to {
                continue;
            }
            let mut simulated = without.clone();
            simulated.insert(to, item.clone());
            if simulated == after {
                return Some((from, to, item));
            }
        }
    }
    None
}

/// The one member who tops up AutoPlay (Autoplay.autoplaySupplierId): the host while
/// connected, else the lowest connected member id; nobody stands in for a host who
/// has locked the party.
pub fn autoplay_supplier_id(members: &[PartyMember], host_only_control: bool) -> Option<String> {
    let host = members
        .iter()
        .find(|m| m.is_host && m.connected)
        .map(|m| m.member_id.clone());
    if host_only_control {
        return host;
    }
    host.or_else(|| {
        let mut connected: Vec<String> = members
            .iter()
            .filter(|m| m.connected)
            .map(|m| m.member_id.clone())
            .collect();
        connected.sort();
        connected.into_iter().next()
    })
}

// ---- Kicks: a door that stays shut on this device for a day ---------------------------

const KICKS_FILE: &str = "party-kicks.json";
const KICK_BLOCK_MS: i64 = 24 * 60 * 60 * 1000;

/// The parties this device was removed from, with the until-times still ahead.
fn recent_kicks(data_dir: &Path) -> HashMap<String, i64> {
    let Ok(bytes) = std::fs::read(data_dir.join(KICKS_FILE)) else {
        return HashMap::new();
    };
    let all: HashMap<String, i64> = serde_json::from_slice(&bytes).unwrap_or_default();
    let now = now_ms();
    all.into_iter().filter(|(_, until)| *until > now).collect()
}

fn record_kick(data_dir: &Path, code: &str) {
    let mut kicks = recent_kicks(data_dir);
    kicks.insert(code.to_owned(), now_ms() + KICK_BLOCK_MS);
    let _ = std::fs::write(
        data_dir.join(KICKS_FILE),
        serde_json::to_vec(&kicks).unwrap_or_default(),
    );
}

/// The message to show when a recently-kicked device tries to rejoin.
pub fn kicked_message(data_dir: &Path, code: &str) -> Option<String> {
    recent_kicks(data_dir)
        .contains_key(code)
        .then(|| "You were removed from this party. You can rejoin tomorrow.".to_owned())
}

/// Who this device jams as, when the account doesn't name it: a stable per-device id.
pub fn device_id(data_dir: &Path) -> String {
    let file = data_dir.join("party-device-id");
    if let Ok(saved) = std::fs::read_to_string(&file)
        && saved.trim().len() >= 16
    {
        return saved.trim().to_owned();
    }
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
    );
    h.write_usize(std::process::id() as usize);
    let id = format!("{:032x}", h.finish());
    let _ = std::fs::write(&file, &id);
    id
}

// ---- The client ---------------------------------------------------------------------

/// What the UI is told, in the order it happens. The state itself lives in the
/// client; events wake the UI to read it.
#[derive(Debug)]
pub enum Event {
    /// A membership was adopted (created or joined): the party as it stands now.
    Adopted,
    /// The socket is up or down again.
    Connection(Connection),
    /// The party is over for this device: `reason` says why, empty when we left.
    Ended(String),
}

struct Inner {
    backend: crate::backend::Backend,
    http: reqwest::Client,
    base: String,
    /// The party's code: the one joined, or the one the server handed out on create.
    code: std::sync::Mutex<String>,
    /// The membership token the REST calls and the socket carry.
    token: std::sync::Mutex<Option<String>>,
    identity: std::sync::Mutex<Option<JoinRequest>>,
    /// The newest truth the server has stated.
    state: std::sync::Mutex<State>,
    events: Sender<Event>,
    clock: ServerClock,
    /// The socket's sender while a session is up; frames sent before then are
    /// dropped, as the browser client drops them into a closed socket.
    out: std::sync::Mutex<Option<mpsc::UnboundedSender<String>>>,
    /// Set when the server has ended this membership (bad token, kicked): the run
    /// loop stops instead of redialling.
    over: AtomicBool,
}

/// A party this device belongs to.
pub struct Party {
    inner: Arc<Inner>,
    /// What arrived since the UI last asked.
    events: Receiver<Event>,
    handle: std::sync::Mutex<Option<tokio::task::AbortHandle>>,
}

impl Party {
    /// Create a party (`create`) or join `code` on `base`. The socket and its timers
    /// run on the backend's runtime; the UI is woken for every event.
    pub fn start(
        backend: &crate::backend::Backend,
        base: String,
        identity: JoinRequest,
        create: bool,
        code: String,
    ) -> Party {
        let (events, ev_rx) = unbounded();
        let inner = Arc::new(Inner {
            backend: backend.clone(),
            http: crate::proxy::http(),
            base: base.trim_end_matches('/').to_owned(),
            code: std::sync::Mutex::new(code),
            token: std::sync::Mutex::new(None),
            identity: std::sync::Mutex::new(Some(identity)),
            state: std::sync::Mutex::new(State::default()),
            events,
            clock: ServerClock::default(),
            out: std::sync::Mutex::new(None),
            over: AtomicBool::new(false),
        });
        let handle = backend.spawn_handle(Inner::run(inner.clone(), create));
        Party {
            inner,
            events: ev_rx,
            handle: std::sync::Mutex::new(Some(handle.abort_handle())),
        }
    }

    /// Leave, and tell the server (best effort) so the slot frees up at once.
    pub fn leave(&self) {
        let inner = self.inner.clone();
        self.inner.backend.spawn(async move {
            inner.release_slot().await;
        });
        if let Some(handle) = self.handle.lock().unwrap().take() {
            handle.abort();
        }
        self.inner.clear_out();
        self.inner.clock.reset();
        {
            let mut state = self.inner.state.lock().unwrap();
            *state = State::idle(std::mem::take(&mut state.activity));
        }
        self.inner.emit(Event::Ended(String::new()));
    }

    pub fn code(&self) -> String {
        self.inner.code.lock().unwrap().clone()
    }

    pub fn server(&self) -> String {
        self.inner.base.clone()
    }

    /// The newest truth the server has stated.
    pub fn state(&self) -> State {
        self.inner.state.lock().unwrap().clone()
    }

    /// The clock's offset (server minus here, ms) and round trip, once measured.
    pub fn clock(&self) -> Option<(i64, i64)> {
        self.inner.clock.reading()
    }

    /// Where the party's playhead is right now (`None` without a track).
    pub fn position_now(&self) -> Option<i64> {
        let state = self.inner.state.lock().unwrap().clone();
        position_now(&state, self.inner.clock.offset.load(Ordering::Relaxed))
    }

    /// How far ahead the party's next resume is scheduled (the play lead), or 0.
    pub fn ms_until_start(&self) -> i64 {
        let state = self.inner.state.lock().unwrap().clone();
        ms_until_start(&state, self.inner.clock.offset.load(Ordering::Relaxed))
    }

    /// Events since the last call.
    pub fn try_recv(&self) -> Option<Event> {
        self.events.try_recv().ok()
    }

    /// The page's "waiting for the party's resume" flag, held where the socket's
    /// report can read it too.
    pub fn set_awaiting(&self, on: bool) {
        if let Ok(mut state) = self.inner.state.lock() {
            state.awaiting_start = on;
        }
    }

    /// Wake the UI (a background task finished something worth drawing).
    pub fn wake(&self) {
        self.inner.backend.wake();
    }

    // ---- Controls (any member, unless the host has taken control) ------------------

    fn send(&self, frame: Value) {
        let out = self.inner.out.lock().unwrap().clone();
        if let Some(out) = out {
            let _ = out.send(frame.to_string());
        }
    }

    pub fn play(&self, position_ms: Option<i64>) {
        let mut frame = json!({ "type": "control", "action": "play" });
        if let Some(ms) = position_ms {
            frame["positionMs"] = json!(ms);
        }
        self.send(frame);
    }

    pub fn pause(&self, position_ms: Option<i64>) {
        let mut frame = json!({ "type": "control", "action": "pause" });
        if let Some(ms) = position_ms {
            frame["positionMs"] = json!(ms);
        }
        self.send(frame);
    }

    pub fn seek(&self, position_ms: i64) {
        self.send(json!({ "type": "control", "action": "seek", "positionMs": position_ms }));
    }

    pub fn next(&self) {
        self.send(json!({ "type": "control", "action": "next" }));
    }

    pub fn previous(&self) {
        self.send(json!({ "type": "control", "action": "previous" }));
    }

    pub fn set_track(&self, track: &PartyTrack, position_ms: i64, is_playing: bool) {
        self.send(json!({ "type": "control", "action": "setTrack", "track": track, "positionMs": position_ms, "isPlaying": is_playing }));
    }

    pub fn set_queue(&self, queue: &[PartyTrack], index: usize) {
        self.send(
            json!({ "type": "control", "action": "setQueue", "queue": queue, "queueIndex": index }),
        );
    }

    pub fn queue_add(&self, tracks: &[PartyTrack], play_next: bool) {
        self.send(json!({ "type": "control", "action": "queueAdd", "tracks": tracks, "playNext": play_next }));
    }

    pub fn queue_remove(&self, video_id: &str) {
        self.send(json!({ "type": "control", "action": "queueRemove", "videoId": video_id }));
    }

    pub fn queue_clear(&self) {
        self.send(json!({ "type": "control", "action": "queueClear" }));
    }

    pub fn queue_move(&self, from_index: usize, to_index: usize) {
        self.send(json!({ "type": "control", "action": "queueMove", "fromIndex": from_index, "toIndex": to_index }));
    }

    pub fn set_max_members(&self, max_members: usize) {
        self.send(
            json!({ "type": "control", "action": "setMaxMembers", "maxMembers": max_members }),
        );
    }

    pub fn kick(&self, member_id: &str) {
        self.send(json!({ "type": "control", "action": "kick", "memberId": member_id }));
    }

    pub fn set_autoplay(&self, enabled: bool) {
        self.send(json!({ "type": "control", "action": "setAutoplay", "enabled": enabled }));
    }

    pub fn set_host_only_control(&self, enabled: bool) {
        self.send(json!({ "type": "control", "action": "setHostOnlyControl", "enabled": enabled }));
    }
}

impl Inner {
    fn emit(&self, e: Event) {
        let _ = self.events.send(e);
        self.backend.wake();
    }

    fn clear_out(&self) {
        *self.out.lock().unwrap() = None;
    }

    /// The membership's slot, released (a leave that never reached the server is
    /// cleaned up by its sweep anyway).
    async fn release_slot(&self) {
        let token = self.token.lock().unwrap().clone();
        let Some(token) = token else { return };
        let _ = self
            .http
            .post(format!(
                "{}/api/parties/{}/leave",
                self.base,
                self.code.lock().unwrap().clone()
            ))
            .header("Authorization", format!("Bearer {token}"))
            .timeout(Duration::from_secs(5))
            .send()
            .await;
    }

    async fn run(self: Arc<Self>, create: bool) {
        // The membership comes first: a create (or join) that fails ends the party
        // before it began. The identity is spent here; a reconnect doesn't ask again.
        let identity = self.identity.lock().unwrap().take();
        let path = if create {
            "/api/parties".to_owned()
        } else {
            format!("/api/parties/{}/join", self.code.lock().unwrap().clone())
        };
        match self.ask("POST", &path, identity).await {
            Ok(membership) => {
                *self.token.lock().unwrap() = Some(membership.token.clone());
                self.adopt(&membership);
            }
            Err(e) => {
                let mut state = self.state.lock().unwrap();
                state.error = Some(e.message.clone());
                drop(state);
                self.emit(Event::Ended(e.message));
                return;
            }
        }
        let mut failures = 0u32;
        let mut backoff = 1000u64;
        loop {
            let end = self.session().await;
            {
                let mut state = self.state.lock().unwrap();
                state.connection = Connection::Connecting;
                state.clock_synced = false;
            }
            self.clock.reset();
            self.clear_out();
            if self.over.load(Ordering::Relaxed) {
                let reason = self.state.lock().unwrap().error.clone().unwrap_or_default();
                self.emit(Event::Ended(reason));
                return;
            }
            match end {
                SessionEnd::Left => {
                    self.emit(Event::Ended(String::new()));
                    return;
                }
                SessionEnd::Dropped(opened) => {
                    if !opened {
                        failures += 1;
                        if failures >= CHECK_AFTER_FAILURES {
                            failures = 0;
                            // The party this device was in may no longer exist: a restart
                            // or a sweep. Leave rather than redial forever.
                            if self.check_membership().await {
                                return;
                            }
                        }
                    } else {
                        failures = 0;
                        backoff = 1000;
                    }
                }
            }
            // Back off with jitter, as the browser client does.
            let wait = backoff / 2 + rand_u64(0, backoff + 1);
            backoff = (backoff * 2).min(MAX_BACKOFF_MS);
            tokio::time::sleep(Duration::from_millis(wait)).await;
        }
    }

    /// One socket, from dial to drop: handshake, the ping burst and the report
    /// trickle, and every frame the server states.
    async fn session(self: &Arc<Self>) -> SessionEnd {
        let Some(token) = self.token.lock().unwrap().clone() else {
            return SessionEnd::Left;
        };
        let ws_url = match self.base.as_str() {
            b if b.to_ascii_lowercase().starts_with("https://") => format!("wss://{}", &b[8..]),
            b if b.to_ascii_lowercase().starts_with("http://") => format!("ws://{}", &b[7..]),
            b => b.to_owned(),
        };
        let url = format!("{ws_url}/ws/parties/{}", self.code.lock().unwrap().clone());
        // The server reads the token from the socket's subprotocols, as the browser
        // client offers it.
        let request =
            match subprotocol_request(&url, &format!("bitchord.v1, bitchord.token.{token}")) {
                Ok(r) => r,
                Err(e) => {
                    log::info!("party: bad socket request: {e}");
                    return SessionEnd::Dropped(false);
                }
            };
        // Through the proxy setting, like every other socket (see jam.rs).
        let connect = async {
            let u = url::Url::parse(&url)?;
            let host = u.host_str().unwrap_or_default().to_owned();
            let port = u.port_or_known_default().unwrap_or(443);
            let tcp = crate::proxy::connect(&host, port).await?;
            anyhow::Ok(tokio_tungstenite::client_async_tls(request, tcp).await?)
        };
        let ws = match tokio::time::timeout(Duration::from_secs(12), connect).await {
            Ok(Ok((ws, _))) => ws,
            Ok(Err(e)) => {
                log::debug!("party: connect: {e}");
                return SessionEnd::Dropped(false);
            }
            Err(_) => return SessionEnd::Dropped(false),
        };
        let (mut write, mut read) = ws.split();
        let (out_tx, mut out_rx) = mpsc::unbounded_channel::<String>();
        *self.out.lock().unwrap() = Some(out_tx.clone());
        {
            let mut state = self.state.lock().unwrap();
            state.connection = Connection::Live;
            state.error = None;
        }
        self.emit(Event::Connection(Connection::Live));
        // A burst on arrival makes the first seconds accurate, then a trickle keeps
        // up with drift.
        let mut burst = PING_BURST;
        let mut next_ping = tokio::time::Instant::now();
        let mut next_report =
            tokio::time::Instant::now() + Duration::from_millis(REPORT_INTERVAL_MS as u64);

        let end = loop {
            tokio::select! {
                text = out_rx.recv() => {
                    let Some(text) = text else { break SessionEnd::Dropped(true) };
                    if let Err(e) = write.send(Message::text(text)).await {
                        log::debug!("party: send: {e}");
                        break SessionEnd::Dropped(true);
                    }
                }
                _ = tokio::time::sleep_until(next_ping) => {
                    self.send_frame(json!({ "type": "ping", "clientMs": now_ms() }));
                    let gap = if burst > 0 {
                        burst -= 1;
                        PING_BURST_GAP_MS
                    } else {
                        PING_INTERVAL_MS
                    };
                    next_ping = tokio::time::Instant::now() + Duration::from_millis(gap as u64);
                }
                _ = tokio::time::sleep_until(next_report) => {
                    // Where this device is, reported so drift (and presence) stay honest.
                    let (state, offset) = (self.state.lock().unwrap().clone(), self.clock.offset.load(Ordering::Relaxed));
                    if let Some(position) = position_now(&state, offset) {
                        self.send_frame(json!({ "type": "report", "positionMs": position, "isPlaying": state.playback.is_playing }));
                    }
                    next_report = tokio::time::Instant::now() + Duration::from_millis(REPORT_INTERVAL_MS as u64);
                }
                frame = read.next() => {
                    let Some(frame) = frame else { break SessionEnd::Dropped(true) };
                    match frame {
                        Ok(Message::Text(text)) => self.on_frame(&text),
                        Ok(Message::Close(_)) | Err(_) => break SessionEnd::Dropped(true),
                        Ok(_) => {}
                    }
                }
            }
        };
        self.clear_out();
        end
    }

    fn send_frame(&self, frame: Value) {
        let out = self.out.lock().unwrap().clone();
        if let Some(out) = out {
            let _ = out.send(frame.to_string());
        }
    }

    fn on_frame(&self, text: &str) {
        let Ok(frame) = serde_json::from_str::<Value>(text) else {
            return;
        };
        let kind = frame
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let mut state = self.state.lock().unwrap();
        match kind.as_str() {
            "welcome" => {
                if let Some(party) = frame.get("party") {
                    let snapshot: PartySnapshot =
                        serde_json::from_value(party.clone()).unwrap_or_default();
                    state.code = (!snapshot.code.is_empty())
                        .then(|| snapshot.code.clone())
                        .or_else(|| state.code.clone());
                    if let Some(you) = frame.get("you") {
                        state.you = Some(
                            serde_json::from_value::<PartyMember>(you.clone()).unwrap_or_default(),
                        );
                    }
                    state.members = snapshot.members;
                    state.max_members = snapshot.max_members;
                    state.host_only_control = snapshot.host_only_control;
                    state.playback = snapshot.playback;
                    // The one message that always carries the queue: a new arrival has
                    // no other way to learn it.
                    state.queue = snapshot.queue;
                    state.connection = Connection::Live;
                    state.error = None;
                }
            }
            "pong" => {
                let sent_at = frame.get("clientMs").and_then(Value::as_i64);
                let server_ms = frame.get("serverMs").and_then(Value::as_i64);
                if let (Some(sent_at), Some(server_ms)) = (sent_at, server_ms) {
                    let received = now_ms();
                    self.clock.record(sent_at, server_ms, received);
                    if let Some((offset, rtt)) = self.clock.reading() {
                        state.clock_synced = true;
                        state.round_trip_ms = rtt;
                        self.clock.offset.store(offset, Ordering::Relaxed);
                    }
                }
            }
            "state" => {
                let playback: PartyPlayback =
                    serde_json::from_value(frame.get("playback").cloned().unwrap_or(Value::Null))
                        .unwrap_or_default();
                // Older than what's applied: two controllers pressing at once settle
                // instead of fighting.
                if playback.seq >= state.playback.seq {
                    let queue_seq = playback.queue_seq;
                    state.playback = playback;
                    // A missed queue frame is healed on the next heartbeat rather than
                    // lived with.
                    if queue_seq != state.queue.seq {
                        self.send_frame(json!({ "type": "syncQueue" }));
                    }
                }
            }
            "queue" => {
                let queue: PartyQueue =
                    serde_json::from_value(frame.get("queue").cloned().unwrap_or(Value::Null))
                        .unwrap_or_default();
                if queue.seq >= state.queue.seq {
                    state.queue = queue;
                }
            }
            "members" => {
                state.members = frame
                    .get("members")
                    .and_then(Value::as_array)
                    .map(|members| {
                        members
                            .iter()
                            .map(|m| serde_json::from_value(m.clone()).unwrap_or_default())
                            .collect()
                    })
                    .unwrap_or_else(|| state.members.clone());
                if let Some(max) = frame.get("maxMembers").and_then(Value::as_u64) {
                    state.max_members = max as usize;
                }
                // Absent on an older server: keep what's held, so an upgrade doesn't
                // silently unlock a party.
                if let Some(hoc) = frame.get("hostOnlyControl").and_then(Value::as_bool) {
                    state.host_only_control = hoc;
                }
                // The only place a promotion is announced.
                if let Some(you) = &state.you
                    && let Some(mine) = state.members.iter().find(|m| m.member_id == you.member_id)
                {
                    state.you = Some(mine.clone());
                }
            }
            "activity" => {
                let action = frame
                    .get("action")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let by = frame
                    .get("by")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                if !action.is_empty() && !by.is_empty() {
                    let entry = PartyActivity {
                        action,
                        by,
                        at_ms: frame
                            .get("atMs")
                            .and_then(Value::as_i64)
                            .unwrap_or_else(now_ms),
                        detail: frame
                            .get("detail")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                    };
                    state.activity.insert(0, entry);
                    state.activity.truncate(100);
                }
            }
            "error" => {
                let reason = frame
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                state.error = Some(
                    frame
                        .get("message")
                        .and_then(Value::as_str)
                        .filter(|m| !m.is_empty())
                        .map_or(reason.clone(), str::to_owned),
                );
                // Terminal: the party no longer knows this device.
                if reason == "bad_token" || reason == "no_such_party" {
                    self.over.store(true, Ordering::Relaxed);
                }
            }
            "bye" => {
                let kicked = frame.get("reason").and_then(Value::as_str) == Some("kicked");
                let prev_error = state.error.clone();
                if kicked {
                    let code = state
                        .code
                        .clone()
                        .unwrap_or_else(|| self.code.lock().unwrap().clone());
                    drop(state);
                    record_kick(&self.backend.data_dir(), &code);
                    state = self.state.lock().unwrap();
                }
                if kicked {
                    state.error = Some("The host removed you from the party.".to_owned());
                } else {
                    state.error = prev_error;
                }
                self.over.store(true, Ordering::Relaxed);
            }
            _ => {}
        }
        drop(state);
        self.backend.wake();
    }

    /// A membership answered: the party as it stands, as this device's starting truth.
    fn adopt(&self, membership: &PartyMembership) {
        *self.code.lock().unwrap() = membership.code.clone();
        let mut state = self.state.lock().unwrap();
        let activity = std::mem::take(&mut state.activity);
        *state = State {
            code: Some(membership.code.clone()),
            you: Some(membership.you.clone()),
            members: membership.party.members.clone(),
            max_members: membership.party.max_members,
            host_only_control: membership.party.host_only_control,
            playback: membership.party.playback.clone(),
            queue: membership.party.queue.clone(),
            connection: Connection::Connecting,
            activity,
            ..Default::default()
        };
        drop(state);
        self.emit(Event::Adopted);
    }

    /// A failed join or create says what went wrong, as the page shows it.
    async fn ask(
        &self,
        method: &str,
        path: &str,
        body: Option<JoinRequest>,
    ) -> Result<PartyMembership, PartyError> {
        let mut req = match (method, &body) {
            ("POST", Some(identity)) => self
                .http
                .post(format!("{}{path}", self.base))
                .json(&identity.to_json()),
            ("POST", None) => self.http.post(format!("{}{path}", self.base)),
            _ => self.http.get(format!("{}{path}", self.base)),
        };
        if let Some(token) = self.token.lock().unwrap().clone() {
            req = req.header("Authorization", format!("Bearer {token}"));
        }
        let response = req
            .timeout(Duration::from_secs(12))
            .send()
            .await
            .map_err(|_| PartyError {
                code: "unreachable".into(),
                message: "Couldn't reach the party server.".into(),
            })?;
        let status = response.status().as_u16();
        let body: Value = response.json().await.unwrap_or(Value::Null);
        if !(200..300).contains(&status) {
            let code = body
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let message = body
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let message = if !message.is_empty() {
                message.to_owned()
            } else if status == 422 {
                "This account can't be used to jam.".to_owned()
            } else {
                format!("The party server said {status}.")
            };
            return Err(PartyError {
                code: if code.is_empty() {
                    format!("http_{status}")
                } else {
                    code.to_owned()
                },
                message,
            });
        }
        serde_json::from_value(body).map_err(|_| PartyError {
            code: "bad_answer".into(),
            message: "The party server answered something unreadable.".into(),
        })
    }

    /// A few failed opens in a row: is this membership still there?
    async fn check_membership(&self) -> bool {
        let token = self.token.lock().unwrap().clone();
        let Some(token) = token else { return false };
        let answer = self
            .http
            .get(format!(
                "{}/api/parties/{}",
                self.base,
                self.code.lock().unwrap().clone()
            ))
            .header("Authorization", format!("Bearer {token}"))
            .timeout(Duration::from_secs(10))
            .send()
            .await;
        if let Ok(response) = answer {
            let status = response.status().as_u16();
            if status == 401 || status == 404 {
                let body: Value = response.json().await.unwrap_or(Value::Null);
                let message = body
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let mut state = self.state.lock().unwrap();
                state.error = Some(if status == 404 {
                    "That party has ended.".to_owned()
                } else {
                    message.to_owned()
                });
                drop(state);
                self.over.store(true, Ordering::Relaxed);
                return true;
            }
        }
        // Unreachable or fine: keep trying.
        false
    }
}

/// The socket request, with the server's subprotocols offered.
fn subprotocol_request(
    url: &str,
    protocols: &str,
) -> anyhow::Result<tokio_tungstenite::tungstenite::http::Request<()>> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut request = url.into_client_request()?;
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        tokio_tungstenite::tungstenite::http::HeaderValue::from_str(protocols)?,
    );
    Ok(request)
}

/// A failed ask: the server's own code and the message the page shows.
#[derive(Debug)]
struct PartyError {
    #[allow(dead_code)]
    code: String,
    message: String,
}

enum SessionEnd {
    /// Left (or the app is closing): done.
    Left,
    /// The socket died; whether it had opened tells a dead party from a dead network.
    Dropped(bool),
}

fn rand_u64(min: u64, max: u64) -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
    );
    min + h.finish() % (max - min).max(1)
}

/// Look a party up before joining it (the invite card): who's in it, and whether
/// it's full. A server that predates the preview route answers a plain-text 404 —
/// still joinable, so that reads as an empty preview.
pub async fn preview(
    http: &reqwest::Client,
    base: &str,
    code: &str,
) -> Result<PartyPreview, String> {
    let url = format!("{}/api/parties/{code}/preview", base.trim_end_matches('/'));
    let response = http
        .get(url)
        .timeout(Duration::from_secs(12))
        .send()
        .await
        .map_err(|_| UNREACHABLE.to_owned())?;
    let status = response.status().as_u16();
    let text = response.text().await.unwrap_or_default();
    if status == 404 {
        // The router's plain-text 404: an older server, still joinable.
        return Ok(PartyPreview {
            code: code.to_owned(),
            ..Default::default()
        });
    }
    if !(200..300).contains(&status) {
        return Err(if text.is_empty() {
            format!("The party server said {status}.")
        } else {
            text.split('\n').next().unwrap_or_default().to_owned()
        });
    }
    serde_json::from_str(&text)
        .map_err(|_| "The party server answered something unreadable.".to_owned())
}

const UNREACHABLE: &str = "Couldn't reach the party server.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes() {
        assert_eq!(clean_code(" ab-c123 ").as_deref(), Some("ABC123"));
        assert_eq!(clean_code("abc12"), None);
        assert_eq!(clean_code("abc1234"), None);
        assert_eq!(clean_code(""), None);
    }

    #[test]
    fn servers() {
        assert_eq!(
            sanitize_server_url(" party.example.com/ "),
            Some("https://party.example.com".to_owned())
        );
        assert_eq!(
            sanitize_server_url("http://127.0.0.1:8787"),
            Some("http://127.0.0.1:8787".to_owned())
        );
        assert_eq!(sanitize_server_url(""), None);
        assert_eq!(sanitize_server_url(":::"), None);
        assert_eq!(idle_server(""), DEFAULT_SERVER);
        assert_eq!(
            idle_server("party.example.com"),
            "https://party.example.com"
        );
    }

    #[test]
    fn invites() {
        assert_eq!(
            parse_invite("bitchord://party/ABC123"),
            Some(("ABC123".to_owned(), None))
        );
        assert_eq!(
            parse_invite("bitchord://party/?c=abc123"),
            Some(("ABC123".to_owned(), None))
        );
        assert_eq!(
            parse_invite("https://bitchord.kushagrasingh.in/invite/ABC123"),
            Some(("ABC123".to_owned(), None))
        );
        assert_eq!(
            parse_invite(
                "https://bitchord.kushagrasingh.in/invite/abc123/?server=party.example.com"
            ),
            Some((
                "ABC123".to_owned(),
                Some("https://party.example.com".to_owned())
            ))
        );
        assert_eq!(parse_invite("bitchord://room/ABC123"), None);
        assert_eq!(
            parse_invite("https://bitchord.kushagrasingh.in/invite/ABC12"),
            None
        );
        // A custom server hosts its own invite page: any origin's /invite/<CODE> is one.
        assert_eq!(
            parse_invite("https://party.example.com/invite/ABC123"),
            Some(("ABC123".to_owned(), None))
        );
        assert_eq!(parse_invite("https://x.com/invitee/ABC123"), None);
        assert_eq!(parse_invite("https://x.com/invite/ABC123/extra"), None);
    }

    #[test]
    fn share_links() {
        assert_eq!(
            invite_url("abc123", None),
            format!("{INVITE_ORIGIN}/invite/ABC123")
        );
        assert_eq!(
            invite_url("abc123", Some("https://party.example.com")),
            "https://party.example.com/invite/ABC123"
        );
        assert_eq!(
            invite_url("abc123", Some(INVITE_ORIGIN)),
            format!("{INVITE_ORIGIN}/invite/ABC123")
        );
    }

    #[test]
    fn clock_picks_the_quickest_round_trip() {
        let clock = ServerClock::default();
        clock.record(0, 10_000, 100); // rtt 100, offset 9950
        clock.record(200, 20_000, 210); // rtt 10, offset 19995 — the winner
        let (offset, rtt) = clock.reading().unwrap();
        assert_eq!(rtt, 10);
        assert_eq!(offset, 20_000 - (200 + 10 / 2));
        // A stale window: only the newest sample is fresh, so it wins on its own.
        clock.record(200_000, 30_000, 200_100);
        let (offset, rtt) = clock.reading().unwrap();
        assert_eq!(rtt, 100);
        assert_eq!(offset, 30_000 - (200_000 + 100 / 2));
        clock.reset();
        assert!(clock.reading().is_none());
    }

    #[test]
    fn playhead() {
        let mut state = State::default();
        state.playback.track = Some(PartyTrack {
            duration_ms: Some(100_000),
            ..Default::default()
        });
        // Paused: the position as the server holds it.
        state.playback.is_playing = false;
        state.playback.position_ms = 40_000;
        assert_eq!(position_now(&state, 0), Some(40_000));
        // Playing: the anchor plus the time since, capped at the track's end.
        state.playback.is_playing = true;
        state.playback.position_ms = 40_000;
        state.playback.anchor_ms = now_ms() - 5_000;
        assert_eq!(position_now(&state, 0), Some(45_000));
        state.playback.position_ms = 99_000;
        state.playback.anchor_ms = now_ms() - 5_000;
        assert_eq!(position_now(&state, 0), Some(100_000));
        // The scheduled instant is in the future: the lead is what's left of it.
        state.playback.anchor_ms = now_ms() + 3_000;
        assert_eq!(ms_until_start(&state, 0), 3_000);
        state.playback.is_playing = false;
        assert_eq!(ms_until_start(&state, 0), 0);
    }

    #[test]
    fn queue_positions() {
        let track = |id: &str| PartyTrack {
            video_id: id.to_owned(),
            ..Default::default()
        };
        let queue = PartyQueue {
            seq: 2,
            index: 3,
            items: vec![track("a"), track("b"), track("c"), track("b"), track("d")],
        };
        let mut playback = PartyPlayback {
            queue_seq: 1,
            queue_index: 1,
            ..Default::default()
        };
        // The state's index is older than the queue: the queue's own index wins, and
        // the second "b" (after the hint) is nearer than the first.
        assert_eq!(party_queue_index_of(&queue, &playback, Some("b")), 3);
        assert_eq!(party_queue_index_of(&queue, &playback, Some("a")), 0);
        assert_eq!(party_queue_index_of(&queue, &playback, Some("nope")), -1);
        assert_eq!(party_queue_index_of(&queue, &playback, None), -1);
        playback.queue_seq = 2;
        // The state's own index names the track: it wins outright.
        playback.queue_index = 3;
        assert_eq!(party_queue_index_of(&queue, &playback, Some("b")), 3);
        // What comes after "b" is whatever follows it.
        assert_eq!(
            party_upcoming_after(&queue, &playback, Some("b"))[0].video_id,
            "d"
        );
        assert!(party_upcoming_after(&queue, &playback, Some("d")).is_empty());
    }

    #[test]
    fn single_moves() {
        let before = ["a", "b", "c", "d"]
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        let mut after = before.clone();
        after.remove(1);
        after.insert(3, "b".to_owned());
        assert_eq!(
            detect_single_move(&before, &after),
            Some((1, 3, "b".to_owned()))
        );
        assert_eq!(detect_single_move(&before, &before), None);
        let swapped = ["b", "a", "c", "d"]
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        assert!(detect_single_move(&before, &swapped).is_some());
        let doubled = ["a", "b", "c", "c"]
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        assert_eq!(detect_single_move(&before, &doubled), None);
    }

    #[test]
    fn autoplay_suppliers() {
        let member = |id: &str, host: bool, on: bool| PartyMember {
            member_id: id.to_owned(),
            is_host: host,
            connected: on,
            ..Default::default()
        };
        let members = [
            member("h", true, true),
            member("b", false, true),
            member("a", false, true),
        ];
        // The host while connected, ahead of everyone else.
        assert_eq!(autoplay_supplier_id(&members, false).as_deref(), Some("h"));
        // A host who isn't holding a socket isn't the supplier either.
        let gone = [
            member("h", true, false),
            member("b", false, true),
            member("a", false, true),
        ];
        assert_eq!(autoplay_supplier_id(&gone, false).as_deref(), Some("a"));
        // Locked: the host alone tops up, and nobody stands in for a host who
        // isn't holding a socket.
        assert_eq!(autoplay_supplier_id(&members, true).as_deref(), Some("h"));
        assert_eq!(autoplay_supplier_id(&gone, true), None);
        // No host at all: the lowest connected member id.
        let members = [member("b", false, true), member("a", false, true)];
        assert_eq!(autoplay_supplier_id(&members, false).as_deref(), Some("a"));
    }

    #[test]
    fn frames_fill_with_defaults() {
        let playback =
            playback_of(&json!({ "seq": 4, "track": { "videoId": "x", "title": "Song" } }));
        assert_eq!(playback.seq, 4);
        assert_eq!(playback.track.as_ref().unwrap().video_id, "x");
        assert_eq!(playback.track.as_ref().unwrap().artist, "");
        assert!(!playback.is_playing);
        assert_eq!(playback.queue_index, 0);
        let queue = queue_of(&json!({ "seq": 1, "items": [{ "videoId": "y" }] }));
        assert_eq!(queue.items.len(), 1);
        assert_eq!(queue.items[0].title, "");
        let snapshot = snapshot_of(&json!({ "members": [{}] }));
        assert_eq!(snapshot.members.len(), 1);
        assert_eq!(snapshot.max_members, 0);
    }

    #[test]
    fn locked_parties() {
        let mut state = State::default();
        assert!(!controls_locked(&state));
        state.code = Some("ABC123".into());
        state.host_only_control = true;
        state.you = Some(PartyMember {
            is_host: true,
            ..Default::default()
        });
        // The host is never locked out of their own party.
        assert!(!controls_locked(&state));
        state.you = Some(PartyMember {
            is_host: false,
            ..Default::default()
        });
        assert!(controls_locked(&state));
        state.host_only_control = false;
        assert!(!controls_locked(&state));
    }
}
