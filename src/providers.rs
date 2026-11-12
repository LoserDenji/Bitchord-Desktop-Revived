//! Music providers, ported from Mochy-Music's source modules (modules/jiosaavn and
//! modules/monochrome): catalogs that can stand in for YouTube's stream.
//!
//! Like the hi-res addons, a provider that has the song plays its own copy — Monochrome
//! as lossless FLAC from its track streamer (or a relay a fork runs), JioSaavn as AAC up
//! to 320 kbps straight off its public API, no account for either. When neither has the
//! song, playback falls through to YouTube exactly as before. Matching is Mochy's own
//! TrackMatcher: the same recording, or nothing.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use regex::Regex;
use serde_json::Value;

use crate::hires::Info;
use crate::model::Track;
use crate::stream::Format;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);
/// How long a search's answer is trusted before the same song asks again.
const MISS_MEMORY: Duration = Duration::from_secs(30 * 60);
/// How long a server that didn't answer is left alone before it's tried again.
const UNREACHABLE_RETRY: Duration = Duration::from_secs(10 * 60);
/// How many of a search's answers may be matched before the song counts as a miss.
const SEARCH_LIMIT: usize = 8;
/// A stream whose size stays unknown gets this placeholder; the download finds the real
/// end (the same trick the hi-res sources use).
const SIZE_PLACEHOLDER: u64 = 512 * 1024 * 1024;
const MONO_STREAMER: &str = "https://tracks.monochrome.st";
/// The relay only answers clients that say which version of its api they speak.
const RELAY_CLIENT: &str = "relay/2";

const BROWSER_UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/134.0.0.0 Safari/537.36";
/// JioSaavn's CDN hands the file to any browser; the searches want browser headers too.
pub const JIOSAAVN_UA: &str = BROWSER_UA;
pub const MONOCHROME_UA: &str = BROWSER_UA;

/// One playable answer from a provider.
pub struct Found {
    pub url: String,
    pub size: u64,
    pub duration_ms: u64,
    pub format: Format,
    pub info: Info,
    /// The header the file itself is fetched with (the CDN's, not the search's).
    pub user_agent: &'static str,
}

/// A song in a provider's catalog, as its search returns it (Mochy's `CatalogTrack`).
#[derive(Clone, Debug)]
pub struct Catalog {
    pub id: String,
    pub title: String,
    /// Every credited artist, as one line ("A. R. Rahman, Sunidhi Chauhan").
    pub artist: String,
    pub album: Option<String>,
    pub duration_ms: Option<u64>,
}

/// The query a provider matches against: what's playing, as the UI lists it.
pub struct Query<'a> {
    pub title: &'a str,
    pub artists: Vec<String>,
    pub duration_ms: Option<u64>,
}

impl<'a> Query<'a> {
    pub fn of(track: &'a Track) -> Self {
        Self {
            title: &track.title,
            artists: track.artists.iter().map(|a| a.name.clone()).filter(|n| !n.trim().is_empty()).collect(),
            duration_ms: track.duration.map(|s| s as u64 * 1000),
        }
    }

    fn first_artist(&self) -> String {
        self.artists.first().map(String::as_str).unwrap_or("").to_owned()
    }

    fn cache_key(&self) -> String {
        format!("{}|{}", self.title, self.artists.join(","))
    }
}

// ── Matching (a port of Mochy's TrackMatcher) ────────────────────────────────────

/// Finds the catalog song that is the same recording as the one playing, best first.
/// The same recording often sits in a catalog several times (the single, the album, a
/// deluxe); the caller may try the near-equal ones in turn.
pub fn ranked(query: &Query, candidates: Vec<Catalog>) -> Vec<Catalog> {
    let mut scored: Vec<(Catalog, f32)> = candidates.into_iter().map(|c| (c.clone(), score(query, &c))).collect();
    scored.retain(|(_, s)| *s >= 0.7);
    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
    scored.into_iter().map(|(c, _)| c).collect()
}

/// How much a catalog row is the same recording: 0..1, or 0 when it can't be.
fn score(query: &Query, song: &Catalog) -> f32 {
    let title = similarity(&core(query.title), &core(&song.title));
    if title < 0.55 {
        return 0.0;
    }
    let song_artists = words(&song.artist);
    let artist = query
        .artists
        .iter()
        .map(|a| {
            let w = words(a);
            if w.is_empty() { 0.0 } else { w.iter().filter(|x| song_artists.contains(x)).count() as f32 / w.len() as f32 }
        })
        .fold(0.0_f32, f32::max);
    if artist < 0.5 {
        return 0.0;
    }
    let duration = match (query.duration_ms, song.duration_ms) {
        (Some(want), Some(have)) => {
            let diff = want.abs_diff(have) as f32 / 1000.0;
            if diff <= 2.0 {
                1.0
            } else if diff >= 15.0 {
                0.0
            } else {
                1.0 - (diff - 2.0) / 13.0
            }
        }
        _ => 0.6,
    };
    // A remix for an original (or the other way round) is a different recording: whole
    // words only, so "Alive" isn't "live" and a remix isn't also a "mix".
    let q_raw = query.title.to_lowercase();
    let s_raw = song.title.to_lowercase();
    let mismatch = VERSION_WORDS.iter().filter(|w| has_word(&q_raw, w) != has_word(&s_raw, w)).count();
    let penalty = 0.25 * mismatch as f32;
    (0.5 * title + 0.3 * artist + 0.2 * duration - penalty).clamp(0.0, 1.0)
}

/// The title without what sources hang on it ("(From "Aashiqui 2")", "feat. …", a
/// "- Official Video" tail), with the words that make it a different recording kept.
pub fn core(raw: &str) -> String {
    let mut t = raw.to_lowercase();
    for _ in 0..3 {
        let Some((start, end)) = feat_bracket(&t) else { break };
        t.replace_range(start..=end, " ");
    }
    if let Some(at) = dash_tail(&t) {
        t.truncate(at);
    }
    for _ in 0..3 {
        let Some((start, end)) = decoration_bracket(&t) else { break };
        t.replace_range(start..=end, " ");
    }
    // JioSaavn names the film a song comes from ("(From "Aashiqui 2")"); YouTube
    // usually doesn't, so the credit comes off the catalog side too.
    for _ in 0..3 {
        let Some((start, end)) = bracket_of(&t, &["from"], true) else { break };
        t.replace_range(start..=end, " ");
    }
    if let Some(at) = feat_tail(&t) {
        t.truncate(at);
    }
    words(&t).join(" ")
}

/// The words a `(...)`/`[...]` group may start with to count as a feature credit.
const FEAT_WORDS: [&str; 4] = ["feat", "ft", "with", "prod"];
/// Words a `" - Remastered 2011"` style tail must name to come off.
const DASH_WORDS: [&str; 10] = ["remaster", "remastered", "official", "video", "audio", "lyric", "lyrics", "visualizer", "visualiser", "mv"];
/// Words inside a bracket that make the whole bracket packaging ("(Clean Version)").
const BRACKET_WORDS: [&str; 12] = ["remaster", "remastered", "official", "video", "audio", "lyric", "lyrics", "visualizer", "visualiser", "mv", "explicit", "clean"];
/// Words that mark a different recording, whole words only.
const VERSION_WORDS: [&str; 16] = ["remix", "live", "acoustic", "instrumental", "karaoke", "sped up", "slowed", "nightcore", "demo", "cover", "edit", "mix", "version", "unplugged", "orchestral", "piano"];

