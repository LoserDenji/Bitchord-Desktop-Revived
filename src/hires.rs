//! High-resolution audio from a compatible addon server.
//!
//! The addon protocol is three GET endpoints returning JSON: `{base}/manifest.json`,
//! `{base}/search?q=…&quality=LOSSLESS` and `{base}/stream/{trackId}?quality=LOSSLESS`.
//! A track found there plays from the addon's own URL instead of YouTube Music; when
//! the addon has no copy, or the answer isn't playable, playback falls back to YouTube.
//! This is the same protocol the BitChord Android app's module sources speak, so a
//! link shared for the phone works here too — except for addons that turn on
//! `"bitchordAuth": true`: those verify every request against a hardware-backed key
//! only the official Android app can hold, and this client detects that and says so
//! instead of failing mysteriously. A base URL may carry a query of its own — install
//! links like `…/quality/manifest.json?sources=tidal` normalize to one — and every
//! endpoint is built so the path lands before that query, not after it.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::Value;

use crate::stream::Format;

/// What the addon server is asked to recognise us as. Addons allow-list the desktop
/// release line's agent (`BitChord/v1.x`) and answer `403 UA_BLOCKED` to anything else,
/// so this build's own name goes first and the release line's is what a refusal falls
/// back to.
pub const USER_AGENT: &str = "Bitchord-Desktop-Revived";
const RELEASE_AGENT: &str = "BitChord/v1.9";
/// Set once an addon has refused our own name: it keeps the release line's agent for the
/// rest of the session instead of being refused on every request.
static RELEASE_LINE: AtomicBool = AtomicBool::new(false);

/// The agent to ask with right now.
pub fn agent() -> &'static str {
    if RELEASE_LINE.load(Ordering::Relaxed) { RELEASE_AGENT } else { USER_AGENT }
}

/// Moves this session's requests over to the release line's agent.
fn use_release_agent() {
    RELEASE_LINE.store(true, Ordering::Relaxed);
}

const TIER: &str = "LOSSLESS";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);
/// `/stream` makes the addon resolve the song with its own upstream, and a cold lookup
/// can take a while: a TorBox-backed addon cold-caching a long FLAC spends twenty to
/// thirty seconds before it can answer at all, and thirty-two was the worst a real one
/// took. Forty-five seconds is the Android app's own ceiling per call; the playback
/// deadlines (the backend's `HIRES_WAIT` and `UPGRADE_WAIT`) still bound the lookup, so
/// a slow resolve costs only the path that could afford it, rather than being cut off
/// and the song believed not to exist.
const STREAM_TIMEOUT: Duration = Duration::from_secs(45);
/// A 429 is waited out, not surfaced (the Android app's back-off): `Retry-After` is
/// honored when sent. The generated wait doubles from half a second up to the cap;
/// the playback deadline still bounds how long a track can wait.
const THROTTLE_BASE: Duration = Duration::from_millis(500);
const THROTTLE_CAP: Duration = Duration::from_secs(8);
/// How many 429s one call waits out before the addon is left alone for this track.
const THROTTLE_WAITS: u32 = 2;
/// How long a `/stream` answer is reused before the addon is asked again — the
/// Android app's `STREAM_TTL_MS`, well inside the hour the reference host assumes
/// when an addon states no expiry of its own.
const STREAM_TTL: Duration = Duration::from_secs(5 * 60);
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
/// How many of the search's candidates get a `/stream` call before giving up —
/// the Android app's `STREAM_ATTEMPTS`, enough to get past a row whose quality
/// wasn't what it claimed, without walking a whole result list at a backend
/// that mints a session per call.
const STREAM_TRIES: usize = 3;
/// With Atmos on, a lossless copy already in hand waits this long for an Atmos answer
/// before it plays; the whole race also stays inside the playback path's own budget.
const ATMOS_GRACE: Duration = Duration::from_secs(5);
/// The size given a stream whose addon and server don't say how big it is: room enough
/// for ranged reads while the download finds the real end.
pub const UNKNOWN_SIZE: u64 = 512 * 1024 * 1024;

/// What the player says about the stream it is playing.
#[derive(Clone, Debug, PartialEq)]
pub struct Info {
    /// The addon's own name (or its host when it publishes none).
    pub source: String,
    /// The host the audio comes from, without the path (it can carry a token).
    pub host: String,
    /// `FLAC`, `ALAC`, `MP3`, … as stated by the addon or read off the URL.
    pub codec: String,
    /// The quality label the addon sent, when it sent one.
    pub quality: String,
    pub bitrate_kbps: Option<u32>,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u32>,
    pub channels: Option<u8>,
    /// The stream is a Dolby Atmos rendition.
    pub atmos: bool,
    pub size: u64,
    pub duration_ms: u64,
}

impl Info {
    pub fn lossless(&self) -> bool {
        matches!(self.codec.to_ascii_lowercase().as_str(), "flac" | "alac" | "wav" | "aiff") || has_lossless_word(&self.quality)
    }

    /// More than CD: better than 48 kHz or more than 16 bits.
    pub fn hires(&self) -> bool {
        self.lossless() && (self.sample_rate.is_some_and(|r| r > 48_000) || self.bit_depth.is_some_and(|b| b > 16))
    }

    /// The quality mark's text, worded as Apple Music words it.
    pub fn badge(&self) -> &'static str {
        if !self.atmos && !self.hires() && !self.lossless() && self.bitrate_kbps.is_some_and(|k| k >= 320) {
            return "320 kbps";
        }
        self.tier()
    }

    /// The tier without a bitrate in it, for anywhere that names the bitrate
    /// separately: `badge`'s "320 kbps" would only repeat what stands beside it.
    pub fn tier(&self) -> &'static str {
        if self.atmos {
            "Dolby Atmos"
        } else if self.hires() {
            "Hi-Res Lossless"
        } else if self.lossless() {
            "Lossless"
        } else {
            "High"
        }
    }

    /// The tier as a rung of one ladder: lossy < Lossless < Hi-Res < Atmos. A background
    /// upgrade is worth the swap only when the copy it found sits strictly above the one
    /// playing — a same-rung answer (an addon's FLAC for a provider's FLAC, say) would
    /// only restart the download and stutter the song for no gain the ear can hear.
    pub fn rung(&self) -> u8 {
        if self.atmos {
            3
        } else if self.hires() {
            2
        } else if self.lossless() {
            1
        } else {
            0
        }
    }

}

/// What a source's Test says, ported from BitChord's `SourceHealth`
/// (`AddonSource.health()`): the manifest answers reachable, well-formed and useful
/// all at once, so the three ways a link can be stand out on the editor's Test line.
/// `Rejected` is the addon (or the address) being wrong in a way no amount of
/// retrying fixes; `Unreachable` is a server having a bad minute, worth trying again.
#[derive(Clone, Debug, PartialEq)]
pub enum Health {
    /// There and usable, with what the addon said about itself.
    Ok(String),
    /// The addon refused, or isn't an addon: retrying won't help.
    Rejected(String),
    /// Couldn't be asked this once: worth trying again.
    Unreachable(String),
}

impl Health {
    /// Whether the row should read as good (a check, not a cross).
    pub fn good(&self) -> bool {
        matches!(self, Health::Ok(_))
    }

    /// The line under the row.
    pub fn message(&self) -> &str {
        match self {
            Health::Ok(m) | Health::Rejected(m) | Health::Unreachable(m) => m,
        }
    }
}

/// One playable answer from the addon.
#[derive(Clone)]
pub struct Stream {
    pub url: String,
    pub size: u64,
    pub duration_ms: u64,
    pub format: Format,
    pub info: Info,
    /// Already-decoded audio (Dolby Atmos out of DASH): no download to run.
    pub data: Option<Vec<u8>>,
    pub selection: Selection,
}

#[derive(Clone, Copy)]
pub(crate) enum Lookup {
    Prefetch,
    Playback,
    Startup,
    /// A manual upgrade or a refresh can bypass the cache to mint a new URL.
    Upgrade { fresh: bool },
    Refresh,
}

/// The same addon request can return a fresh media URL without running another race.
#[derive(Clone)]
pub struct Selection {
    client: Arc<Client>,
    candidate: Candidate,
    atmos: bool,
    revision: u64,
    refreshed: bool,
}

impl Selection {
    async fn invalidate(&self) {
        let key = (self.candidate.id.clone(), self.atmos);
        self.client.streams.lock().await.invalidate(&key, self.revision);
    }

    pub async fn refresh(&self, http: &reqwest::Client) -> anyhow::Result<Stream> {
        // A refresh stands up a stream that was already playing — the upgrade
        // pass's situation, so its rendition may be a manifest one — and it runs
        // because the link in hand stopped working, so the cached answer (the
        // same link) is skipped for a fresh mint.
        anyhow::ensure!(!self.refreshed, "the addon already provided a fresh URL");
        self.invalidate().await;
        self.client.open(http, &self.candidate, self.atmos, Lookup::Refresh).await?
            .ok_or_else(|| anyhow::anyhow!("the selected addon has no stream"))
    }
}

/// One addon server, with its manifest and recent searches remembered.
pub struct Client {
    base: String,
    manifest: tokio::sync::Mutex<Option<Manifest>>,
    searches: tokio::sync::Mutex<SearchCache>,
    /// When this addon may be asked again, after it said not yet (a 429's back-off or
    /// a 5xx's bad minute). Per-addon and not per-request, for the reason the Android
    /// app gives: a 429 is a statement about the server, not about the one call that
    /// happened to draw it, so a search landing on one holds back the stream lookup
    /// queued behind it too — without that, a fan-out simply re-trips the limiter
    /// with every sibling request and the addon never gets the quiet it asked for.
    quiet_until: QuietUntil,
    /// Recent `/stream` answers, keyed by candidate id — the same track passes this
    /// way several times over (the play, the upgrade pass, a re-open when the player
    /// asks for fresh URLs), and each re-mint is another session an addon's backend
    /// has to stand up for a link it had already handed out.
    streams: tokio::sync::Mutex<StreamCache>,
}

// None represents a server delay beyond the range of the platform clock.
type QuietUntil = tokio::sync::Mutex<Option<tokio::time::Instant>>;

struct Manifest {
    name: Option<String>,
    defaults: Vec<(String, String)>,
    quality: String,
    /// The addon asks for BitChord's Android-only authentication
    /// (`"bitchordAuth": true`), which a desktop client can't provide.
    auth: bool,
}

#[derive(Clone)]
pub(crate) struct Candidate {
    id: String,
    title: String,
    artist: String,
    duration: Option<u32>,
    quality: String,
    format: String,
    atmos: bool,
    stream_url: Option<String>,
}

/// A server's recent searches, remembering the order they arrived: a full cache sheds
/// its oldest entries, not everything at once (a clear-all made the next burst of songs
/// each re-ask a server that had just answered them).
#[derive(Default)]
struct SearchCache {
    answers: HashMap<String, Vec<Candidate>>,
    order: std::collections::VecDeque<String>,
}

const SEARCH_CACHE: usize = 64;

/// A server's recent `/stream` answers, fresh for `STREAM_TTL`. The answer kept is
/// the addon's own JSON, never a decoded rendition (megabytes of PCM per entry);
/// the rendition itself is fetched and decoded only when the lookup can play it.
/// A failure is not kept either — an addon that was briefly unable to mint a
/// session should be asked again on the next track, not written off for the window.
#[derive(Default)]
struct StreamCache {
    answers: HashMap<(String, bool), CachedAnswer>,
    revision: u64,
    order: std::collections::VecDeque<(String, bool)>,
}

struct CachedAnswer {
    json: Option<Value>,
    revision: u64,
    stored_at: tokio::time::Instant,
}

impl StreamCache {
    fn get(&self, key: &(String, bool)) -> Option<(Value, u64)> {
        let answer = self.answers.get(key)?;
        (answer.stored_at.elapsed() < STREAM_TTL).then_some(())?;
        Some((answer.json.as_ref()?.clone(), answer.revision))
    }

    fn invalidate(&mut self, key: &(String, bool), revision: u64) {
        if let Some(answer) = self.answers.get_mut(key)
            && answer.revision == revision
        {
            // Keep the revision so older requests in flight cannot restore this URL.
            answer.json = None;
        }
    }

    fn insert(&mut self, key: (String, bool), found: Value, revision: u64) {
        if self.answers.get(&key).is_some_and(|answer| answer.revision >= revision) {
            return;
        }
        if !self.answers.contains_key(&key) {
            self.order.push_back(key.clone());
            while self.order.len() > SEARCH_CACHE {
                if let Some(oldest) = self.order.pop_front() {
                    self.answers.remove(&oldest);
                }
            }
        }
        self.answers.insert(key, CachedAnswer { json: Some(found), revision, stored_at: tokio::time::Instant::now() });
    }
}

impl SearchCache {
    fn get(&self, key: &str) -> Option<&Vec<Candidate>> {
        self.answers.get(key)
    }

    fn insert(&mut self, key: String, found: Vec<Candidate>) {
        if !self.answers.contains_key(&key) {
            self.order.push_back(key.clone());
            while self.order.len() > SEARCH_CACHE {
                if let Some(oldest) = self.order.pop_front() {
                    self.answers.remove(&oldest);
                }
            }
        }
        self.answers.insert(key, found);
    }
}

impl Client {
    /// A client for whatever the listener pasted. `None` when it isn't an http(s) URL.
    pub fn new(raw: &str) -> Option<Self> {
        let base = normalize_base(raw);
        let parsed = url::Url::parse(&base).ok()?;
        if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none_or(str::is_empty) {
            return None;
        }
        Some(Self {
            base,
            manifest: Default::default(),
            searches: Default::default(),
            // The epoch is "never quiet": an addon starts owed its first question.
            quiet_until: tokio::sync::Mutex::new(Some(tokio::time::Instant::now())),
            streams: Default::default(),
        })
    }

    /// The addon's host, for a log line (the link's path may carry a token).
    pub(crate) fn same_link(&self, other: &Self) -> bool {
        self.base == other.base
    }

    pub fn host(&self) -> String {
        host_of(&self.base)
    }

    /// The rows that could be `track`, best first. `atmos` also asks the addon for its
    /// Dolby Atmos renditions, so the caller can prefer one across every source.
    pub async fn picks(&self, http: &reqwest::Client, track: &crate::model::Track, atmos: bool) -> anyhow::Result<Vec<Candidate>> {
        let manifest = self.manifest(http).await?;
        if manifest.auth {
            anyhow::bail!("{}", AUTH_DESKTOP_MSG);
        }
        let artist = track.artists.first().map(|a| a.name.clone()).unwrap_or_else(|| track.artist_line());
        let title = track.title.clone();
        let mut queries = Vec::new();
        let searchable = searchable_title(&title, &artist);
        let primary = primary_artist(&artist);
        if !searchable.is_empty() {
            queries.push(if primary.is_empty() { searchable.clone() } else { format!("{searchable} {primary}") });
            if !primary.is_empty() && queries.first().is_none_or(|q| q != &searchable) {
                queries.push(searchable);
            }
        }
        // The Atmos-aware search marks rows the plain one can't (an Atmos rendition is
        // often a separate, low-bitrate lossy row), then the plain pass fills in the
        // stereo lossless copies for when no Atmos row turns up.
        let passes: &[(bool, bool)] = if atmos { &[(true, true), (false, false)] } else { &[(false, false)] };
        let mut out: Vec<Candidate> = Vec::new();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut last_err = None;
        for &(search_atmos, prefer_atmos) in passes {
            for query in &queries {
                let candidates = match self.search(http, &manifest, query, search_atmos).await {
                    Ok(c) => c,
                    Err(e) => { last_err = Some(e); continue; }
                };
                for candidate in rank(candidates, &title, &artist, track.duration, prefer_atmos) {
                    if seen.insert(candidate.id.clone()) {
                        out.push(candidate);
                    }
                }
            }
        }
        if let (true, Some(e)) = (out.is_empty(), last_err) {
            return Err(e);
        }
        Ok(out)
    }

    /// The playable stream behind one `picks` row, probed for its size (Dolby Atmos
    /// included). A miss is `None`, not a fault. A full manifest rendition is tried
    /// during an upgrade or when playback has no other source to start from.
    async fn open(self: &Arc<Self>, http: &reqwest::Client, candidate: &Candidate, atmos: bool, lookup: Lookup) -> anyhow::Result<Option<Stream>> {
        let manifest = self.manifest(http).await?;
        self.stream(http, &manifest, candidate, atmos, lookup).await
    }

    async fn manifest(&self, http: &reqwest::Client) -> anyhow::Result<Manifest> {
        let mut slot = self.manifest.lock().await;
        if let Some(m) = slot.as_ref() {
            return Ok(Manifest { name: m.name.clone(), defaults: m.defaults.clone(), quality: m.quality.clone(), auth: m.auth });
        }
        let url = self.endpoint(&["manifest.json"], &[])?;
        let json = get_json(http, &url, REQUEST_TIMEOUT, &self.quiet_until).await?;
        let id = json.get("id").and_then(Value::as_str).unwrap_or("").trim();
        anyhow::ensure!(!id.is_empty(), "that URL answered, but not with an addon manifest");
        let resources: Vec<&str> = json.get("resources").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
        anyhow::ensure!(resources.is_empty() || resources.iter().any(|r| r.eq_ignore_ascii_case("search")), "the addon doesn't offer search");
        let name = json.get("name").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned);
        let (defaults, quality) = settings(json.get("settings"));
        let manifest = Manifest { name, defaults, quality, auth: manifest_auth(&json) };
        *slot = Some(Manifest { name: manifest.name.clone(), defaults: manifest.defaults.clone(), quality: manifest.quality.clone(), auth: manifest.auth });
        Ok(manifest)
    }

    async fn search(&self, http: &reqwest::Client, manifest: &Manifest, query: &str, atmos: bool) -> anyhow::Result<Vec<Candidate>> {
        let key = format!("{}|{query}|{atmos}", manifest.quality);
        if let Some(hit) = self.searches.lock().await.get(&key) {
            return Ok(hit.clone());
        }
        let mut params = manifest.defaults.clone();
        params.push(("quality".to_owned(), manifest.quality.clone()));
        if atmos {
            params.push(("atmos".to_owned(), "auto".to_owned()));
        }
        params.push(("q".to_owned(), query.to_owned()));
        let url = self.endpoint(&["search"], &params)?;
        let json = get_json(http, &url, REQUEST_TIMEOUT, &self.quiet_until).await?;
        let tracks = json.get("tracks").and_then(Value::as_array).cloned().unwrap_or_default();
        let found: Vec<Candidate> = tracks.iter().filter_map(candidate).take(400).collect();
        self.searches.lock().await.insert(key, found.clone());
        Ok(found)
    }

    async fn stream(self: &Arc<Self>, http: &reqwest::Client, manifest: &Manifest, candidate: &Candidate, atmos: bool, lookup: Lookup) -> anyhow::Result<Option<Stream>> {
        // The answer behind this row is reused for a few minutes: the addon hands
        // out the same rendition either way, and every re-mint is another session
        // on its backend (the Android app's `STREAM_TTL_MS`). `fresh` is the
        // refresh path, which runs because the link already handed out stopped
        // working — the cache would hand the same dead link right back.
        let key = (candidate.id.clone(), atmos);
        let cached = {
            let mut streams = self.streams.lock().await;
            if !matches!(lookup, Lookup::Upgrade { fresh: true } | Lookup::Refresh)
                && let Some(hit) = streams.get(&key)
            {
                Ok(hit)
            } else {
                streams.revision += 1;
                Err(streams.revision)
            }
        };
        let (json, revision) = match cached {
            Ok(hit) => hit,
            Err(revision) => {
                let answer = self.mint(http, manifest, candidate, atmos).await?;
                // A 404 can still use the search row; it is not a successful mint.
                if !answer.is_null() && !matches!(lookup, Lookup::Refresh) {
                    self.streams.lock().await.insert(key, answer.clone(), revision);
                }
                (answer, revision)
            }
        };
        let stated_url = json.get("url").and_then(Value::as_str).unwrap_or("").trim().to_owned();
        let stated_url = if stated_url.is_empty() { candidate.stream_url.clone().unwrap_or_default() } else { stated_url };
        if stated_url.is_empty() {
            return Ok(None);
        }
        let selection = Selection { client: self.clone(), candidate: candidate.clone(), atmos, revision, refreshed: matches!(lookup, Lookup::Refresh) };
        anyhow::ensure!(playable_url(&stated_url), "the addon returned a URL that isn't http(s)");
        anyhow::ensure!(!encrypted(&json), "the addon returned an encrypted stream");
        // A rendition served by the addon's own host paces its download differently
        // from a CDN's (see `dash::note_media_host`).
        crate::dash::note_media_host(&self.base, &stated_url);
        let transport_line = json.get("format").and_then(Value::as_str).map(str::trim).unwrap_or("");
        let transport = json.get("manifest").and_then(Value::as_str).or(json.get("mediaType").and_then(Value::as_str)).map(str::trim).filter(|s| !s.is_empty()).unwrap_or(transport_line);
        let quality = json.get("quality").and_then(Value::as_str).or(json.get("streamQuality").and_then(Value::as_str)).or(json.get("audioQuality").and_then(Value::as_str)).unwrap_or("").trim().to_owned();
        let label = format!("{quality} {} {}", candidate.quality, candidate.format);
        let codec = json.get("codec").and_then(Value::as_str).or(json.get("fileCodec").and_then(Value::as_str)).map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).unwrap_or_default();
        let container = json.get("container").and_then(Value::as_str).or(json.get("containerFormat").and_then(Value::as_str)).map(str::trim).filter(|s| !s.is_empty()).unwrap_or("");
        // An addon that names no container puts the file format in `format`, which is the
        // transport keyword instead when the answer is a manifest (handled above).
        let container = if container.is_empty() && !is_manifest(transport_line) { transport_line } else { container };
        let mime = json.get("mimeType").and_then(Value::as_str).unwrap_or("");
        if is_manifest(transport) {
            // DASH arrives as an MPD, not a file: the Atmos E-AC-3 rendition (or the
            // hi-res FLAC one) is fetched whole and decoded to PCM, so it plays and
            // seeks like any other stream.
            let dash = matches!(transport.trim().to_ascii_lowercase().as_str(), "dash" | "mpd" | "application/dash+xml");
            // Avoid an expensive download while fallback audio can start. When
            // nothing else can play, the startup lookup can try within its deadline.
            anyhow::ensure!(matches!(lookup, Lookup::Startup | Lookup::Upgrade { .. } | Lookup::Refresh), "a manifest rendition is deferred until needed for playback or an upgrade");
            anyhow::ensure!(dash, "the addon returned a streaming manifest, not a file");
            let audio = match crate::dash::fetch(http, &stated_url).await {
                Err(e) if !matches!(lookup, Lookup::Refresh) && crate::stream::is_media_rejected(&e) => {
                    selection.invalidate().await;
                    return Box::pin(self.stream(http, manifest, candidate, atmos, Lookup::Refresh)).await;
                }
                result => result?,
            };
            let size = audio.pcm.len() as u64;
            let info = Info {
                source: manifest.name.clone().unwrap_or_else(|| host_of(&self.base)),
                host: host_of(&stated_url),
                codec: audio.codec.to_owned(),
                quality: if quality.is_empty() { candidate.quality.clone() } else { quality },
                bitrate_kbps: Some(audio.bitrate_kbps).filter(|k| *k > 0),
                sample_rate: Some(audio.sample_rate),
                bit_depth: Some(audio.bit_depth).filter(|b| *b > 0),
                channels: Some(2),
                atmos: audio.atmos,
                size,
                duration_ms: audio.duration_ms,
            };
            let stream = Stream { url: stated_url, size, duration_ms: audio.duration_ms, format: Format::Pcm { rate: audio.sample_rate, channels: 2 }, info, data: Some(audio.pcm), selection };
            return Ok(Some(stream));
        }
        let Some(format) = format_of(&codec, container, mime, &stated_url, &label) else {
            anyhow::bail!("the addon's format isn't playable ({codec} {container} {mime})");
        };
        let mut size = json_size(&json);
        if size == 0 {
            size = content_size(http, &stated_url).await.unwrap_or(0);
        }
        // Some addon URLs only reveal their size once the stream starts; use a placeholder
        // large enough for ranged reads and let the download determine the real end.
        if size == 0 {
            log::info!("hi-res: stream size unknown for \"{}\", using ranged placeholder", candidate.title);
            size = UNKNOWN_SIZE;
        }
        let sample_rate = json.get("sampleRate").and_then(Value::as_f64).filter(|r| *r > 0.0).map(|r| if r < 1000.0 { (r * 1000.0) as u32 } else { r as u32 })
            .or_else(|| number_before(&label, "khz").map(|r| (r * 1000.0) as u32));
        let bit_depth = json.get("bitDepth").and_then(Value::as_f64).filter(|b| *b > 0.0).map(|b| b as u32)
            .or_else(|| number_before(&label, "bit").filter(|b| (8.0..=32.0).contains(b)).map(|b| b as u32));
        let bitrate_kbps = json.get("bitrate").and_then(Value::as_f64).filter(|k| *k > 0.0).map(|k| if k > 3000.0 { (k / 1000.0) as u32 } else { k as u32 })
            .or_else(|| number_before(&label, "kbps").map(|k| k as u32));
        let duration_ms = candidate.duration.map_or(0, |s| s as u64 * 1000);
        let atmos = candidate.atmos || json_is_atmos(&json);
        let info = Info {
            source: manifest.name.clone().unwrap_or_else(|| host_of(&self.base)),
            host: host_of(&stated_url),
            codec: codec_name(&codec, &format),
            quality: if quality.is_empty() { candidate.quality.clone() } else { quality },
            bitrate_kbps,
            sample_rate,
            bit_depth,
            channels: None,
            atmos,
            size,
            duration_ms,
        };
        let stream = Stream { url: stated_url, size, duration_ms, format, info, data: None, selection };
        Ok(Some(stream))
    }

    /// One `/stream` call: the addon's answer, or `Null` for a 404 — the row may
    /// still carry its own URL, which the caller reads off the candidate.
    async fn mint(&self, http: &reqwest::Client, manifest: &Manifest, candidate: &Candidate, atmos: bool) -> anyhow::Result<Value> {
        let mut params = manifest.defaults.clone();
        params.push(("quality".to_owned(), manifest.quality.clone()));
        if atmos {
            params.push(("atmos".to_owned(), "auto".to_owned()));
        }
        let url = self.endpoint(&["stream", &candidate.id], &params)?;
        match get_json(http, &url, STREAM_TIMEOUT, &self.quiet_until).await {
            Ok(j) => Ok(j),
            // A miss, not a fault: the row may still carry its own URL.
            Err(e) if e.to_string().contains("not found") => Ok(Value::Null),
            Err(e) => Err(e),
        }
    }

    fn endpoint(&self, segments: &[&str], params: &[(String, String)]) -> anyhow::Result<String> {
        let mut url = base_url_with(&self.base, segments)?;
        {
            let mut query = url.query_pairs_mut();
            for (key, value) in params {
                query.append_pair(key, value);
            }
        }
        Ok(url.into())
    }

    /// The editor's Test: health-checks a link whether or not it's been saved, so a
    /// typed one reports on itself rather than on a stored source (BitChord's
    /// `SourceRegistry::probeCandidate`). The manifest answers reachable, well-formed
    /// and useful in one read; an addon that never published one may still answer a
    /// search, and is healthy the moment it does — which is why a failed manifest read
    /// is given the second opinion before it's called a fault.
    pub async fn probe(&self, http: &reqwest::Client) -> Health {
        let url = match self.endpoint(&["manifest.json"], &[]) {
            Ok(url) => url,
            Err(e) => return Health::Rejected(format!("{e:#}")),
        };
        let manifest = async {
            let json = get_json(http, &url, REQUEST_TIMEOUT, &self.quiet_until).await?;
            let id = json.get("id").and_then(Value::as_str).unwrap_or("").trim();
            anyhow::ensure!(!id.is_empty(), "that URL answered, but not with an addon manifest");
            let resources: Vec<&str> = json.get("resources").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
            anyhow::ensure!(resources.is_empty() || resources.iter().any(|r| r.eq_ignore_ascii_case("search")), "the addon doesn't offer search");
            let name = json.get("name").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned);
            Ok::<_, anyhow::Error>((name, manifest_auth(&json)))
        }
        .await;
        match manifest {
            Ok((name, auth)) => {
                if auth {
                    return Health::Rejected(AUTH_DESKTOP_MSG.to_owned());
                }
                // A manifest alone is only half an answer: what the player does with a
                // source is search it, so the probe asks for one ordinary word.
                match probe_search(http, &self.base, &self.quiet_until).await {
                    Ok(n) => {
                        let suffix = if n == 1 { "result" } else { "results" };
                        Health::Ok(format!("{} · search works{}", name.unwrap_or_else(|| self.host()), (n > 0).then(|| format!(" · {n} {suffix}")).unwrap_or_default()))
                    }
                    Err(e) => {
                        let reason = format!("search failed: {e:#}");
                        if transport_failure(&e) { Health::Unreachable(reason) } else { Health::Rejected(reason) }
                    }
                }
            }
            Err(e) => {
                let reason = format!("{e:#}");
                // An auth-protected addon (or one that guards even its manifest) says
                // so in its answer; that's a verdict, not a server having a bad minute.
                if e.is::<AuthError>() {
                    return if transport_failure(&e) { Health::Unreachable(reason) } else { Health::Rejected(reason) };
                }
                match probe_search(http, &self.base, &self.quiet_until).await {
                    Ok(n) => Health::Ok(format!("No manifest · search works · {n} result{}", if n == 1 { "" } else { "s" })),
                    Err(e) => {
                        let search = format!("search failed: {e:#}");
                        let message = format!("{search} (manifest: {reason})");
                        if transport_failure(&e) { Health::Unreachable(message) } else { Health::Rejected(message) }
                    }
                }
            }
        }
    }
}