/// The first `(...)`/`[...]` group holding one of `words`.
fn bracket_of(text: &str, words: &[&str], starts_only: bool) -> Option<(usize, usize)> {
    let mut from = 0;
    while let Some(open) = text[from..].find(['(', '[']) {
        let open = from + open;
        let Some(close) = text[open..].find([')', ']']).map(|c| open + c) else { break };
        let inner = &text[open + 1..close];
        let hits = words.iter().any(|w| (starts_only && starts_with_word(inner, w)) || (!starts_only && has_word(inner, w)));
        if hits {
            return Some((open, close));
        }
        from = close + 1;
    }
    None
}

fn feat_bracket(text: &str) -> Option<(usize, usize)> {
    bracket_of(text, &FEAT_WORDS, true)
}

fn decoration_bracket(text: &str) -> Option<(usize, usize)> {
    bracket_of(text, &BRACKET_WORDS, false)
}

/// Where a `" - Remastered 2011"` style tail starts, if it names packaging.
fn dash_tail(text: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(at) = [" - ", " – "].iter().filter_map(|d| text[from..].find(d).map(|at| from + at)).next() {
        let tail = &text[at..];
        if DASH_WORDS.iter().any(|w| has_word(tail, w)) {
            return Some(at);
        }
        from = at + 3;
    }
    None
}

/// Where a `feat. whoever` tail starts.
fn feat_tail(text: &str) -> Option<usize> {
    for marker in ["feat.", "feat ", "ft.", "ft "] {
        if let Some(at) = text.find(marker) {
            // A whole word, not the middle of another ("featuring" holds "feat").
            let before_ok = at == 0 || !text[..at].ends_with(|c: char| c.is_alphanumeric());
            if before_ok {
                return Some(at);
            }
        }
    }
    None
}

fn starts_with_word(text: &str, word: &str) -> bool {
    let t = text.trim_start();
    let Some(rest) = t.strip_prefix(word) else { return false };
    rest.starts_with(|c: char| !c.is_alphanumeric()) || rest.is_empty()
}

/// `word` as a whole word in `text` (multi-word phrases too: "sped up").
fn has_word(text: &str, word: &str) -> bool {
    let norm: String = text.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ");
    format!(" {} ", norm).contains(&format!(" {} ", word))
}

fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty() && *w != "the" && *w != "and")
        .map(str::to_owned)
        .collect()
}

fn similarity(a: &str, b: &str) -> f32 {
    if a == b {
        return 1.0;
    }
    let wa: std::collections::HashSet<&str> = a.split(' ').filter(|w| !w.is_empty()).collect();
    let wb: std::collections::HashSet<&str> = b.split(' ').filter(|w| !w.is_empty()).collect();
    if wa.is_empty() || wb.is_empty() {
        return 0.0;
    }
    let shared = wa.intersection(&wb).count();
    let union = wa.union(&wb).count();
    shared as f32 / union.max(1) as f32
}

/// Titles come html-escaped ("Tum Hi Ho (From &quot;Aashiqui 2&quot;)").
fn unescape(text: &str) -> String {
    text.replace("&quot;", "\"")
        .replace("&#039;", "'")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
        .trim()
        .to_owned()
}

fn enc(text: &str) -> String {
    url::form_urlencoded::byte_serialize(text.as_bytes()).collect::<String>().replace('+', "%20")
}

fn host_of(url: &str) -> String {
    url::Url::parse(url).ok().and_then(|u| u.host_str().map(str::to_owned)).unwrap_or_default()
}

fn origin_of(url: &str) -> String {
    url::Url::parse(url).map(|u| u.origin().ascii_serialization()).unwrap_or_default()
}

/// What a JSON object's string field says, when it says anything non-blank.
fn string_at(v: &Value, path: &[&str]) -> Option<String> {
    let mut at = v;
    for key in path {
        at = at.get(key)?;
    }
    at.as_str().map(|s| s.to_owned()).filter(|s| !s.trim().is_empty())
}

fn array_at<'a>(v: &'a Value, key: &str) -> Option<&'a Vec<Value>> {
    v.get(key).and_then(Value::as_array)
}

// ── Shared bookkeeping ───────────────────────────────────────────────────────────

/// Songs looked up lately: the matches (so the next play skips the search) and the
/// misses (so the next play of a song a provider hasn't got doesn't pay for it again).
#[derive(Default)]
struct Memory {
    matches: HashMap<String, Catalog>,
    misses: HashMap<String, Instant>,
}

impl Memory {
    fn hit(&self, key: &str) -> Option<Catalog> {
        self.matches.get(key).cloned()
    }

    fn missed(&self, key: &str) -> bool {
        self.misses.get(key).is_some_and(|at| at.elapsed() < MISS_MEMORY)
    }

    fn remember(&mut self, key: &str, song: Catalog) {
        if self.matches.len() > 512 {
            self.matches.clear();
        }
        self.matches.insert(key.to_owned(), song);
    }

    fn mark_miss(&mut self, key: &str) {
        if self.misses.len() > 512 {
            self.misses.clear();
        }
        self.misses.insert(key.to_owned(), Instant::now());
    }
}

/// Servers (or one API) that didn't answer, left alone for a while so one dead server
/// costs one song a timeout, not every song.
#[derive(Default)]
struct Outage {
    down: HashMap<String, (String, Instant)>,
}

impl Outage {
    /// Why `url` is being left alone right now, `None` once its time is up.
    fn reason(&self, url: &str) -> Option<&str> {
        self.down.get(url).filter(|(_, at)| at.elapsed() < UNREACHABLE_RETRY).map(|(r, _)| r.as_str())
    }

    fn mark(&mut self, url: &str, reason: String) {
        self.down.insert(url.to_owned(), (reason, Instant::now()));
    }

    fn clear(&mut self, url: &str) {
        self.down.remove(url);
    }
}

async fn get_text(http: &reqwest::Client, url: &str, headers: &[(&str, &str)]) -> anyhow::Result<String> {
    let response = http.get(url).timeout(REQUEST_TIMEOUT).headers(headers_of(headers)).send().await?;
    let status = response.status();
    anyhow::ensure!(status.is_success(), "HTTP {}", status.as_u16());
    Ok(response.text().await?)
}

fn headers_of(list: &[(&str, &str)]) -> reqwest::header::HeaderMap {
    let mut map = reqwest::header::HeaderMap::new();
    for (k, v) in list {
        if let (Ok(k), Ok(v)) = (reqwest::header::HeaderName::from_bytes(k.as_bytes()), reqwest::header::HeaderValue::from_str(v)) {
            map.insert(k, v);
        }
    }
    map
}