/// What the search probe asks for. Deliberately an ordinary word rather than a
/// nonsense string: an addon is entitled to nothing for a query nobody could match,
/// and a probe that reliably comes back empty proves less than one that usually
/// comes back full (BitChord's `PROBE_QUERY`).
const PROBE_QUERY: &str = "music";

/// What an addon that turned on BitChord's Android-only authentication means for a
/// desktop listener, worded once and reused everywhere (the Test line and the
/// playback log). BitChord addon auth checks a hardware-backed key only the official
/// BitChord Android app can hold, so no desktop client can pass it.
const AUTH_DESKTOP_MSG: &str = "requires BitChord Android authentication (only the official BitChord Android app can answer it)";

/// One `/search`, counted. Zero is a success — an addon holding nothing for one query
/// is answering correctly — so callers read the count, not emptiness.
async fn probe_search(http: &reqwest::Client, base: &str, quiet_until: &QuietUntil) -> anyhow::Result<usize> {
    let mut url = base_url_with(base, &["search"])?;
    url.query_pairs_mut().append_pair("q", PROBE_QUERY);
    let json = get_json(http, url.as_str(), REQUEST_TIMEOUT, quiet_until).await?;
    Ok(json.get("tracks").and_then(Value::as_array).map(|a| a.len()).unwrap_or(0))
}

/// `{base}/{segments…}`, for a base that may carry a query of its own — an install
/// link like `…/quality/manifest.json?sources=tidal` normalizes to a base ending in
/// `?sources=tidal`. Pasting the path on with `format!` would land it after the
/// query, where no server looks for it.
fn base_url_with(base: &str, segments: &[&str]) -> anyhow::Result<url::Url> {
    let mut url = url::Url::parse(base)?;
    {
        let mut path = url.path_segments_mut().map_err(|_| anyhow::anyhow!("that addon address can't take paths"))?;
        path.pop_if_empty();
        for segment in segments {
            path.push(segment);
        }
    }
    Ok(url)
}

/// The playback lookup. Each source searches and opens its stream at the
/// same time; with Atmos off the first usable (lossless) answer plays, and with Atmos on
/// the first Atmos rendition wins — otherwise the first lossless copy is held for a short
/// grace while the rest answer, then played. An upgrade keeps comparing lossless
/// answers until the deadline. `None` means no source returned a playable copy.
///
/// This speaks only the addon protocol. A link that also runs Monochrome is added to the
/// Monochrome list too (Settings › Add a custom provider asks a pasted link both ways),
/// so its Monochrome turn comes from its own row in the source list — asking every
/// Bitchord addon as Monochrome too would only put the same questions to servers that
/// answered as Bitchord addons.
pub(crate) async fn find(http: &reqwest::Client, sources: &[Arc<Client>], track: &crate::model::Track, atmos: bool, deadline: tokio::time::Instant, lookup: Lookup) -> Option<Stream> {
    let upgrade = matches!(lookup, Lookup::Upgrade { .. });
    let mut tasks = tokio::task::JoinSet::new();
    let (send, mut found) = tokio::sync::mpsc::unbounded_channel();
    for client in sources.iter().cloned() {
        let (http, track, send) = (http.clone(), track.clone(), send.clone());
        tasks.spawn(async move {
            match client.picks(&http, &track, atmos).await {
                Ok(picks) => open_first(&http, &client, &picks, atmos, lookup, deadline, &send).await,
                Err(e) => log::info!("hi-res {}: \"{}\": {e:#}", client.host(), track.title),
            }
        });
    }
    drop(send);
    let mut lossless: Option<Stream> = None;
    let mut held_at: Option<tokio::time::Instant> = None;
    loop {
        // Playback gives Atmos a short grace; an upgrade can keep comparing until
        // its full lookup budget expires because audio is already playing.
        let deadline = held_at.map_or(deadline, |t| (t + ATMOS_GRACE).min(deadline));
        let Ok(Some(s)) = tokio::time::timeout_at(deadline, found.recv()).await else { break };
        if s.info.atmos || (!upgrade && !atmos) {
            log::info!("hi-res: \"{}\" plays {} ({})", track.title, s.info.badge(), s.info.host);
            return Some(s);
        }
        if upgrade {
            if lossless.as_ref().is_none_or(|best| s.info.rung() > best.info.rung()) {
                lossless = Some(s);
            }
        } else {
            // No Atmos answer yet: keep the lossless copy in case none turns up.
            held_at.get_or_insert_with(tokio::time::Instant::now);
            lossless = lossless.or(Some(s));
        }
    }
    if let Some(s) = &lossless {
        log::info!("hi-res: \"{}\" plays {} ({})", track.title, s.info.badge(), s.info.host);
    }
    lossless
}