/// A tiny ranged ask, which doubles as an existence check, a size lookup and a read of
/// the file's first bytes: `(total size, first bytes, headers)`. Fails when the server
/// says anything but yes.
async fn head_bytes(http: &reqwest::Client, url: &str, headers: &[(&str, &str)]) -> anyhow::Result<(Option<u64>, Vec<u8>, reqwest::header::HeaderMap)> {
    let response = http
        .get(url)
        .timeout(REQUEST_TIMEOUT)
        .header("Range", "bytes=0-41")
        .header("Accept-Encoding", "identity")
        .headers(headers_of(headers))
        .send()
        .await?;
    let status = response.status();
    anyhow::ensure!(status.is_success(), "HTTP {}", status.as_u16());
    let head = response.headers().clone();
    let total = head
        .get(reqwest::header::CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|r| r.rsplit('/').next().and_then(|s| s.trim().parse::<u64>().ok()))
        .or_else(|| head.get(reqwest::header::CONTENT_LENGTH).and_then(|v| v.to_str().ok()).and_then(|s| s.parse::<u64>().ok()));
    let body = response.bytes().await.map(|b| b.to_vec()).unwrap_or_default();
    Ok((total.filter(|n| *n > 0), body, head))
}

// ── JioSaavn ─────────────────────────────────────────────────────────────────────

/// JioSaavn's catalog as plain AAC files up to 320 kbps, no account and no DRM; it has
/// no lossless and misses many big international labels, and is strongest on Indian
/// music. (Ported from Mochy's `JioSaavnModule`.)
pub struct JioSaavn {
    inner: Mutex<SaavnInner>,
    quality: Mutex<SaavnQuality>,
}

#[derive(Default)]
struct SaavnInner {
    /// Songs found for a query, so the next play of the same song skips the search.
    matches: HashMap<String, Song>,
    /// Queries the catalog had nothing for, so the next play doesn't pay for it again.
    misses: HashMap<String, Instant>,
    outage: Outage,
}

impl SaavnInner {
    fn missed(&self, key: &str) -> bool {
        self.misses.get(key).is_some_and(|at| at.elapsed() < MISS_MEMORY)
    }

    fn mark_miss(&mut self, key: &str) {
        if self.misses.len() > 512 {
            self.misses.clear();
        }
        self.misses.insert(key.to_owned(), Instant::now());
    }
}

/// The stream quality picked in Settings (Mochy's module choices).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SaavnQuality {
    /// JioSaavn's best, 160 for the songs that don't have it.
    #[default]
    K320,
    /// Half the data, still clear.
    K160,
    /// For slow or metered networks.
    K96,
}

impl SaavnQuality {
    pub fn label(self) -> &'static str {
        match self {
            Self::K320 => "AAC 320",
            Self::K160 => "AAC 160",
            Self::K96 => "AAC 96",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::K320 => "320",
            Self::K160 => "160",
            Self::K96 => "96",
        }
    }

    pub fn from_key(key: &str) -> Self {
        match key {
            "96" => Self::K96,
            "160" => Self::K160,
            _ => Self::K320,
        }
    }

    pub const ALL: [SaavnQuality; 3] = [Self::K320, Self::K160, Self::K96];
}

/// One row of the search: the song and what's needed to stream it.
#[derive(Clone)]
struct Song {
    catalog: Catalog,
    encrypted_url: String,
    has320: bool,
}

impl JioSaavn {
    pub fn new() -> Self {
        Self { inner: Mutex::new(SaavnInner::default()), quality: Mutex::new(SaavnQuality::default()) }
    }

    pub fn set_quality(&self, quality: SaavnQuality) {
        *self.quality.lock().unwrap() = quality;
    }

    pub fn quality(&self) -> SaavnQuality {
        *self.quality.lock().unwrap()
    }

    /// JioSaavn's copy of the song, `None` when it has none (a miss, not a fault).
    pub async fn find(&self, http: &reqwest::Client, query: &Query<'_>) -> anyhow::Result<Option<Found>> {
        if let Some(reason) = self.inner.lock().unwrap().outage.reason("jiosaavn") {
            return Err(anyhow::anyhow!("{reason}"));
        }
        let key = query.cache_key();
        if self.inner.lock().unwrap().missed(&key) {
            return Ok(None);
        }
        // The cache is read and written between the asks (its lock mustn't cross one).
        let cached = self.inner.lock().unwrap().matches.get(&key).cloned();
        let song = match cached {
            Some(song) => song,
            None => {
                let search = format!("{} {}", core(query.title), query.first_artist()).trim().to_owned();
                let url = format!(
                    "https://www.jiosaavn.com/api.php?__call=search.getResults&_format=json&_marker=0&api_version=4&ctx=android&p=1&n={SEARCH_LIMIT}&q={}",
                    enc(&search)
                );
                let text = match get_text(http, &url, &JIOSAAVN_HEADERS).await {
                    Ok(text) => {
                        self.inner.lock().unwrap().outage.clear("jiosaavn");
                        text
                    }
                    Err(e) => {
                        let reason = format!("JioSaavn didn't answer ({e:#})");
                        self.inner.lock().unwrap().outage.mark("jiosaavn", reason.clone());
                        return Err(anyhow::anyhow!(reason));
                    }
                };
                let songs = songs_in(&text);
                let Some(best) = ranked(query, songs.iter().map(|s| s.catalog.clone()).collect()).into_iter().next() else {
                    self.inner.lock().unwrap().mark_miss(&key);
                    log::info!("jiosaavn: no matching recording of \u{201c}{}\u{201d}", query.title);
                    return Ok(None);
                };
                let Some(best) = songs.into_iter().find(|s| s.catalog.id == best.id) else {
                    self.inner.lock().unwrap().mark_miss(&key);
                    return Ok(None);
                };
                self.inner.lock().unwrap().matches.insert(key, best.clone());
                best
            }
        };
        log::info!("jiosaavn: \u{201c}{}\u{201d} by {}{}", song.catalog.title, song.catalog.artist, song.catalog.album.as_ref().map(|a| format!(" ({a})")).unwrap_or_default());

        // The link JioSaavn hides behind DES, the same fixed key its apps use.
        let Some(given) = decrypt_media_url(&song.encrypted_url) else {
            return Err(anyhow::anyhow!("JioSaavn's link couldn't be read"));
        };
        let wanted = match self.quality() {
            SaavnQuality::K96 => 96,
            SaavnQuality::K160 => 160,
            SaavnQuality::K320 => if song.has320 { 320 } else { 160 },
        };
        let (url, kbps, size) = stream_at(http, &given, wanted).await;
        let duration_ms = song.catalog.duration_ms.unwrap_or(0);
        let info = Info {
            source: "JioSaavn".to_owned(),
            host: host_of(&url),
            codec: "AAC".to_owned(),
            quality: format!("AAC {}", kbps.map(|k| k.to_string()).unwrap_or_else(|| "?".to_owned())),
            bitrate_kbps: kbps,
            sample_rate: Some(44_100),
            bit_depth: None,
            channels: None,
            atmos: false,
            size,
            duration_ms,
        };
        Ok(Some(Found { url, size, duration_ms, format: Format::File { hint: "m4a", mime: Some("audio/mp4".to_owned()) }, info, user_agent: JIOSAAVN_UA }))
    }
}