/// The best playable row of one source: Atmos rows first when the preference is on (a
/// miss leaves the first lossless copy held), then the plain rows. Asked one at a
/// time, the way the Android app walks its rows: each `/stream` mints a session on
/// the addon's own backend, and opening three together is three sessions standing
/// beside each other for a race two of them are about to lose — an origin that
/// proxies its own audio (rather than handing out a CDN link) exhausts under exactly
/// that burst, and 503s for everyone until its pool recovers.
async fn open_first(http: &reqwest::Client, client: &Arc<Client>, candidates: &[Candidate], atmos: bool, lookup: Lookup, deadline: tokio::time::Instant, found: &tokio::sync::mpsc::UnboundedSender<Stream>) {
    let rounds: &[bool] = if atmos { &[true, false] } else { &[false] };
    let rows: Vec<_> = rounds.iter().flat_map(|&round| candidates.iter().filter(move |c| !atmos || c.atmos == round).take(STREAM_TRIES)).collect();
    let upgrade = matches!(lookup, Lookup::Upgrade { .. });
    let mut sent_lossless = false;
    for (index, candidate) in rows.iter().enumerate() {
        // A quiet period belongs to the addon, not to this row. Spend only the
        // overall lookup budget waiting, including after a preceding row hit 429.
        if !matches!(tokio::time::timeout_at(deadline, wait_for_quiet(&client.quiet_until)).await, Ok(Ok(()))) {
            return;
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return;
        }
        // Reserve time for later rows while still minting only one at a time.
        let budget = (remaining / (rows.len() - index) as u32).min(STREAM_TIMEOUT);
        let attempt = client.open(http, candidate, atmos && candidate.atmos, lookup);
        match tokio::time::timeout(budget, attempt).await {
            Ok(Ok(Some(s))) if usable(&s) => {
                let done = s.info.atmos || (!atmos && !upgrade);
                if (done || upgrade || !sent_lossless) && found.send(s).is_err() {
                    return;
                }
                if done {
                    return;
                }
                sent_lossless = true;
            }
            Ok(Err(e)) => log::info!("hi-res {}: candidate {} skipped: {e:#}", client.host(), candidate.id),
            Err(_) => log::info!("hi-res {}: candidate {} timed out", client.host(), candidate.id),
            Ok(_) => {}
        }
    }
}

/// The chain is lossless or Atmos; a lossy copy from an addon isn't better than YouTube.
fn usable(stream: &Stream) -> bool {
    stream.info.atmos || stream.info.lossless()
}

/// Health-checks a pasted link, saved or not. A link that isn't http(s) can't be a
/// source, and is rejected before anything is asked of the network.
pub async fn probe_link(raw: &str, http: &reqwest::Client) -> Health {
    match Client::new(raw) {
        Some(client) => client.probe(http).await,
        None => Health::Rejected("That isn't an http(s) link, so it can't be a source.".to_owned()),
    }
}

/// The base URL with a trailing slash or a pasted `…/manifest.json` taken off.
/// A query string or fragment (e.g. `…/manifest.json?sources=tidal`)
/// no longer defeats the suffix check, and is preserved on the returned
/// base so per-request query params keep working.
pub fn normalize_base(raw: &str) -> String {
    let trimmed = raw.trim().trim_end_matches('/');
    // Split off any `?...` / `#...` tail before inspecting the last path
    // segment; otherwise `manifest.json?sources=…` never matches `.json`
    // and every endpoint built from the base 404s.
    let cut = trimmed.find(|c| c == '?' || c == '#').unwrap_or(trimmed.len());
    let (head, tail) = trimmed.split_at(cut);
    let last = head.rsplit('/').next().unwrap_or("");
    if last.to_ascii_lowercase().ends_with(".json") && head.contains("://") {
        format!("{}{}", head[..head.len() - last.len()].trim_end_matches('/'), tail)
    } else {
        trimmed.to_owned()
    }
}

fn host_of(url: &str) -> String {
    url::Url::parse(url).ok().and_then(|u| u.host_str().map(str::to_owned)).unwrap_or_default()
}

/// Whether the manifest turns on BitChord's addon authentication. The Android app
/// accepts `true`, `1`, `"true"` and `"1"`; anything else (or nothing) means the
/// addon works without it.
fn manifest_auth(json: &Value) -> bool {
    match json.get("bitchordAuth") {
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|n| n == 1.0),
        Some(Value::String(s)) => matches!(s.trim(), "true" | "1"),
        _ => false,
    }
}

fn settings(value: Option<&Value>) -> (Vec<(String, String)>, String) {
    let mut defaults = Vec::new();
    let mut quality = TIER.to_owned();
    let Some(list) = value.and_then(Value::as_array) else { return (defaults, quality) };
    for entry in list {
        let Some(key) = entry.get("key").and_then(Value::as_str).map(str::trim).filter(|k| !k.is_empty()) else { continue };
        let Some(default) = entry.get("default").and_then(query_value) else { continue };
        if key.eq_ignore_ascii_case("quality") {
            let options: Vec<String> = entry.get("options").and_then(Value::as_array).map(|a| a.iter().filter_map(|o| o.get("value").and_then(query_value)).collect()).unwrap_or_default();
            quality = match_tier(&options);
        } else {
            defaults.push((key.to_owned(), default));
        }
    }
    (defaults, quality)
}

fn query_value(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()).filter(|s| !s.is_empty()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// The option that best answers a lossless request.
fn match_tier(options: &[String]) -> String {
    if options.is_empty() {
        return TIER.to_owned();
    }
    if let Some(exact) = options.iter().find(|o| o.eq_ignore_ascii_case(TIER)) {
        return exact.clone();
    }
    const WORDS: [&str; 7] = ["lossless", "flac", "hifi", "hi-res", "hires", "max", "best"];
    options.iter().find(|o| WORDS.iter().any(|w| o.to_ascii_lowercase().contains(w))).cloned().unwrap_or_else(|| options[0].clone())
}

fn candidate(v: &Value) -> Option<Candidate> {
    let id = v.get("id").and_then(Value::as_str).unwrap_or("").trim();
    let title = v.get("title").and_then(Value::as_str).unwrap_or("").trim();
    if id.is_empty() || title.is_empty() {
        return None;
    }
    Some(Candidate {
        id: id.to_owned(),
        title: title.to_owned(),
        artist: v.get("artist").and_then(Value::as_str).unwrap_or("").trim().to_owned(),
        duration: seconds(v.get("duration").unwrap_or(&Value::Null)),
        quality: v.get("audioQuality").and_then(Value::as_str).unwrap_or("").trim().to_owned(),
        format: v.get("format").and_then(Value::as_str).unwrap_or("").trim().to_owned(),
        atmos: json_is_atmos(v),
        stream_url: v.get("streamURL").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned),
    })
}

fn seconds(v: &Value) -> Option<u32> {
    if let Some(n) = v.as_f64() {
        return (n > 0.0).then_some(n as u32);
    }
    let text = v.as_str()?.trim();
    if let Some((m, s)) = text.split_once(':') {
        let m: u32 = m.parse().ok()?;
        let s: f64 = s.parse().ok()?;
        return Some(m * 60 + s as u32);
    }
    text.parse::<f64>().ok().filter(|n| *n > 0.0).map(|n| n as u32)
}

fn json_size(v: &Value) -> u64 {
    for key in ["fileSize", "filesize", "size", "contentLength", "content_length", "length", "bytes"] {
        if let Some(n) = v.get(key) {
            if let Some(u) = n.as_u64().filter(|u| *u > 0) {
                return u;
            }
            if let Some(f) = n.as_f64().filter(|f| *f > 0.0) {
                return f as u64;
            }
            if let Some(s) = n.as_str().and_then(|s| s.trim().parse::<u64>().ok()).filter(|u| *u > 0) {
                return s;
            }
        }
    }
    0
}

/// A row or stream that says it's Dolby Atmos, under any of the names addons use for it.
fn json_is_atmos(v: &Value) -> bool {
    const KEYS: [&str; 7] = ["audioMode", "audioModes", "quality", "streamQuality", "audioQuality", "format", "codec"];
    KEYS.iter().any(|key| v.get(*key).is_some_and(value_is_atmos))
}

fn value_is_atmos(v: &Value) -> bool {
    match v {
        Value::String(s) => {
            let s = s.to_ascii_lowercase();
            ["atmos", "dolby", "ac-3", "ac3", "eac3", "ec-3"].iter().any(|w| s.contains(w))
        }
        Value::Array(items) => items.iter().any(value_is_atmos),
        _ => false,
    }
}

fn encrypted(v: &Value) -> bool {
    match v.get("encrypted") {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => {
            let s = s.trim().to_ascii_lowercase();
            !s.is_empty() && s != "false" && s != "none" && s != "0"
        }
        Some(Value::Number(n)) => n.as_f64().is_some_and(|n| n != 0.0),
        _ => false,
    }
}

fn is_manifest(transport: &str) -> bool {
    matches!(transport.trim().to_ascii_lowercase().as_str(), "hls" | "m3u8" | "dash" | "mpd" | "application/x-mpegurl" | "application/vnd.apple.mpegurl" | "application/dash+xml")
}

fn playable_url(url: &str) -> bool {
    url::Url::parse(url).is_ok_and(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some_and(|h| !h.is_empty()))
}

/// The container/codec rodio will be handed.
fn format_of(codec: &str, container: &str, mime: &str, url: &str, label: &str) -> Option<Format> {
    let codec = codec.to_ascii_lowercase();
    let container = container.to_ascii_lowercase();
    let mime = mime.to_ascii_lowercase();
    let ext = url.split(['?', '#']).next().unwrap_or(url).rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    let all = format!("{codec} {container} {mime} {ext}");
    // Symphonia has no AC-3 family decoder, so an Atmos rendition in E-AC-3/TrueHD isn't
    // playable and the next copy is tried instead.
    if ["ac-3", "ac3", "eac-3", "eac3", "ec-3", "truehd", "mlp"].iter().any(|w| all.contains(w)) {
        return None;
    }
    let file = |hint: &'static str, mime: &str| Format::File { hint, mime: Some(mime.to_owned()) };
    if all.contains("flac") {
        return Some(file("flac", "audio/flac"));
    }
    if (all.contains("alac") || ext == "m4a" || ext == "mp4" || mime.contains("mp4")) && has_lossless_word(label) {
        return Some(file("m4a", "audio/mp4"));
    }
    if all.contains("wav") && !all.contains("wave") {
        return Some(file("wav", "audio/wav"));
    }
    if all.contains("mp3") || mime.contains("mpeg") {
        return Some(file("mp3", "audio/mpeg"));
    }
    if all.contains("vorbis") || ext == "ogg" || mime.contains("ogg") {
        return Some(file("ogg", "audio/ogg"));
    }
    if all.contains("opus") || ext == "webm" || mime.contains("webm") {
        return Some(Format::Opus);
    }
    if all.contains("aac") || all.contains("m4a") || ext == "m4a" {
        return Some(file("m4a", "audio/mp4"));
    }
    if has_lossless_word(label) {
        // A lossless row that names no codec is FLAC in practice.
        return Some(file("flac", "audio/flac"));
    }
    None
}

fn codec_name(codec: &str, format: &Format) -> String {
    let codec = codec.trim();
    if !codec.is_empty() {
        return codec.to_ascii_uppercase();
    }
    match format {
        Format::Aac => "AAC",
        Format::Opus => "OPUS",
        Format::Pcm { .. } => "PCM",
        Format::File { hint, .. } => match *hint {
            "flac" => "FLAC",
            "wav" => "WAV",
            "mp3" => "MP3",
            "ogg" => "VORBIS",
            _ => "ALAC",
        },
    }
    .to_owned()
}

fn has_lossless_word(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    ["lossless", "flac", "hifi", "hi-res", "hires", "master", "24bit", "24-bit"].iter().any(|w| text.contains(w))
}

/// The number just before `unit` in free text (`24-bit`, `96 kHz`, `1411kbps`).
fn number_before(text: &str, unit: &str) -> Option<f64> {
    let lower = text.to_ascii_lowercase();
    let mut from = 0;
    while let Some(found) = lower[from..].find(unit) {
        let at = from + found;
        let mut end = at;
        // `24-bit`, `96 kHz` and `1411 kbps` all read the same.
        while end > 0 && matches!(lower.as_bytes()[end - 1], b' ' | b'-') {
            end -= 1;
        }
        let mut start = end;
        while start > 0 && (lower.as_bytes()[start - 1].is_ascii_digit() || lower.as_bytes()[start - 1] == b'.') {
            start -= 1;
        }
        if start < end
            && let Ok(n) = lower[start..end].trim_matches('.').parse::<f64>()
            && n > 0.0
        {
            return Some(n);
        }
        from = at + unit.len();
        if from >= lower.len() {
            break;
        }
    }
    None
}

// ── Matching ─────────────────────────────────────────────────────────────

fn rank(mut candidates: Vec<Candidate>, title: &str, artist: &str, duration: Option<u32>, prefer_atmos: bool) -> Vec<Candidate> {
    let mut scored: Vec<(Candidate, i32)> = candidates.drain(..).filter_map(|c| score(&c, title, artist, duration, prefer_atmos).map(|s| (c, s))).collect();
    scored.sort_by(|a, b| b.1.cmp(&a.1));
    scored.into_iter().map(|(c, _)| c).collect()
}

fn score(candidate: &Candidate, title: &str, artist: &str, duration: Option<u32>, prefer_atmos: bool) -> Option<i32> {
    let wanted = title_parts(title, artist);
    let got = title_parts(&candidate.title, &candidate.artist);
    if wanted.0.is_empty() || wanted.0 != got.0 || wanted.1 != got.1 {
        return None;
    }
    let target_artists = artist_names(artist);
    let candidate_artists = artist_names(&candidate.artist);
    // A copy that doesn't name the artist is not the same song: never swap the
    // listener's stream for it.
    if !target_artists.is_empty()
        && (candidate_artists.is_empty() || !target_artists.iter().any(|w| candidate_artists.iter().any(|g| same_artist(w, g))))
    {
        return None;
    }
    let artist_score = match (target_artists.is_empty() || candidate_artists.is_empty(), target_artists == candidate_artists) {
        (true, _) => 0,
        (false, true) => 30,
        (false, false) => 20,
    };
    let duration_score = match (duration, candidate.duration) {
        (Some(wanted), Some(got)) => (20 - wanted.abs_diff(got) as i32).max(0),
        _ => 0,
    };
    // With Atmos preferred, a marked Atmos copy always outranks a plain one: the
    // listener asked for it, and every candidate here is already the same song.
    let atmos_score = if prefer_atmos && candidate.atmos { 1000 } else { 0 };
    Some(100 + artist_score + duration_score + atmos_score)
}

/// The title with the packaging taken off, version markers kept.
fn searchable_title(title: &str, artist: &str) -> String {
    let (words, versions) = title_parts(title, artist);
    let mut out = words;
    for version in versions {
        out.push(' ');
        out.push_str(&version);
    }
    out.trim().to_owned()
}

fn primary_artist(artist: &str) -> String {
    artist.to_ascii_lowercase().split([',', '&', '/', ';', '·', '|']).next().unwrap_or("").trim().to_owned()
}

type TitleParts = (String, Vec<String>);

/// A title split into its core words and its version markers.
fn title_parts(raw: &str, artist: &str) -> TitleParts {
    let mut versions = BTreeSet::new();
    let mut text = raw.to_ascii_lowercase().replace('&', " and ");
    for _ in 0..3 {
        let Some((start, end)) = bracket_span(&text) else { break };
        classify(&text[start + 1..end], &mut versions);
        text.replace_range(start..=end, " ");
    }
    if let Some(at) = text.find(['(', '[']) {
        classify(&text[at..], &mut versions);
        text.truncate(at);
    }
    for _ in 0..3 {
        let Some(at) = text.find(" - ") else { break };
        let (head, tail) = (text[..at].to_owned(), text[at + 3..].to_owned());
        if is_artist_name(&head, artist) {
            classify(&head, &mut versions);
            text = tail;
        } else if is_artist_name(&tail, artist) {
            classify(&tail, &mut versions);
            text = head;
        } else {
            classify(&tail, &mut versions);
            text = head;
        }
    }
    for marker in [" feat.", " feat ", " ft.", " ft ", " featuring ", " with "] {
        if let Some(at) = text.find(marker) {
            text.truncate(at);
        }
    }
    let mut words: Vec<String> = text.split([' ', '.', '·']).map(clean).filter(|w| !w.is_empty() && w != "and").collect();
    while words.len() > 1 && matches!(words.last().map(String::as_str), Some("song" | "songs" | "video" | "audio" | "lyrics" | "official" | "full" | "hd" | "hq" | "4k" | "mp3" | "ost" | "soundtrack")) {
        words.pop();
    }
    (words.join(" "), versions.into_iter().collect())
}

fn bracket_span(text: &str) -> Option<(usize, usize)> {
    let open = text.find(['(', '['])?;
    let close = text[open..].find([')', ']'])? + open;
    Some((open, close))
}

fn classify(segment: &str, versions: &mut BTreeSet<String>) {
    let words: Vec<String> = segment.split([' ', '.', '·']).map(clean).filter(|w| !w.is_empty()).collect();
    if words.is_empty() {
        return;
    }
    let joined = words.join("");
    if matches!(joined.as_str(), "albumversion" | "originalversion" | "originalmix" | "singleversion" | "radioversion" | "radioedit" | "stereoversion" | "monoversion" | "studioversion" | "fullversion" | "standardversion" | "explicitversion" | "deluxeversion" | "originaltrack") {
        return;
    }
    for word in words {
        if VERSION_WORDS.contains(&word.as_str()) {
            versions.insert(word);
        }
    }
}

fn is_artist_name(text: &str, artist: &str) -> bool {
    if artist.is_empty() {
        return false;
    }
    let words: Vec<String> = text.split([' ', '.', '·']).map(clean).filter(|w| !w.is_empty()).collect();
    if words.is_empty() {
        return false;
    }
    let credited: Vec<String> = artist.to_ascii_lowercase().split([' ', '.', '·']).map(clean).filter(|w| !w.is_empty()).collect();
    words.iter().all(|w| credited.contains(w))
}

fn artist_names(value: &str) -> Vec<Vec<String>> {
    let normalized = value.to_ascii_lowercase().trim().to_owned();
    if normalized.is_empty() || matches!(normalized.as_str(), "unknown" | "unknown artist" | "unknown artist(s)") {
        return Vec::new();
    }
    normalized
        .split([',', '&', '/', ';', '·', '|'])
        .map(|name| {
            let mut parts: Vec<String> = name.split([' ', '.']).map(clean).filter(|w| w.len() > 1).collect();
            if parts.is_empty() {
                parts = vec![name.trim().to_owned()];
            }
            parts
        })
        .collect::<Vec<Vec<String>>>()
        .into_iter()
        .filter(|parts| parts.iter().any(|w| !w.is_empty()))
        .collect()
}

fn same_artist(first: &[String], second: &[String]) -> bool {
    if contains_words(first, second) || contains_words(second, first) {
        return true;
    }
    // Addons sometimes drop a middle name; accept only when every token of the shorter
    // credited name appears in order in the longer one.
    ordered_subset(first, second) || ordered_subset(second, first)
}

fn ordered_subset(shorter: &[String], longer: &[String]) -> bool {
    if shorter.is_empty() || shorter.len() >= longer.len() {
        return false;
    }
    let mut at = 0;
    for word in shorter {
        let Some(pos) = longer[at..].iter().position(|w| w == word) else { return false };
        at += pos + 1;
    }
    true
}

fn contains_words(outer: &[String], inner: &[String]) -> bool {
    !inner.is_empty() && inner.len() <= outer.len() && (0..=outer.len() - inner.len()).any(|start| outer[start..start + inner.len()] == *inner)
}

fn clean(word: &str) -> String {
    word.chars().filter(|c| c.is_ascii_alphanumeric()).collect()
}

const VERSION_WORDS: &[&str] = &[
    "remix", "remixes", "rmx", "refix", "flip", "bootleg", "mashup", "medley", "live", "concert", "unplugged", "acoustic", "instrumental", "karaoke", "vocals", "vocal",
    "acapella", "acappella", "backing", "stems", "stem", "cover", "demo", "reprise", "remake", "rework", "extended", "edit", "version", "mix", "dub", "vip", "session",
    "sessions", "reverb", "nightcore", "lofi", "orchestral", "symphonic", "part", "pt", "chapter",
];

// ── HTTP ─────────────────────────────────────────────────────────────────

async fn wait_for_quiet(quiet_until: &QuietUntil) -> anyhow::Result<()> {
    loop {
        // A sibling request may extend the quiet period while this one sleeps.
        let until = *quiet_until.lock().await;
        let Some(until) = until else { anyhow::bail!("the addon requested a quiet period beyond the clock range") };
        if tokio::time::Instant::now() >= until {
            return Ok(());
        }
        tokio::time::sleep_until(until).await;
    }
}

async fn get_json(http: &reqwest::Client, url: &str, timeout: Duration, quiet_until: &QuietUntil) -> anyhow::Result<Value> {
    // A 429 is waited out rather than surfaced, for the Android app's reason: the
    // callers above this read a failure as "this addon has nothing", which is a
    // different and wrong conclusion from "this addon is busy". The wait lands in
    // `quiet_until` — per-addon, so a search that drew the 429 holds back the
    // stream lookup queued behind it instead of re-tripping the limiter with it.
    let mut attempt = 0u32;
    loop {
        // Twice at most for the agent: a refusal is asked again under the release
        // line's agent, which the addon does answer, and a second refusal is the
        // addon's real answer to us.
        let mut refused = None;
        for agent_attempt in 0..2 {
            wait_for_quiet(quiet_until).await?;
            let response = http.get(url).timeout(timeout).header("Accept", "application/json").header("User-Agent", agent()).send().await.map_err(addon_err)?;
            let code = response.status();
            if code == reqwest::StatusCode::NOT_FOUND {
                anyhow::bail!("not found");
            }
            if code.is_success() {
                let bytes = response.bytes().await.map_err(addon_err)?;
                anyhow::ensure!(!bytes.is_empty(), "the addon answered with nothing");
                return Ok(serde_json::from_slice(&bytes)?);
            }
            // A refusal of this app's own name: the release line's agent is asked once,
            // and kept for the rest of the session if it answers. Before anything that
            // reads the body — a 403 is the agent, not the link.
            if code == reqwest::StatusCode::FORBIDDEN {
                if agent_attempt == 0 {
                    use_release_agent();
                    continue;
                }
                anyhow::bail!("the addon doesn't answer this app");
            }
            if code == reqwest::StatusCode::TOO_MANY_REQUESTS {
                // Even the final 429 must hold back the next request to this addon.
                // Retry-After is in seconds; without it, double the wait per retry.
                // Only the generated backoff is capped; the lookup deadline still
                // bounds playback's wait when a server asks for a longer pause.
                let retry_after = response.headers().get(reqwest::header::RETRY_AFTER).and_then(|v| v.to_str().ok()).and_then(|s| retry_after(s, std::time::SystemTime::now()));
                let backoff = THROTTLE_BASE * (1 << attempt);
                let wait = retry_after.unwrap_or(backoff.min(THROTTLE_CAP));
                let mut quiet = quiet_until.lock().await;
                *quiet = match (*quiet, tokio::time::Instant::now().checked_add(wait)) {
                    (Some(previous), Some(next)) => Some(previous.max(next)),
                    _ => None,
                };
            }
            // BitChord addon auth answers 401 (503 while its verifier can't load keys)
            // with a JSON body naming itself; say what that means rather than "HTTP 401".
            let body = response.text().await.unwrap_or_default();
            if let Some(msg) = auth_answer(&body) {
                return Err(AuthError { status: code, message: msg }.into());
            }
            if code.is_server_error() {
                // A server having a bad minute (the Android app's AddonUnavailable), not a
                // verdict on the addon: the caller moves to the next row or the next
                // source, and the next track asks again — nothing here holds a grudge.
                anyhow::bail!("the addon is having a bad minute (HTTP {})", code.as_u16());
            }
            // Other addons guard themselves behind a sign-in on their own site and point
            // at it ("login":"/auth/discord/login"): they work once the listener has
            // signed in there and pasted the personal link that comes out of it.
            if let Some(login_url) = login_url(url, &body) {
                anyhow::bail!("this addon needs your own sign-in link: open {login_url} in a browser, sign in, and paste the link it gives you here");
            }
            refused = Some(code);
            break;
        }
        let Some(code) = refused else { anyhow::bail!("the addon doesn't answer this app"); };
        if code == reqwest::StatusCode::TOO_MANY_REQUESTS && attempt < THROTTLE_WAITS {
            attempt += 1;
            continue;
        }
        anyhow::bail!("HTTP {}", code.as_u16());
    }
}