/// The explicit cookie lets the uncensored versions show up next to the clean ones.
const JIOSAAVN_HEADERS: [(&str, &str); 3] = [("Accept", "application/json"), ("User-Agent", JIOSAAVN_UA), ("Cookie", "explicit_content=1")];

/// The settings' Test: one search, and the API's answer speaks for the server.
pub async fn probe_saavn(http: &reqwest::Client) -> (bool, String) {
    let url = format!("https://www.jiosaavn.com/api.php?__call=search.getResults&_format=json&_marker=0&api_version=4&ctx=android&p=1&n=1&q={}", enc("test"));
    match get_text(http, &url, &JIOSAAVN_HEADERS).await {
        Ok(text) if serde_json::from_str::<Value>(&text).is_ok_and(|j| array_at(&j, "results").is_some()) => {
            (true, "JioSaavn is reachable · search works".to_owned())
        }
        Ok(_) => (false, "JioSaavn answered, but not with a search".to_owned()),
        Err(e) => (false, format!("JioSaavn didn't answer ({e:#})")),
    }
}

/// The search's rows, with the encrypted link each one streams from.
fn songs_in(text: &str) -> Vec<Song> {
    let Ok(root) = serde_json::from_str::<Value>(text) else { return Vec::new() };
    let Some(results) = array_at(&root, "results") else { return Vec::new() };
    results
        .iter()
        .filter_map(|song| {
            let more = song.get("more_info")?;
            let encrypted = string_at(more, &["encrypted_media_url"])?;
            let id = string_at(song, &["id"])?;
            let title = string_at(song, &["title"]).map(|t| unescape(&t))?;
            let artists = more.get("artistMap").and_then(|m| m.get("primary_artists")).and_then(Value::as_array).map(|list| {
                list.iter().filter_map(|a| a.get("name").and_then(Value::as_str).map(|n| unescape(n))).collect::<Vec<_>>().join(", ")
            });
            let album = string_at(more, &["album"]).map(|a| unescape(&a));
            let duration_ms = string_at(more, &["duration"]).and_then(|d| d.parse::<u64>().ok()).map(|s| s * 1000);
            Some(Song {
                catalog: Catalog { id, title, artist: artists.unwrap_or_default(), album, duration_ms },
                encrypted_url: encrypted,
                has320: string_at(more, &["320kbps"]).is_some_and(|v| v.eq_ignore_ascii_case("true")),
            })
        })
        .collect()
}

/// The media key JioSaavn's apps decrypt their links with (DES-ECB, base64 input).
const MEDIA_KEY: &[u8; 8] = b"38346591";

fn decrypt_media_url(encrypted: &str) -> Option<String> {
    use base64::Engine;
    use ecb::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyInit};
    type DesEcb = ecb::Decryptor<des::Des>;
    let mut text: String = encrypted.trim().chars().filter(|c| !c.is_whitespace()).collect();
    while text.len() % 4 != 0 {
        text.push('=');
    }
    let data = base64::engine::general_purpose::STANDARD.decode(text.as_bytes()).ok()?;
    let plain = DesEcb::new_from_slice(MEDIA_KEY).ok()?.decrypt_padded_vec_mut::<Pkcs7>(&data).ok()?;
    let url = String::from_utf8(plain).ok()?;
    let url = url.trim();
    url.starts_with("http").then(|| url.to_owned())
}

/// The link names one bitrate's file, the others sit beside it under the same name; a
/// file the CDN doesn't have would fail mid-queue, so it's checked with one tiny ask.
async fn stream_at(http: &reqwest::Client, given: &str, kbps: u32) -> (String, Option<u32>, u64) {
    static RENDITION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)_(12|48|96|160|320)\.(mp4|m4a|aac|mp3)([?#]|$)").unwrap());
    let Some(caps) = RENDITION.captures(given) else {
        return sized(http, given, JIOSAAVN_UA, None).await;
    };
    let offered: u32 = caps[1].parse().unwrap_or(0);
    let whole = caps.get(0).unwrap();
    if offered == kbps {
        let (_, size) = there(http, given, JIOSAAVN_UA).await;
        return (given.to_owned(), Some(offered), if size > 0 { size } else { SIZE_PLACEHOLDER });
    }
    let url = format!("{}_{kbps}.{}{}", &given[..whole.start()], &caps[2], &given[whole.end()..]);
    match head_bytes(http, &url, &[("User-Agent", JIOSAAVN_UA)]).await {
        Ok((Some(total), _, _)) => (url, Some(kbps), total),
        // A file the CDN doesn't have falls back to the bitrate the link came with.
        _ => {
            let (_, size) = there(http, given, JIOSAAVN_UA).await;
            (given.to_owned(), Some(offered), if size > 0 { size } else { SIZE_PLACEHOLDER })
        }
    }
}

/// Is `url` there, and how big is it — a ranged ask, then a HEAD.
async fn there(http: &reqwest::Client, url: &str, ua: &str) -> (bool, u64) {
    if let Ok((Some(total), _, _)) = head_bytes(http, url, &[("User-Agent", ua)]).await {
        return (true, total);
    }
    if let Ok(response) = http.head(url).timeout(REQUEST_TIMEOUT).header("User-Agent", ua).send().await
        && response.status().is_success()
        && let Some(size) = response.content_length().filter(|n| *n > 0)
    {
        return (true, size);
    }
    (false, 0)
}

/// The size as the server tells it, else the placeholder ranged reads work with.
async fn sized(http: &reqwest::Client, url: &str, ua: &str, known: Option<u64>) -> (String, Option<u32>, u64) {
    let (_, size) = there(http, url, ua).await;
    let size = known.or(Some(size)).filter(|n| *n > 0).unwrap_or(SIZE_PLACEHOLDER);
    (url.to_owned(), None, size)
}

// ── Monochrome ───────────────────────────────────────────────────────────────────

/// Lossless FLAC for every song Monochrome has, no account needed — from its own track
/// streamer, and from any server a listener adds (a Monochrome site's, or a relay some
/// forks run). (Ported from Mochy's `MonochromeModule`.)
pub struct Monochrome {
    inner: Mutex<MonoInner>,
}

struct MonoInner {
    memory: Memory,
    outage: Outage,
    /// The servers people added, tried before the built-in one since they were added for
    /// a reason. A link's kind is remembered once it has answered as one.
    added: Vec<Server>,
}

/// The two kinds of server Monochrome sites play from: its own track streamer, and the
/// audio relay some forks run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Streamer,
    Relay,
}

/// Which of Monochrome's servers a lookup asks: the ones the listener added, or its own
/// track streamer. Settings › Music sources lists them as separate sources.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Which {
    Added,
    Builtin,
}

impl Kind {
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "Streamer" => Some(Self::Streamer),
            "Relay" => Some(Self::Relay),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Server {
    pub url: String,
    pub kind: Kind,
    pub name: String,
}

impl Monochrome {
    pub fn new() -> Self {
        Self { inner: Mutex::new(MonoInner { memory: Memory::default(), outage: Outage::default(), added: Vec::new() }) }
    }

    /// The servers people added, saved as `url|kind|name` lines (the built-in streamer
    /// is always there and never saved). Kinds start out unknown and are asked for when
    /// a search needs one.
    pub fn set_servers(&self, saved: &str) {
        let mut added = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for line in saved.lines() {
            let line = line.trim();
            if line.is_empty() || !seen.insert(line.to_owned()) {
                continue;
            }
            let Some((url, kind, name)) = parse_server_line(line) else { continue };
            if url == MONO_STREAMER {
                continue;
            }
            added.push(Server { url, kind, name });
        }
        self.inner.lock().unwrap().added = added;
    }

    /// The servers `which` names. The added ones run as their known kind, and as a relay
    /// too when it isn't sure yet, since a pasted link may be either.
    fn servers_for(&self, which: Which) -> Vec<Server> {
        let inner = self.inner.lock().unwrap();
        match which {
            Which::Added => inner
                .added
                .iter()
                .flat_map(|server| {
                    let mut all = vec![server.clone()];
                    if server.kind == Kind::Streamer {
                        all.push(Server { url: server.url.clone(), kind: Kind::Relay, name: server.name.clone() });
                    }
                    all
                })
                .collect(),
            Which::Builtin => vec![Server { url: MONO_STREAMER.to_owned(), kind: Kind::Streamer, name: host_of(MONO_STREAMER) }],
        }
    }

    /// Monochrome's copy of the song from the servers `which` names, `None` when none
    /// of them has it. A server that just failed waits its turn, unless every one of
    /// them did.
    pub async fn find_on(&self, http: &reqwest::Client, which: Which, query: &Query<'_>) -> anyhow::Result<Option<Found>> {
        for server in &self.servers_for(which) {
            if let Some(found) = self.resolve_on(http, server, query).await {
                return Ok(Some(found));
            }
        }
        Ok(None)
    }

    /// Same as `find`, but without outage checks or per-server miss caching — used by
    /// addons that *might* speak the Monochrome protocol so a single bad minute doesn't
    /// poison the Bitchord addon cache. If this also returns `None`, the caller falls
    /// through to the addon protocol (search + `/stream`).
    pub async fn resolve_addon_as_mono(http: &reqwest::Client, server: &Server, query: &Query<'_>) -> Option<Found> {
        // Identify the kind first (unknown streamers are also tried as relays below).
        let kind = match server.kind {
            Kind::Streamer => Kind::Streamer,
            _ => server.kind,
        };
        let identified = identify(http, &server.url).await.unwrap_or(kind);
        let try_kinds = match identified {
            Kind::Relay => vec![Kind::Relay],
            Kind::Streamer => vec![Kind::Streamer, Kind::Relay],
        };
        for k in try_kinds {
            let server = Server { url: server.url.clone(), kind: k, name: server.name.clone() };
            if let Some(found) = Self::resolve_one_direct(http, &server, query).await {
                return Some(found);
            }
        }
        None
    }

    /// The bare “try this server and query” path with no outage/skip bookkeeping —
    /// `resolve_addon_as_mono` uses it after identifying the endpoint’s kind.
    async fn resolve_one_direct(http: &reqwest::Client, server: &Server, query: &Query<'_>) -> Option<Found> {
        let key = format!("{}|{}", server.url, query.cache_key());
        // Reuse the shared cache so repeat finds don't re-search.
        let cached = {
            let mono = Self::new();
            let inner = mono.inner.lock().unwrap();
            (inner.memory.missed(&key), inner.memory.hit(&key))
        };
        let catalog = match cached.1 {
            Some(c) => c,
            None => {
                let search = format!("{} {}", query.first_artist(), core(query.title)).trim().to_owned();
                let url = search_url(server, &search);
                let text = match get_text(http, &url, &search_headers(server.kind)).await {
                    Ok(text) => text,
                    Err(_) => return None,
                };
                let tracks = tracks_in(server.kind, &text);
                let Some(best) = ranked(query, tracks).into_iter().next() else { return None };
                drop(text);
                // Write into a fresh instance's memory (the one the real callers use too).
                let mono = Self::new();
                mono.inner.lock().unwrap().memory.remember(&key, best.clone());
                best
            }
        };
        log::info!("monochrome {}: “{}” by {}{}", server.name, catalog.title, catalog.artist, catalog.album.as_ref().map(|a| format!(" ({a})")).unwrap_or_default());

        // The head of the file: it should open as FLAC, and its STREAMINFO says how fine
        // it is. The relay finds the file on the first ask and hands back a link of its
        // own, which plays without the relay's header.
        let ask = stream_ask(server, &catalog.id);
        let head_headers: Vec<(&str, &str)> = match server.kind {
            Kind::Streamer => vec![("User-Agent", MONOCHROME_UA)],
            Kind::Relay => vec![("User-Agent", MONOCHROME_UA), ("X-Relay-Client", RELAY_CLIENT)],
        };
        let (total, head, response_headers) = match head_bytes(http, &ask, &head_headers).await {
            Ok(answer) => answer,
            Err(_) => return None,
        };
        let url = match server.kind {
            Kind::Streamer => ask.clone(),
            Kind::Relay => match response_headers.get("X-Relay-Stream").and_then(|v| v.to_str().ok()).map(str::to_owned) {
                Some(link) if link.starts_with(&format!("{}/", origin_of(&server.url))) => link,
                _ => return None,
            },
        };
        let Some((bits, rate)) = flac_info(&head) else {
            return None;
        };
        let hi_res = bits > 16 || rate > 48_000;
        let duration_ms = catalog.duration_ms.unwrap_or(0);
        let info = Info {
            source: "Monochrome".to_owned(),
            host: host_of(&url),
            codec: "FLAC".to_owned(),
            quality: if hi_res { "Hi-Res FLAC".to_owned() } else { "FLAC".to_owned() },
            bitrate_kbps: None,
            sample_rate: Some(rate),
            bit_depth: Some(bits),
            channels: None,
            atmos: false,
            size: total.unwrap_or(0),
            duration_ms,
        };
        let mut found = Found {
            url,
            size: total.unwrap_or(0),
            duration_ms,
            format: Format::File { hint: "flac", mime: Some("audio/flac".to_owned()) },
            info,
            user_agent: MONOCHROME_UA,
        };
        if found.size == 0 {
            found.size = SIZE_PLACEHOLDER;
        }
        Some(found)
    }