/// Addon links carry a token: a request failure names its kind, never the URL the
/// reqwest error would print.
fn addon_err(e: reqwest::Error) -> anyhow::Error {
    if e.is_timeout() {
        anyhow::anyhow!("the addon timed out")
    } else if e.is_connect() {
        anyhow::anyhow!("the addon didn't connect")
    } else {
        anyhow::anyhow!("the addon request failed")
    }
}

/// Whether a mapped addon failure was the transport (timeout, connect) or a 5xx —
/// the addon's own "bad minute" word for it — rather than the addon itself: those
/// are worth another try, and the probe says Unreachable rather than Rejected, so
/// a transient outage is not painted as a configuration error (the Android app's
/// AddonUnavailable, rendered separately for the same reason).
fn transport_failure(error: &anyhow::Error) -> bool {
    if let Some(auth) = error.downcast_ref::<AuthError>() {
        return auth.status.is_server_error();
    }
    let reason = error.to_string();
    reason.contains("timed out") || reason.contains("didn't connect") || reason.contains("request failed") || reason.contains("bad minute")
}

fn retry_after(value: &str, now: std::time::SystemTime) -> Option<Duration> {
    let value = value.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let date = httpdate::parse_http_date(value).ok()?;
    Some(date.duration_since(now).unwrap_or_default())
}

#[derive(Debug)]
struct AuthError {
    status: reqwest::StatusCode,
    message: String,
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{AUTH_DESKTOP_MSG} {} (HTTP {})", self.message, self.status.as_u16())
    }
}

impl std::error::Error for AuthError {}

/// The reason a `401 {"error":"bitchord_auth",…}` answer gave, for the log line.
fn auth_answer(body: &str) -> Option<String> {
    let json: Value = serde_json::from_str(body).ok()?;
    if json.get("error").and_then(Value::as_str) != Some("bitchord_auth") {
        return None;
    }
    let code = json.get("code").and_then(Value::as_str).unwrap_or("");
    Some(match code {
        "expired" | "stale_proof" => "the addon's proof went stale".to_owned(),
        "unavailable" => "the addon's verifier couldn't load BitChord's keys".to_owned(),
        _ => "the addon refused this client".to_owned(),
    })
}

/// The sign-in address a guarded addon points at ("login":"/auth/discord/login"),
/// resolved against the request that was refused.
fn login_url(request: &str, body: &str) -> Option<String> {
    let json: Value = serde_json::from_str(body).ok()?;
    let login = json.get("login").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())?;
    url::Url::parse(request).ok()?.join(login).ok().map(String::from)
}