    async fn resolve_on(&self, http: &reqwest::Client, server: &Server, query: &Query<'_>) -> Option<Found> {
        if self.inner.lock().unwrap().outage.reason(&server.url).is_some() {
            return None;
        }
        let key = format!("{}|{}", server.url, query.cache_key());
        // The cache is read and written between the asks (its lock mustn't cross one).
        let cached = {
            let inner = self.inner.lock().unwrap();
            (inner.memory.missed(&key), inner.memory.hit(&key))
        };
        if cached.0 {
            return None;
        }
        let catalog = match cached.1 {
            Some(c) => c,
            None => {
                let search = format!("{} {}", query.first_artist(), core(query.title)).trim().to_owned();
                let url = search_url(server, &search);
                let text = match get_text(http, &url, &search_headers(server.kind)).await {
                    Ok(text) => {
                        self.inner.lock().unwrap().outage.clear(&server.url);
                        text
                    }
                    Err(e) => {
                        let reason = format!("{} didn't answer ({e:#})", server.name);
                        self.inner.lock().unwrap().outage.mark(&server.url, reason.clone());
                        log::info!("monochrome {}: {reason}", server.url);
                        return None;
                    }
                };
                let tracks = tracks_in(server.kind, &text);
                let Some(best) = ranked(query, tracks).into_iter().next() else {
                    self.inner.lock().unwrap().memory.mark_miss(&key);
                    log::info!("monochrome {}: no matching recording of \u{201c}{}\u{201d}", server.name, query.title);
                    return None;
                };
                self.inner.lock().unwrap().memory.remember(&key, best.clone());
                best
            }
        };
        log::info!("monochrome {}: \u{201c}{}\u{201d} by {}{}", server.name, catalog.title, catalog.artist, catalog.album.as_ref().map(|a| format!(" ({a})")).unwrap_or_default());

        // The head of the file: it should open as FLAC, and its STREAMINFO says how fine
        // it is. The relay finds the file on the first ask and hands back a link of its
        // own, which plays without the relay's header.
        let ask = stream_ask(server, &catalog.id);
        let head_headers: Vec<(&str, &str)> = match server.kind {
            Kind::Streamer => vec![("User-Agent", MONOCHROME_UA)],
            Kind::Relay => vec![("User-Agent", MONOCHROME_UA), ("X-Relay-Client", RELAY_CLIENT)],
        };
        let (total, head, response_headers) = match head_bytes(http, &ask, &head_headers).await {
            Ok(answer) => answer,
            Err(e) => {
                self.inner.lock().unwrap().outage.mark(&server.url, format!("{} gave no stream", server.name));
                log::info!("monochrome {}: {e:#}", server.url);
                return None;
            }
        };
        let url = match server.kind {
            Kind::Streamer => ask.clone(),
            Kind::Relay => match response_headers.get("X-Relay-Stream").and_then(|v| v.to_str().ok()).map(str::to_owned) {
                Some(link) if link.starts_with(&format!("{}/", origin_of(&server.url))) => link,
                _ => {
                    log::info!("monochrome {}: the relay gave no stream link", server.url);
                    return None;
                }
            },
        };
        let Some((bits, rate)) = flac_info(&head) else {
            log::info!("monochrome {}: the stream isn't FLAC", server.url);
            return None;
        };
        let hi_res = bits > 16 || rate > 48_000;
        let duration_ms = catalog.duration_ms.unwrap_or(0);
        let info = Info {
            source: "Monochrome".to_owned(),
            host: host_of(&url),
            codec: "FLAC".to_owned(),
            quality: if hi_res { "Hi-Res FLAC".to_owned() } else { "FLAC".to_owned() },
            bitrate_kbps: None,
            sample_rate: Some(rate),
            bit_depth: Some(bits),
            channels: None,
            atmos: false,
            size: total.unwrap_or(0),
            duration_ms,
        };
        Some(Found {
            url,
            size: total.unwrap_or(0),
            duration_ms,
            format: Format::File { hint: "flac", mime: Some("audio/flac".to_owned()) },
            info,
            user_agent: MONOCHROME_UA,
        })
        .map(|mut found| {
            if found.size == 0 {
                found.size = SIZE_PLACEHOLDER;
            }
            found
        })
    }
}

fn parse_server_line(line: &str) -> Option<(String, Kind, String)> {
    let mut parts = line.splitn(3, '|');
    let url = parts.next()?.trim().to_owned();
    let kind = Kind::from_name(parts.next().unwrap_or("Streamer").trim())?;
    let name = parts.next().unwrap_or("").trim().to_owned();
    let name = if name.is_empty() { host_of(&url) } else { name };
    Some((url, kind, name))
}

fn search_url(server: &Server, query: &str) -> String {
    let base = server.url.trim_end_matches('/');
    match server.kind {
        Kind::Streamer => format!("{base}/search/tracks?q={}&limit={SEARCH_LIMIT}", enc(query)),
        Kind::Relay => format!("{base}/search?q={}&n={SEARCH_LIMIT}", enc(query)),
    }
}

fn stream_ask(server: &Server, id: &str) -> String {
    let base = server.url.trim_end_matches('/');
    match server.kind {
        Kind::Streamer => format!("{base}/track/{}", enc(id)),
        Kind::Relay => format!("{base}/s/{}?q=flac", enc(id)),
    }
}

fn search_headers(kind: Kind) -> Vec<(&'static str, &'static str)> {
    match kind {
        Kind::Streamer => vec![("Accept", "application/json"), ("User-Agent", MONOCHROME_UA)],
        Kind::Relay => vec![("Accept", "application/json"), ("User-Agent", MONOCHROME_UA), ("X-Relay-Client", RELAY_CLIENT)],
    }
}

/// A search's rows, in whichever shape the server answers in.
fn tracks_in(kind: Kind, text: &str) -> Vec<Catalog> {
    let Ok(root) = serde_json::from_str::<Value>(text) else { return Vec::new() };
    match kind {
        Kind::Streamer => streamer_tracks(&root),
        Kind::Relay => relay_tracks(&root),
    }
}

/// The streamer's rows: milliseconds of duration, artists as a list.
fn streamer_tracks(root: &Value) -> Vec<Catalog> {
    let Some(tracks) = array_at(root, "tracks") else { return Vec::new() };
    tracks
        .iter()
        .filter_map(|t| {
            if t.get("playable").and_then(Value::as_bool) == Some(false) {
                return None;
            }
            let id = string_at(t, &["trackId"]).or_else(|| string_at(t, &["id"]))?;
            let title = string_at(t, &["title"])?;
            let artists = t.get("artistNames").and_then(Value::as_array).map(|list| {
                list.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")
            });
            // The streamer gives milliseconds; older answers gave seconds.
            let duration_ms = t.get("duration").and_then(Value::as_u64).map(|d| if d > 10_000 { d } else { d * 1000 });
            Some(Catalog {
                id,
                title,
                artist: artists.unwrap_or_default(),
                album: string_at(t, &["albumTitle"]).or_else(|| string_at(t, &["releaseTitle"])),
                duration_ms,
            })
        })
        .collect()
}

/// The relay's search: every artist on one line joined by a bare `&`, and seconds.
fn relay_tracks(root: &Value) -> Vec<Catalog> {
    let Some(rows) = array_at(root, "data") else { return Vec::new() };
    rows.iter()
        .filter_map(|t| {
            let id = string_at(t, &["id"])?;
            let title = string_at(t, &["title"])?;
            let artist = string_at(t, &["artist"]).map(|a| relay_artists(&a).join(", ")).unwrap_or_default();
            let duration_ms = t.get("duration").and_then(Value::as_u64).map(|d| if d > 10_000 { d } else { d * 1000 });
            Some(Catalog {
                id,
                title,
                artist,
                album: string_at(t, &["album"]),
                duration_ms,
            })
        })
        .collect()
}

/// A bare `&` between artists, so a name like Simon & Garfunkel keeps its spaced one.
fn relay_artists(line: &str) -> Vec<String> {
    let chars: Vec<char> = line.chars().collect();
    let mut parts = Vec::new();
    let mut current = String::new();
    for (i, c) in chars.iter().enumerate() {
        if *c == '&' {
            let spaced_before = i > 0 && chars[i - 1] == ' ';
            let spaced_after = i + 1 < chars.len() && chars[i + 1] == ' ';
            if !spaced_before && !spaced_after {
                parts.push(current.trim().to_owned());
                current.clear();
                continue;
            }
        }
        current.push(*c);
    }
    parts.push(current.trim().to_owned());
    parts.into_iter().filter(|p| !p.is_empty()).collect()
}


/// What a FLAC file's STREAMINFO says: `(bits per sample, sample rate)`. `None` when the
/// bytes aren't a FLAC file at all.
fn flac_info(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 4 || &bytes[0..4] != b"fLaC" {
        return None;
    }
    if bytes.len() < 4 + 4 + 18 {
        return Some((16, 44_100));
    }
    let b = &bytes[8..26];
    let sample_rate = ((b[10] as u32) << 12) | ((b[11] as u32) << 4) | ((b[12] as u32) >> 4);
    let bits = ((((b[12] & 0x01) as u32) << 4) | ((b[13] as u32) >> 4)) + 1;
    let sane = (8000..=768_000).contains(&sample_rate) && (8..=32).contains(&bits);
    Some(if sane { (bits, sample_rate) } else { (16, 44_100) })
}

/// A pasted link as its own base and its site's origin, spaces and a missing
/// `https://` forgiven. `None` when it isn't an http(s) link with a host.
fn link_parts(link: &str) -> Option<(String, String)> {
    let text = link.trim().replace(' ', "%20");
    let text = if text.contains("://") { text } else { format!("https://{text}") };
    let url = url::Url::parse(&text).ok()?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none_or(str::is_empty) {
        return None;
    }
    let origin = url.origin().ascii_serialization();
    let base = format!("{origin}{}", url.path()).trim_end_matches('/').to_owned();
    Some((base, origin))
}

/// Which kind of server answers a search at `base`, if either does: a real search
/// answer of that kind, not a web page or some other api that happens to say 200.
async fn identify(http: &reqwest::Client, base: &str) -> Option<Kind> {
    for kind in [Kind::Streamer, Kind::Relay] {
        let base_server = Server { url: base.to_owned(), kind, name: host_of(base) };
        let url = search_url(&base_server, "test");
        if let Ok(text) = get_text(http, &url, &search_headers(kind)).await {
            let Ok(json) = serde_json::from_str::<Value>(&text) else { continue };
            let answers = match kind {
                Kind::Streamer => array_at(&json, "tracks").is_some(),
                Kind::Relay => array_at(&json, "data").is_some(),
            };
            if answers {
                return Some(kind);
            }
        }
    }
    None
}

/// The settings' Test for a pasted Monochrome link: a server's own link answers a
/// search; a site's doesn't, so its scripts get read for the server it plays from.
pub async fn probe_monochrome(http: &reqwest::Client, link: &str) -> (bool, String) {
    let Some((base, origin)) = link_parts(link) else {
        return (false, "That isn't an http(s) link, so it can't be a server.".to_owned());
    };
    if let Some(kind) = identify(http, &base).await {
        return (true, format!("{} · {} · search works", host_of(&base), if kind == Kind::Streamer { "track streamer" } else { "relay" }));
    }
    // Not a server itself: a Monochrome site names its server in its own scripts.
    let page = match get_text(http, &origin, &[("User-Agent", MONOCHROME_UA)]).await {
        Ok(page) => page,
        Err(e) => return (false, format!("Neither a server nor a site ({e:#})")),
    };
    match server_in_site(http, &origin, &page).await {
        Some(server) => (true, format!("Site plays from {} ({})", server.name, if server.kind == Kind::Streamer { "track streamer" } else { "relay" })),
        None => (false, "No Monochrome server found at that link.".to_owned()),
    }
}

/// A Monochrome site names its server in its own scripts, so each server-looking link
/// in them gets tried.
async fn server_in_site(http: &reqwest::Client, origin: &str, html: &str) -> Option<Server> {
    static SCRIPT_SRC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?i)<script[^>]+src=["']([^"']+)["']"#).unwrap());
    static SERVER_LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"https://[a-z0-9-]+(?:\.[a-z0-9-]+)+").unwrap());
    let site = url::Url::parse(origin).ok()?;
    let scripts: Vec<String> = SCRIPT_SRC
        .captures_iter(html)
        .filter_map(|c| site.join(c[1].trim()).ok().map(|u| u.to_string()))
        .filter(|u| u.starts_with(&format!("{origin}/")))
        .take(4)
        .collect();
    let mut candidates: Vec<String> = Vec::new();
    for script in scripts {
        let Ok(text) = get_text(http, &script, &[("User-Agent", MONOCHROME_UA)]).await else { continue };
        for m in SERVER_LINK.find_iter(&text) {
            let link = m.as_str();
            let looks = host_of(link).starts_with("tracks.") || link.contains("relay");
            if looks && !candidates.iter().any(|c| c == link) {
                candidates.push(link.to_owned());
            }
        }
        if candidates.len() >= 8 {
            break;
        }
    }
    for candidate in candidates.iter().take(8) {
        if let Some(kind) = identify(http, candidate).await {
            return Some(Server { url: candidate.clone(), kind, name: host_of(candidate) });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog(title: &str, artist: &str, duration_ms: Option<u64>) -> Catalog {
        Catalog { id: "1".into(), title: title.into(), artist: artist.into(), album: None, duration_ms }
    }

    #[test]
    fn the_same_recording_matches() {
        let query = Query { title: "Tum Hi Ho", artists: vec!["Arijit Singh".into()], duration_ms: Some(262_000) };
        assert_eq!(ranked(&query, vec![catalog("Tum Hi Ho", "Arijit Singh", Some(262_000))]).len(), 1);
        assert_eq!(ranked(&query, vec![catalog("Tum Hi Ho (From \u{201c}Aashiqui 2\u{201d})", "Arijit Singh", Some(264_000))]).len(), 1);
        // A different recording doesn't.
        assert_eq!(ranked(&query, vec![catalog("Tum Hi Ho (Live)", "Arijit Singh", Some(300_000))]).len(), 0);
        assert_eq!(ranked(&query, vec![catalog("Tum Hi Ho (Remix)", "Arijit Singh", Some(262_000))]).len(), 0);
        // Nor a cover by someone else.
        assert_eq!(ranked(&query, vec![catalog("Tum Hi Ho", "Someone Else", Some(262_000))]).len(), 0);
    }

    #[test]
    fn titles_lose_their_packaging() {
        assert_eq!(core("Tum Hi Ho (From \"Aashiqui 2\")"), "tum hi ho");
        assert_eq!(core("Tum Hi Ho [From \"Aashiqui 2\"]"), "tum hi ho");
        assert_eq!(core("Call Me Maybe (feat. someone)"), "call me maybe");
        assert_eq!(core("Call Me Maybe feat. someone"), "call me maybe");
        assert_eq!(core("Song Name - Remastered 2011"), "song name");
        assert_eq!(core("Song Name - Official Video"), "song name");
        assert_eq!(core("Song Name (Clean Version)"), "song name");
        assert_eq!(core("Alive and Well"), "alive well");
    }

    #[test]
    fn a_remix_is_not_also_a_mix_and_alive_is_not_live() {
        // Whole words only, so neither penalty misfires on the other's title.
        let query = Query { title: "Alive", artists: vec!["Sia".into()], duration_ms: Some(200_000) };
        assert_eq!(ranked(&query, vec![catalog("Alive", "Sia", Some(200_000))]).len(), 1);
        let query = Query { title: "Sunset Mix", artists: vec!["DJ".into()], duration_ms: Some(200_000) };
        // "mix" alone doesn't read as a remix mismatch against itself.
        assert_eq!(ranked(&query, vec![catalog("Sunset Mix", "DJ", Some(200_000))]).len(), 1);
    }

    #[test]
    fn jiosaavn_links_come_out_of_des() {
        use base64::Engine;
        use ecb::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyInit};
        type DesEcb = ecb::Encryptor<des::Des>;
        let plain = b"https://aac.saavncdn.com/744/abc_96.mp4";
        let data = DesEcb::new_from_slice(MEDIA_KEY).unwrap().encrypt_padded_vec_mut::<Pkcs7>(plain);
        let text = base64::engine::general_purpose::STANDARD.encode(data);
        assert_eq!(decrypt_media_url(&text).as_deref(), Some("https://aac.saavncdn.com/744/abc_96.mp4"));
        assert_eq!(decrypt_media_url("not base64 at all"), None);
    }

    #[test]
    fn jiosaavn_rows_are_read() {
        let text = r#"{"results":[{"id":"123","title":"Tum Hi Ho (From &quot;Aashiqui 2&quot;)","more_info":{"encrypted_media_url":"AAAA","duration":"262","320kbps":"true","album":"Aashiqui 2","artistMap":{"primary_artists":[{"name":"Arijit Singh"}]}}}]}"#;
        let songs = songs_in(text);
        assert_eq!(songs.len(), 1);
        assert_eq!(songs[0].catalog.title, "Tum Hi Ho (From \"Aashiqui 2\")");
        assert_eq!(songs[0].catalog.artist, "Arijit Singh");
        assert_eq!(songs[0].catalog.duration_ms, Some(262_000));
        assert!(songs[0].has320);
    }

    #[test]
    fn monochrome_rows_are_read_both_ways() {
        let streamer = r#"{"tracks":[{"trackId":"t1","title":"Eastside","artistNames":["benny blanco","Halsey"],"duration":161,"albumTitle":"Eastside","playable":true},{"trackId":"t2","title":"Gone","playable":false}]}"#;
        let tracks = tracks_in(Kind::Streamer, streamer);
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].id, "t1");
        assert_eq!(tracks[0].artist, "benny blanco, Halsey");
        assert_eq!(tracks[0].duration_ms, Some(161_000));

        let relay = r#"{"data":[{"id":"r1","title":"Eastside","artist":"benny blanco&Halsey & Khalid","duration":161}]}"#;
        let tracks = tracks_in(Kind::Relay, relay);
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].artist, "benny blanco, Halsey & Khalid");
    }

    #[test]
    fn flac_heads_are_read() {
        // A minimal FLAC header: "fLaC", block header, and the 34 bytes of STREAMINFO
        // (10 of sizes, the packed rate/channels/bits, 4 of totals and 16 of md5).
        let mut head = Vec::new();
        head.extend_from_slice(b"fLaC");
        head.extend_from_slice(&[0u8; 4]);
        head.extend_from_slice(&[0u8; 10]);
        // 20 bits of sample rate, 3 of channels - 1, 5 of bits - 1, left-aligned from
        // byte 18 (so the next field's zeroes fill the last four bits of the word).
        let packed: u32 = (44_100 << 8) | (1 << 5) | 15;
        head.extend_from_slice(&(packed << 4).to_be_bytes());
        head.extend_from_slice(&[0u8; 4 + 16]);
        assert_eq!(head.len(), 42);
        assert_eq!(flac_info(&head), Some((16, 44_100)));
        // A 24-bit 96 kHz copy reads as hi-res.
        let mut hires = head.clone();
        let packed: u32 = (96_000 << 8) | (1 << 5) | 23;
        hires[18..22].copy_from_slice(&(packed << 4).to_be_bytes());
        assert_eq!(flac_info(&hires), Some((24, 96_000)));
        assert_eq!(flac_info(b"OggS"), None);
        assert_eq!(flac_info(b"fLaC"), Some((16, 44_100)));
    }

    #[test]
    fn links_are_taken_as_they_come() {
        let (base, origin) = link_parts(" tracks.monochrome.st ").unwrap();
        assert_eq!(base, "https://tracks.monochrome.st");
        assert_eq!(origin, "https://tracks.monochrome.st");
        let (base, _) = link_parts("https://monochrome.example/site/").unwrap();
        assert_eq!(base, "https://monochrome.example/site");
        assert!(link_parts("ftp://x").is_none());
    }

    #[test]
    fn saved_servers_round_trip() {
        let mono = Monochrome::new();
        mono.set_servers("https://relay.example|Relay|\nhttps://tracks.other.example|Streamer|other\nhttps://tracks.monochrome.st|Streamer|builtin");
        let added = mono.servers_for(Which::Added);
        // The saved servers come back with their kinds and names, the built-in one is
        // never saved (it has its own source), and an unknown-kind streamer gets a relay
        // try.
        assert_eq!(added.iter().filter(|s| s.url == "https://relay.example").count(), 1);
        assert_eq!(added.iter().filter(|s| s.url == "https://tracks.other.example").count(), 2);
        assert_eq!(added.iter().filter(|s| s.url == MONO_STREAMER).count(), 0);
        assert_eq!(added.iter().find(|s| s.url == "https://tracks.other.example").map(|s| s.name.as_str()), Some("other"));
        let builtin = mono.servers_for(Which::Builtin);
        assert_eq!(builtin.len(), 1);
        assert_eq!(builtin[0].url, MONO_STREAMER);
        assert_eq!(builtin[0].kind, Kind::Streamer);
    }
}