/// How many bytes the stream holds, from a HEAD or a one-byte range request.
async fn content_size(http: &reqwest::Client, url: &str) -> Option<u64> {
    if let Ok(response) = http.head(url).timeout(PROBE_TIMEOUT).header("User-Agent", agent()).send().await
        && response.status().is_success()
        && let Some(size) = response.content_length().filter(|n| *n > 0)
    {
        return Some(size);
    }
    let response = http.get(url).timeout(PROBE_TIMEOUT).header("User-Agent", agent()).header("Range", "bytes=0-0").header("Accept-Encoding", "identity").send().await.ok()?;
    if let Some(range) = response.headers().get(reqwest::header::CONTENT_RANGE).and_then(|v| v.to_str().ok())
        && let Some(total) = range.rsplit('/').next().and_then(|s| s.trim().parse::<u64>().ok())
    {
        return Some(total);
    }
    response.content_length().filter(|n| *n > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    async fn serve<F, R>(respond: F) -> (String, tokio::task::JoinHandle<()>)
    where
        F: Fn(String, &str) -> R + Send + 'static,
        R: Future<Output = (u16, String)> + Send + 'static,
    {
        serve_with_headers(move |path, origin| {
            let response = respond(path, origin);
            async move {
                let (status, body) = response.await;
                (status, "", body)
            }
        }).await
    }

    async fn serve_with_headers<F, R>(respond: F) -> (String, tokio::task::JoinHandle<()>)
    where
        F: Fn(String, &str) -> R + Send + 'static,
        R: Future<Output = (u16, &'static str, String)> + Send + 'static,
    {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let origin = base.clone();
        let server = tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut reader = BufReader::new(&mut socket);
                let mut request = String::new();
                if reader.read_line(&mut request).await.unwrap_or(0) == 0 {
                    continue;
                }
                let mut header = String::new();
                while reader.read_line(&mut header).await.unwrap() > 0 && header != "\r\n" {
                    header.clear();
                }
                let head = request.starts_with("HEAD ");
                let path = request.split_whitespace().nth(1).unwrap().split('?').next().unwrap().to_owned();
                let response = respond(path, &origin);
                tokio::spawn(async move {
                    let (status, headers, body) = response.await;
                    let response = format!("HTTP/1.1 {status} Test\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), if head { "" } else { &body });
                    let _ = socket.write_all(response.as_bytes()).await;
                });
            }
        });
        (base, server)
    }

    #[tokio::test]
    async fn manifest_rendition_is_deferred_unless_needed_for_startup() {
        let (media_hit, mut media_requests) = tokio::sync::mpsc::unbounded_channel();
        let (media, media_server) = serve(move |_, _| {
            media_hit.send(()).unwrap();
            std::future::pending()
        }).await;
        let (base, server) = serve(move |path, _| {
            let response = match path.as_str() {
                "/manifest.json" => r#"{"id":"test"}"#.to_owned(),
                "/search" => r#"{"tracks":[{"id":"song","title":"Test Song","artist":"Test Artist"}]}"#.to_owned(),
                "/stream/song" => serde_json::json!({"url":format!("{media}/test.mpd"),"format":"dash"}).to_string(),
                _ => panic!("unexpected addon request: {path}"),
            };
            std::future::ready((200, response))
        }).await;
        let client = Arc::new(Client::new(&base).unwrap());
        let track = crate::model::Track {
            title: "Test Song".into(),
            artists: vec![crate::model::Link { name: "Test Artist".into(), id: None }],
            ..Default::default()
        };
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        for lookup in [Lookup::Prefetch, Lookup::Playback] {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
            assert!(find(&http, std::slice::from_ref(&client), &track, false, deadline, lookup).await.is_none());
            assert!(media_requests.try_recv().is_err());
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
        let lookup = find(&http, std::slice::from_ref(&client), &track, false, deadline, Lookup::Startup);
        let (stream, requested) = tokio::time::timeout(Duration::from_secs(3), async {
            tokio::join!(lookup, media_requests.recv())
        }).await.unwrap();
        assert_eq!(requested, Some(()));
        assert!(stream.is_none(), "the rendition never answers");
        server.abort();
        media_server.abort();
    }

    #[tokio::test]
    async fn probe_distinguishes_search_outages_from_rejection() {
        let (base, server) = serve(|path, _| std::future::ready(match path.as_str() {
            "/offline/manifest.json" | "/auth/manifest.json" | "/verifier/manifest.json" | "/login/manifest.json" | "/login-outage/manifest.json" => (200, r#"{"id":"test"}"#.to_owned()),
            "/absent/manifest.json" | "/search-only/manifest.json" => (404, String::new()),
            "/offline/search" | "/absent/search" => (503, String::new()),
            "/login/search" => (401, r#"{"login":"/sign-in"}"#.to_owned()),
            "/login-outage/search" => (503, r#"{"login":"/sign-in"}"#.to_owned()),
            "/search-only/search" => (200, r#"{"tracks":[{}]}"#.to_owned()),
            "/auth/search" | "/locked/manifest.json" => (401, r#"{"error":"bitchord_auth","code":"expired"}"#.to_owned()),
            "/verifier/search" | "/verifier-manifest/manifest.json" => (503, r#"{"error":"bitchord_auth","code":"unavailable"}"#.to_owned()),
            _ => panic!("unexpected addon request: {path}"),
        })).await;
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        for path in ["offline", "absent"] {
            assert!(matches!(Client::new(&format!("{base}/{path}")).unwrap().probe(&http).await, Health::Unreachable(reason) if reason.contains("HTTP 503")));
        }
        assert!(matches!(Client::new(&format!("{base}/search-only")).unwrap().probe(&http).await, Health::Ok(reason) if reason.contains("1 result")));
        for path in ["auth", "locked"] {
            assert!(matches!(Client::new(&format!("{base}/{path}")).unwrap().probe(&http).await, Health::Rejected(reason) if reason.contains("HTTP 401") && reason.contains("proof went stale")));
        }
        for path in ["verifier", "verifier-manifest"] {
            assert!(matches!(Client::new(&format!("{base}/{path}")).unwrap().probe(&http).await, Health::Unreachable(reason) if reason.contains("HTTP 503") && reason.contains("verifier couldn't load")));
        }
        assert!(matches!(Client::new(&format!("{base}/login")).unwrap().probe(&http).await, Health::Rejected(reason) if reason.contains("sign-in link")));
        assert!(matches!(Client::new(&format!("{base}/login-outage")).unwrap().probe(&http).await, Health::Unreachable(reason) if reason.contains("HTTP 503")));
        server.abort();
    }

    #[tokio::test]
    async fn retry_after_outlasts_the_playback_deadline() {
        let (base, server) = serve_with_headers(|_, _| std::future::ready((429, "Retry-After: 60\r\n", String::new()))).await;
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        let started = tokio::time::Instant::now();
        let quiet = tokio::sync::Mutex::new(Some(started));
        // Cancellation of the request must leave its server-supplied delay intact.
        assert!(tokio::time::timeout(Duration::from_secs(1), get_json(&http, &base, REQUEST_TIMEOUT, &quiet)).await.is_err());
        assert!(quiet.lock().await.unwrap() >= started + Duration::from_secs(60));
        server.abort();
    }

    #[tokio::test]
    async fn atmos_lookup_keeps_lossless_when_the_next_row_stalls() {
        let (asked, mut requests) = tokio::sync::mpsc::unbounded_channel();
        let (base, server) = serve(move |path, _| {
            let asked = asked.clone();
            async move {
                let body = match path.as_str() {
                    "/manifest.json" => r#"{"id":"test"}"#,
                    "/search" => r#"{"tracks":[{"id":"first","title":"Test Song","artist":"Test Artist"},{"id":"slow","title":"Test Song","artist":"Test Artist"}]}"#,
                    "/stream/first" => r#"{"url":"https://example.invalid/first.flac","format":"flac","size":100}"#,
                    "/stream/slow" => {
                        asked.send(()).unwrap();
                        return std::future::pending().await;
                    }
                    _ => panic!("unexpected addon request: {path}"),
                };
                (200, body.to_owned())
            }
        }).await;
        let client = Arc::new(Client::new(&base).unwrap());
        let track = crate::model::Track {
            title: "Test Song".into(),
            artists: vec![crate::model::Link { name: "Test Artist".into(), id: None }],
            ..Default::default()
        };
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
        let stream = find(&http, &[client], &track, true, deadline, Lookup::Playback).await.unwrap();
        assert_eq!(stream.url, "https://example.invalid/first.flac");
        assert!(requests.try_recv().is_ok());
        server.abort();
    }

    #[test]
    fn retry_after_accepts_http_dates_and_delta_seconds() {
        let now = std::time::UNIX_EPOCH + Duration::from_secs(784111777);
        let date = httpdate::fmt_http_date(now + Duration::from_secs(60));
        assert_eq!(retry_after(&date, now), Some(Duration::from_secs(60)));
        assert_eq!(retry_after(&date, now + Duration::from_secs(61)), Some(Duration::ZERO));
        assert_eq!(retry_after("60", now), Some(Duration::from_secs(60)));
        assert_eq!(retry_after("invalid", now), None);
    }

    #[tokio::test]
    async fn stalled_first_row_leaves_time_to_compare_later_upgrade_rows() {
        let (base, server) = serve(|path, _| async move {
            let body = match path.as_str() {
                "/manifest.json" => r#"{"id":"test"}"#,
                "/stream/slow" => return std::future::pending().await,
                "/stream/cd" => r#"{"url":"https://example.invalid/cd.flac","size":100,"bitDepth":16}"#,
                "/stream/hires" => r#"{"url":"https://example.invalid/hires.flac","size":100,"bitDepth":24}"#,
                _ => panic!("unexpected addon request: {path}"),
            };
            (200, body.to_owned())
        }).await;
        let client = Arc::new(Client::new(&base).unwrap());
        let rows = ["slow", "cd", "hires"].map(|id| candidate(&serde_json::json!({"id":id,"title":"Test Song"})).unwrap());
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        for atmos in [false, true] {
            let (send, mut answers) = tokio::sync::mpsc::unbounded_channel();
            let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
            tokio::time::timeout_at(deadline, open_first(&http, &client, &rows, atmos, Lookup::Upgrade { fresh: true }, deadline, &send)).await.unwrap();
            assert_eq!(answers.try_recv().unwrap().info.rung(), 1);
            assert_eq!(answers.try_recv().unwrap().info.rung(), 2);
            assert!(answers.try_recv().is_err());
        }
        server.abort();
    }

    #[tokio::test]
    async fn quiet_period_extensions_do_not_spend_the_first_rows_request_budget() {
        let (sent, mut requests) = tokio::sync::mpsc::unbounded_channel();
        let (base, server) = serve(move |path, _| std::future::ready(match path.as_str() {
            "/manifest.json" => (200, r#"{"id":"test"}"#.to_owned()),
            "/stream/first" => {
                sent.send(tokio::time::Instant::now()).unwrap();
                (200, r#"{"url":"https://example.invalid/first.flac","size":100}"#.to_owned())
            }
            "/stream/second" | "/stream/third" => (404, String::new()),
            _ => panic!("unexpected addon request: {path}"),
        })).await;
        let client = Arc::new(Client::new(&base).unwrap());
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        client.manifest(&http).await.unwrap();
        let rows = ["first", "second", "third"].map(|id| candidate(&serde_json::json!({"id":id,"title":"Test Song"})).unwrap());
        let now = tokio::time::Instant::now();
        *client.quiet_until.lock().await = Some(now + Duration::from_millis(150));
        let deadline = now + Duration::from_secs(1);
        let (send, mut answers) = tokio::sync::mpsc::unbounded_channel();
        let find = open_first(&http, &client, &rows, false, Lookup::Playback, deadline, &send);
        tokio::pin!(find);
        tokio::select! {
            biased;
            _ = &mut find => panic!("lookup finished during the quiet period"),
            _ = std::future::ready(()) => {}
        }
        let extended = now + Duration::from_millis(500);
        *client.quiet_until.lock().await = Some(extended);
        tokio::time::timeout_at(deadline, find).await.unwrap();
        assert_eq!(answers.try_recv().unwrap().url, "https://example.invalid/first.flac");
        assert!(requests.try_recv().unwrap() >= extended);
        server.abort();
    }

    #[tokio::test]
    async fn download_completion_survives_probe_and_whole_file_failures() {
        use std::sync::atomic::AtomicUsize;
        use crate::stream::{self, Media, Ranging, Refresh, Shared};

        let whole = AtomicUsize::new(0);
        let (started, mut starts) = tokio::sync::mpsc::unbounded_channel();
        let (base, server) = serve(move |path, _| {
            let response = match path.as_str() {
                "/reject" => Some((403, String::new())),
                "/whole" if whole.fetch_add(1, Ordering::Relaxed) == 0 => Some((200, "data".to_owned())),
                "/whole" => Some((500, String::new())),
                "/stall" => {
                    started.send(()).unwrap();
                    None
                }
                _ => panic!("unexpected media request: {path}"),
            };
            async move {
                match response {
                    Some(value) => value,
                    None => std::future::pending().await,
                }
            }
        }).await;
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        for path in ["reject", "whole", "stall"] {
            let shared = Shared::new(4, 0, None, Format::Opus, None, false);
            let refresh = Refresh { once: true, callback: Arc::new(|| Box::pin(async { anyhow::bail!("no fresh URL") })) };
            let task = tokio::spawn(stream::download(http.clone(), Media::File(format!("{base}/{path}")), agent().to_owned(), shared.clone(), Some(refresh), Ranging::Header));
            if path == "stall" {
                tokio::time::timeout(Duration::from_secs(1), starts.recv()).await.unwrap().unwrap();
                shared.cancel();
                task.abort();
                assert!(task.await.unwrap_err().is_cancelled());
            } else {
                task.await.unwrap();
                assert!(shared.error().is_some());
            }
            tokio::time::timeout(Duration::from_millis(250), shared.finished()).await.expect(path);
        }
        server.abort();
    }

    #[tokio::test]
    async fn cached_url_rejection_refreshes_once_on_whole_download() {
        use std::sync::atomic::AtomicUsize;
        use crate::stream::{self, Media, Ranging, Refresh, Shared};

        let mints = Arc::new(AtomicUsize::new(0));
        let old_requests = Arc::new(AtomicUsize::new(0));
        let (base, server) = serve({
            let (mints, old_requests) = (mints.clone(), old_requests.clone());
            move |path, origin| {
                std::future::ready(match path.as_str() {
                    "/manifest.json" => (200, r#"{"id":"test"}"#.to_owned()),
                    "/stream/song" => {
                        let url = match mints.fetch_add(1, Ordering::SeqCst) {
                            0 => return std::future::ready((404, String::new())),
                            1 => format!("{origin}/old.flac"),
                            _ => format!("{origin}/fresh.flac"),
                        };
                        (200, serde_json::json!({"url":url,"size":4}).to_string())
                    }
                    "/old.flac" if old_requests.fetch_add(1, Ordering::SeqCst) > 0 => (403, String::new()),
                    "/old.flac" | "/fresh.flac" => (200, "data".to_owned()),
                    _ => panic!("unexpected addon request: {path}"),
                })
            }
        }).await;
        let client = Arc::new(Client::new(&base).unwrap());
        let row = candidate(&serde_json::json!({"id":"song","title":"Test Song"})).unwrap();
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        assert!(client.open(&http, &row, false, Lookup::Playback).await.unwrap().is_none());
        let first = client.open(&http, &row, false, Lookup::Playback).await.unwrap().unwrap();
        let cached = client.open(&http, &row, false, Lookup::Playback).await.unwrap().unwrap();
        assert_eq!(first.url, cached.url);
        assert_eq!(mints.load(Ordering::SeqCst), 2);
        let shared = Shared::new(cached.size, 0, None, cached.format, None, false);
        let refresh = Refresh { once: true, callback: Arc::new({
            let http = http.clone();
            move || {
                let (selection, http) = (cached.selection.clone(), http.clone());
                Box::pin(async move { Ok(Media::File(selection.refresh(&http).await?.url)) })
            }
        }) };
        stream::download(http, Media::File(cached.url), agent().to_owned(), shared.clone(), Some(refresh), Ranging::Header).await;
        assert!(shared.is_complete(), "{:?}", shared.error());
        assert_eq!(old_requests.load(Ordering::SeqCst), 2);
        assert_eq!(mints.load(Ordering::SeqCst), 3);
        assert!(client.streams.lock().await.get(&("song".into(), false)).is_none());
        server.abort();
    }

    #[test]
    fn late_cache_updates_and_invalidations_cannot_replace_a_newer_answer() {
        let key = ("song".to_owned(), false);
        let mut cache = StreamCache::default();
        cache.insert(key.clone(), Value::from("new"), 2);
        cache.insert(key.clone(), Value::from("old"), 1);
        cache.invalidate(&key, 1);
        assert_eq!(cache.get(&key), Some((Value::from("new"), 2)));
        cache.invalidate(&key, 2);
        cache.insert(key.clone(), Value::from("old"), 1);
        assert!(cache.get(&key).is_none());
    }

    #[tokio::test]
    async fn exhausted_retries_still_hold_back_the_next_request() {
        let (sent, mut requests) = tokio::sync::mpsc::unbounded_channel();
        let (base, server) = serve(move |path, _| {
            sent.send(tokio::time::Instant::now()).unwrap();
            std::future::ready(if path == "/busy" { (429, String::new()) } else { (200, "{}".to_owned()) })
        }).await;
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        let quiet = tokio::sync::Mutex::new(Some(tokio::time::Instant::now()));
        let err = get_json(&http, &format!("{base}/busy"), REQUEST_TIMEOUT, &quiet).await.unwrap_err();
        assert_eq!(err.to_string(), "HTTP 429");
        assert_eq!(requests.len(), 3);
        for _ in 0..3 {
            requests.try_recv().unwrap();
        }
        let old = quiet.lock().await.unwrap();
        assert!(old > tokio::time::Instant::now());
        let url = format!("{base}/next");
        let next = get_json(&http, &url, REQUEST_TIMEOUT, &quiet);
        tokio::pin!(next);
        tokio::select! {
            biased;
            _ = &mut next => panic!("request completed during the quiet period"),
            _ = std::future::ready(()) => {}
        }
        // Model the deadline extension a sibling response records during the wait.
        let extended = old + Duration::from_millis(250);
        *quiet.lock().await = Some(extended);
        next.await.unwrap();
        assert!(requests.try_recv().unwrap() >= extended);
        server.abort();
    }

    #[tokio::test]
    async fn addon_upgrade_chooses_a_later_finer_copy_and_refreshes_it() {
        let (release_fine, fine_ready) = tokio::sync::watch::channel(0);
        let (base, server) = serve(move |path, origin| {
            let direct = format!("{origin}/fast.flac");
            let mut fine_ready = fine_ready.clone();
            async move {
                let body = match path.as_str() {
                    "/fast/manifest.json" | "/fine/manifest.json" => r#"{"id":"test"}"#,
                    "/fast/search" => return (200, serde_json::json!({"tracks":[{"id":"song","title":"Test Song","artist":"Test Artist","streamURL":direct}]}).to_string()),
                    "/fast.flac" => "test audio",
                    "/fine/search" => r#"{"tracks":[{"id":"song","title":"Test Song","artist":"Test Artist"}]}"#,
                    "/fast/stream/song" => return (404, String::new()),
                    "/fine/stream/song" => {
                        let stage = *fine_ready.wait_for(|stage| *stage > 0).await.unwrap();
                        tokio::time::sleep(Duration::from_millis(150)).await;
                        match stage {
                            1 => r#"{"url":"https://example.invalid/fine.flac","format":"flac","size":200,"bitDepth":24}"#,
                            2 => r#"{"url":"https://example.invalid/fine.flac?refreshed","format":"flac","size":200,"bitDepth":24}"#,
                            _ => r#"{"url":"https://example.invalid/atmos.m4a","format":"m4a","size":300,"audioMode":"Atmos"}"#,
                        }
                    }
                    _ => panic!("unexpected addon request: {path}"),
                };
                (200, body.to_owned())
            }
        }).await;
        let addons = ["fast", "fine"].map(|name| Arc::new(Client::new(&format!("{base}/{name}")).unwrap()));
        let track = crate::model::Track {
            title: "Test Song".into(),
            artists: vec![crate::model::Link { name: "Test Artist".into(), id: None }],
            ..Default::default()
        };
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        let playback = find(&http, &addons, &track, false, tokio::time::Instant::now() + Duration::from_secs(5), Lookup::Playback).await.unwrap();
        assert_eq!(playback.info.rung(), 1);
        // The slow addon stays pending past playback's cutoff. Keep the plain copy
        // even though its Atmos grace has not elapsed.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
        let playback = tokio::time::timeout(Duration::from_secs(3), find(&http, &addons, &track, true, deadline, Lookup::Playback)).await.unwrap().unwrap();
        assert_eq!(playback.url, format!("{base}/fast.flac"));
        release_fine.send(1).unwrap();
        let upgrade = find(&http, &addons, &track, false, tokio::time::Instant::now() + Duration::from_secs(5), Lookup::Upgrade { fresh: false }).await.unwrap();
        assert_eq!(upgrade.info.rung(), 2);
        assert_eq!(upgrade.url, "https://example.invalid/fine.flac");
        // A fresh pass is the listener's manual upgrade ask: the cached answer is
        // skipped, so the addon's newer word (the stage-2 link) is what comes back.
        release_fine.send(2).unwrap();
        let upgrade = find(&http, &addons, &track, false, tokio::time::Instant::now() + Duration::from_secs(5), Lookup::Upgrade { fresh: true }).await.unwrap();
        assert_eq!(upgrade.url, "https://example.invalid/fine.flac?refreshed");
        let refreshed = upgrade.selection.refresh(&http).await.unwrap();
        assert_eq!(refreshed.url, "https://example.invalid/fine.flac?refreshed");
        assert_eq!(refreshed.size, upgrade.size);
        release_fine.send(3).unwrap();
        // A later playback of the same track: the cached answers have gone stale by
        // then, so the addons are asked again and the Atmos answer (stage 3) wins.
        let addons = ["fast", "fine"].map(|name| Arc::new(Client::new(&format!("{base}/{name}")).unwrap()));
        let playback = find(&http, &addons, &track, true, tokio::time::Instant::now() + Duration::from_secs(5), Lookup::Playback).await.unwrap();
        assert_eq!(playback.info.rung(), 3);
        server.abort();
    }

    #[test]
    fn base_is_normalized_like_the_addon_protocol() {
        assert_eq!(normalize_base(" https://music.example/addon/ "), "https://music.example/addon");
        assert_eq!(normalize_base("https://music.example/addon/manifest.json"), "https://music.example/addon");
        assert_eq!(normalize_base("https://music.example/addon/MANIFEST.JSON"), "https://music.example/addon");
        assert_eq!(normalize_base("https://music.example"), "https://music.example");
    }

    #[test]
    fn base_keeps_query_when_manifest_is_stripped() {
        // Install URLs like `…/manifest.json?sources=tidal` must still
        // normalize: without the split below, the suffix never matches
        // and every endpoint built from the base 404s.
        assert_eq!(
            normalize_base("https://music.example/<token>/quality/manifest.json?sources=tidal"),
            "https://music.example/<token>/quality?sources=tidal"
        );
        assert_eq!(
            normalize_base("https://music.example/addon/manifest.json?foo=1#frag"),
            "https://music.example/addon?foo=1#frag"
        );
        // No manifest suffix: untouched, query included.
        assert_eq!(
            normalize_base("https://music.example/addon?x=1"),
            "https://music.example/addon?x=1"
        );
    }

    #[test]
    fn the_manifest_url_keeps_the_bases_query() {
        // The install link's `?sources=tidal` survives normalization onto the base, so
        // every endpoint must be built with the path before the query, not after it.
        let base = normalize_base("https://addon.example/abc/quality/manifest.json?sources=tidal");
        assert_eq!(base, "https://addon.example/abc/quality?sources=tidal");
        let manifest = base_url_with(&base, &["manifest.json"]).unwrap();
        assert_eq!(manifest.as_str(), "https://addon.example/abc/quality/manifest.json?sources=tidal");
        let mut search = base_url_with(&base, &["search"]).unwrap();
        search.query_pairs_mut().append_pair("quality", "LOSSLESS").append_pair("q", "test");
        assert_eq!(search.as_str(), "https://addon.example/abc/quality/search?sources=tidal&quality=LOSSLESS&q=test");
    }

    #[test]
    fn quality_labels_are_read() {
        assert_eq!(number_before("Hi-Res FLAC 24-bit 96 kHz", "khz"), Some(96.0));
        assert_eq!(number_before("Hi-Res FLAC 24-bit 96 kHz", "bit"), Some(24.0));
        assert_eq!(number_before("320 kbps", "kbps"), Some(320.0));
        assert_eq!(number_before("lossless", "kbps"), None);
    }

    #[test]
    fn a_row_matches_only_its_own_recording() {
        let target = |title: &str| Candidate {
            id: "1".into(),
            title: title.into(),
            artist: "Carly Rae Jepsen".into(),
            duration: Some(200),
            quality: String::new(),
            format: String::new(),
            atmos: false,
            stream_url: None,
        };
        let pick = |title: &str| rank(vec![target(title)], "Call Me Maybe", "Carly Rae Jepsen", Some(202), false).len();
        assert_eq!(pick("Call Me Maybe"), 1);
        assert_eq!(pick("Call Me Maybe (Live at Wembley)"), 0);
        assert_eq!(pick("Call Me Maybe (Official Audio)"), 1);
    }

    #[test]
    fn a_copy_without_the_artist_is_not_the_song() {
        let row = |artist: &str| Candidate {
            id: "1".into(),
            title: "Call Me Maybe".into(),
            artist: artist.into(),
            duration: Some(200),
            quality: String::new(),
            format: String::new(),
            atmos: false,
            stream_url: None,
        };
        let pick = |artist: &str| rank(vec![row(artist)], "Call Me Maybe", "Carly Rae Jepsen", Some(202), false).len();
        assert_eq!(pick(""), 0);
        assert_eq!(pick("Someone Else"), 0);
        assert_eq!(pick("Carly Rae Jepsen"), 1);
        assert_eq!(pick("Carly Jepsen"), 1);
    }

    #[test]
    fn a_retempoed_copy_of_the_same_song_is_the_song() {
        // Same title and artist, only the length differs: the re-tempo filter is gone
        // (the Android app's matcher never had one), so a sped-up or slowed copy that
        // kept the title is matched rather than dropped.
        let row = |secs: u32| Candidate {
            id: "1".into(),
            title: "Call Me Maybe".into(),
            artist: "Carly Rae Jepsen".into(),
            duration: Some(secs),
            quality: String::new(),
            format: String::new(),
            atmos: false,
            stream_url: None,
        };
        let pick = |secs: u32| rank(vec![row(secs)], "Call Me Maybe", "Carly Rae Jepsen", Some(202), false).len();
        assert_eq!(pick(200), 1);
        assert_eq!(pick(160), 1); // sped-up: matched, no length gate
        assert_eq!(pick(250), 1); // slowed: matched too
    }

    #[test]
    fn a_preferred_atmos_copy_leads_even_against_a_better_matched_plain_one() {
        let row = |id: &str, artist: &str, atmos: bool| Candidate {
            id: id.into(),
            title: "Call Me Maybe".into(),
            artist: artist.into(),
            duration: Some(200),
            quality: String::new(),
            format: String::new(),
            atmos,
            stream_url: None,
        };
        let rows = vec![row("plain", "Carly Rae Jepsen", false), row("atmos", "Carly Jepsen", true)];
        let ranked = rank(rows, "Call Me Maybe", "Carly Rae Jepsen", Some(202), true);
        assert_eq!(ranked[0].id, "atmos");
        // Without the preference the exact artist spelling still wins.
        let rows = vec![row("plain", "Carly Rae Jepsen", false), row("atmos", "Carly Jepsen", true)];
        assert_eq!(rank(rows, "Call Me Maybe", "Carly Rae Jepsen", Some(202), false)[0].id, "plain");
    }

    #[test]
    fn the_badge_says_what_the_stream_is() {
        let info = |codec: &str, rate: Option<u32>, bits: Option<u32>| Info {
            source: "Addon".into(),
            host: "example".into(),
            codec: codec.into(),
            quality: String::new(),
            bitrate_kbps: None,
            sample_rate: rate,
            bit_depth: bits,
            channels: None,
            atmos: false,
            size: 1,
            duration_ms: 1,
        };
        assert_eq!(info("FLAC", Some(44_100), Some(16)).badge(), "Lossless");
        assert_eq!(info("FLAC", Some(96_000), Some(24)).badge(), "Hi-Res Lossless");
        assert_eq!(info("MP3", None, None).badge(), "High");
        let atmos = Info { atmos: true, ..info("EAC3", None, None) };
        assert_eq!(atmos.badge(), "Dolby Atmos");
    }

    #[test]
    fn the_ladder_climbs_one_way() {
        // The rung a background upgrade compares against: lossy < Lossless < Hi-Res <
        // Atmos, strictly, so a same-tier answer never restarts the download.
        let info = |codec: &str, rate: Option<u32>, bits: Option<u32>| Info {
            source: "Addon".into(),
            host: "example".into(),
            codec: codec.into(),
            quality: String::new(),
            bitrate_kbps: None,
            sample_rate: rate,
            bit_depth: bits,
            channels: None,
            atmos: false,
            size: 1,
            duration_ms: 1,
        };
        let mp3 = info("MP3", None, None);
        let flac = info("FLAC", Some(44_100), Some(16));
        let hires = info("FLAC", Some(96_000), Some(24));
        let atmos = Info { atmos: true, ..info("EAC3", None, None) };
        assert!(mp3.rung() < flac.rung());
        assert!(flac.rung() < hires.rung());
        assert!(hires.rung() < atmos.rung());
        assert_eq!(mp3.rung(), 0);
        assert_eq!(flac.rung(), flac.rung()); // a same-tier answer is the same rung
    }

    #[test]
    fn atmos_rows_are_read_off_the_names_addons_use() {
        let row = |json: Value| json_is_atmos(&json);
        assert!(row(serde_json::json!({"audioMode": "DOLBY_ATMOS"})));
        assert!(row(serde_json::json!({"audioModes": ["STEREO", "DOLBY_ATMOS"]})));
        assert!(row(serde_json::json!({"quality": "E-AC-3 JOC"})));
        assert!(!row(serde_json::json!({"audioMode": "STEREO", "format": "FLAC"})));
    }

    #[test]
    fn bitchord_auth_flags_are_read_like_the_android_app_reads_them() {
        let flag = |v: Value| manifest_auth(&serde_json::json!({ "id": "x", "bitchordAuth": v }));
        assert!(flag(Value::Bool(true)));
        assert!(flag(Value::from(1)));
        assert!(flag(Value::from("true")));
        assert!(flag(Value::from("1")));
        assert!(flag(Value::from("  true  ")));
        assert!(!flag(Value::Bool(false)));
        assert!(!flag(Value::from(0)));
        assert!(!flag(Value::from("false")));
        assert!(!flag(Value::from("off")));
        assert!(!manifest_auth(&serde_json::json!({ "id": "x" })));
    }

    #[test]
    fn an_auth_answer_is_named_for_what_it_is() {
        let body = r#"{"error":"bitchord_auth","code":"missing","message":"no credentials"}"#;
        assert_eq!(auth_answer(body).as_deref(), Some("the addon refused this client"));
        assert_eq!(auth_answer(r#"{"error":"bitchord_auth","code":"unavailable"}"#).as_deref(), Some("the addon's verifier couldn't load BitChord's keys"));
        assert!(auth_answer(r#"{"error":"other"}"#).is_none());
        assert!(auth_answer("not json").is_none());
    }
}
