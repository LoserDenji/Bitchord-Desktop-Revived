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
use std::time::Duration;

use serde_json::Value;

use crate::stream::Format;

/// Addons key off this prefix; anything else is answered `403 UA_BLOCKED`.
pub const USER_AGENT: &str = concat!("BitChord/v", env!("CARGO_PKG_VERSION"));
const TIER: &str = "LOSSLESS";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const SEARCH_LIMIT: usize = 25;
/// How many of the search's candidates get a `/stream` call before giving up.
const STREAM_TRIES: usize = 3;

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
        if self.atmos {
            "Dolby Atmos"
        } else if self.hires() {
            "Hi-Res Lossless"
        } else if self.lossless() {
            "Lossless"
        } else if self.bitrate_kbps.is_some_and(|k| k >= 320) {
            "320 kbps"
        } else {
            "High"
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
pub struct Stream {
    pub url: String,
    pub size: u64,
    pub duration_ms: u64,
    pub format: Format,
    pub info: Info,
    /// Already-decoded audio (Dolby Atmos out of DASH): no download to run.
    pub data: Option<Vec<u8>>,
}

/// One addon server, with its manifest and recent searches remembered.
pub struct Client {
    base: String,
    manifest: tokio::sync::Mutex<Option<Manifest>>,
    searches: tokio::sync::Mutex<HashMap<String, Vec<Candidate>>>,
}

struct Manifest {
    name: Option<String>,
    defaults: Vec<(String, String)>,
    quality: String,
    /// The addon asks for BitChord's Android-only authentication
    /// (`"bitchordAuth": true`), which a desktop client can't provide.
    auth: bool,
}

#[derive(Clone)]
struct Candidate {
    id: String,
    title: String,
    artist: String,
    duration: Option<u32>,
    quality: String,
    format: String,
    atmos: bool,
    stream_url: Option<String>,
}

impl Client {
    /// A client for whatever the listener pasted. `None` when it isn't an http(s) URL.
    pub fn new(raw: &str) -> Option<Self> {
        let base = normalize_base(raw);
        let parsed = url::Url::parse(&base).ok()?;
        if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none_or(str::is_empty) {
            return None;
        }
        Some(Self { base, manifest: Default::default(), searches: Default::default() })
    }

    /// The addon's host, for a log line (the link's path may carry a token).
    pub fn host(&self) -> String {
        host_of(&self.base)
    }

    /// The addon's copy of `track`, already probed for its size. `None`: a miss (which is
    /// not a fault), so the caller plays it from YouTube. `atmos`: prefer a Dolby Atmos
    /// rendition when the addon offers one, but still accept lossless/hi-res copies.
    pub async fn find(&self, http: &reqwest::Client, track: &crate::model::Track, atmos: bool) -> anyhow::Result<Option<Stream>> {
        // Try the Monochrome protocol first for this addon link.
        let query = crate::providers::Query::of(track);
        let mono_server = crate::providers::Server {
            url: self.base.clone(),
            kind: crate::providers::Kind::Streamer,
            name: self.host(),
        };
        // resolve_on handles its own caching (separate from Bitchord addon cache)
        // and identifying the kind if needed.
        if let Some(found) = crate::providers::Monochrome::resolve_addon_as_mono(http, &mono_server, &query).await {
            return Ok(Some(Stream {
                url: found.url,
                size: found.size,
                duration_ms: found.duration_ms,
                format: found.format,
                info: found.info,
                data: None,
            }));
        }

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
        // When Atmos is preferred, try Atmos-aware results first, then fall back to a
        // normal lossless lookup so non-Atmos hi-res/lossless still works.
        let passes: &[(bool, bool)] = if atmos { &[(true, true), (false, false)] } else { &[(false, false)] };
        let mut tried: BTreeSet<String> = BTreeSet::new();
        let mut last_err = None;
        for &(search_atmos, prefer_atmos) in passes {
            for query in &queries {
                let candidates = match self.search(http, &manifest, query, search_atmos).await {
                    Ok(c) => c,
                    Err(e) => { last_err = Some(e); continue; }
                };
                let ranked = rank(candidates, &title, &artist, track.duration, prefer_atmos);
                for candidate in ranked.into_iter().filter(|c| tried.insert(c.id.clone())).take(STREAM_TRIES) {
                    // Ask for the Atmos rendition only in the Atmos-preferred pass for
                    // candidates that are actually Atmos; otherwise request normally so
                    // lossless/hi-res copies resolve.
                    let stream_atmos = prefer_atmos && candidate.atmos;
                    match self.stream(http, &manifest, &candidate, stream_atmos).await {
                        Ok(Some(s)) => return Ok(Some(s)),
                        Ok(None) => {}
                        Err(e) => {
                            log::info!("hi-res: \"{}\" candidate {} skipped: {e:#}", track.title, candidate.id);
                            last_err = Some(e);
                        }
                    }
                }
            }
        }
        if let (true, Some(e)) = (tried.is_empty(), last_err) {
            return Err(e);
        }
        Ok(None)
    }

    async fn manifest(&self, http: &reqwest::Client) -> anyhow::Result<Manifest> {
        let mut slot = self.manifest.lock().await;
        if let Some(m) = slot.as_ref() {
            return Ok(Manifest { name: m.name.clone(), defaults: m.defaults.clone(), quality: m.quality.clone(), auth: m.auth });
        }
        let url = base_url_with(&self.base, &["manifest.json"])?.into();
        let json = get_json(http, &url).await?;
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
        let json = get_json(http, &url).await?;
        let tracks = json.get("tracks").and_then(Value::as_array).cloned().unwrap_or_default();
        let found: Vec<Candidate> = tracks.iter().filter_map(candidate).take(400).collect();
        let mut cache = self.searches.lock().await;
        if cache.len() > 64 {
            cache.clear();
        }
        cache.insert(key, found.clone());
        Ok(found)
    }

    async fn stream(&self, http: &reqwest::Client, manifest: &Manifest, candidate: &Candidate, atmos: bool) -> anyhow::Result<Option<Stream>> {
        let mut params = manifest.defaults.clone();
        params.push(("quality".to_owned(), manifest.quality.clone()));
        if atmos {
            params.push(("atmos".to_owned(), "auto".to_owned()));
        }
        let url = self.endpoint(&["stream", &candidate.id], &params)?;
        let json = match get_json(http, &url).await {
            Ok(j) => j,
            // A miss, not a fault: the row may still carry its own URL.
            Err(e) if e.to_string().contains("not found") => Value::Null,
            Err(e) => return Err(e),
        };
        let stated_url = json.get("url").and_then(Value::as_str).unwrap_or("").trim().to_owned();
        let stated_url = if stated_url.is_empty() { candidate.stream_url.clone().unwrap_or_default() } else { stated_url };
        anyhow::ensure!(!stated_url.is_empty(), "the addon returned no stream URL");
        anyhow::ensure!(playable_url(&stated_url), "the addon returned a URL that isn't http(s)");
        anyhow::ensure!(!encrypted(&json), "the addon returned an encrypted stream");
        let transport = json.get("manifest").and_then(Value::as_str).or(json.get("mediaType").and_then(Value::as_str)).unwrap_or("");
        let quality = json.get("quality").and_then(Value::as_str).or(json.get("streamQuality").and_then(Value::as_str)).or(json.get("audioQuality").and_then(Value::as_str)).unwrap_or("").trim().to_owned();
        let label = format!("{quality} {} {}", candidate.quality, candidate.format);
        let codec = json.get("codec").and_then(Value::as_str).or(json.get("fileCodec").and_then(Value::as_str)).map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).unwrap_or_default();
        let container = json.get("container").and_then(Value::as_str).or(json.get("containerFormat").and_then(Value::as_str)).unwrap_or("");
        let mime = json.get("mimeType").and_then(Value::as_str).unwrap_or("");
        if is_manifest(transport) {
            // Streaming manifests: the whole rendition is fetched and reworked into
            // something the player decodes like a file. Atmos arrives as a DASH E-AC-3
            // rendition, decoded to stereo PCM; Tidal-style hi-res arrives as a DASH
            // FLAC rendition, reassembled into one .flac byte stream.
            let dash = matches!(transport.trim().to_ascii_lowercase().as_str(), "dash" | "mpd" | "application/dash+xml");
            let codec_lc = codec.to_ascii_lowercase();
            let dolby = ["eac3", "ec-3", "ac3", "ac-3", "dolby", "atmos"].iter().any(|w| codec_lc.contains(w)) || json_is_atmos(&json);
            let flac = codec_lc.contains("flac");
            anyhow::ensure!(dash && (dolby || flac), "the addon returned a streaming manifest, not a file");
            if dolby {
                let audio = crate::dash::fetch(http, &stated_url).await?;
                let size = audio.pcm.len() as u64;
                let info = Info {
                    source: manifest.name.clone().unwrap_or_else(|| host_of(&self.base)),
                    host: host_of(&stated_url),
                    codec: "E-AC-3".to_owned(),
                    quality: if quality.is_empty() { candidate.quality.clone() } else { quality },
                    bitrate_kbps: Some(audio.bitrate_kbps).filter(|k| *k > 0),
                    sample_rate: Some(audio.sample_rate),
                    bit_depth: Some(16),
                    channels: Some(2),
                    atmos: true,
                    size,
                    duration_ms: audio.duration_ms,
                };
                return Ok(Some(Stream { url: stated_url, size, duration_ms: audio.duration_ms, format: Format::Pcm { rate: audio.sample_rate, channels: 2 }, info, data: Some(audio.pcm) }));
            }
            let flac = crate::dash::fetch_flac(http, &stated_url).await?;
            let size = flac.bytes.len() as u64;
            let info = Info {
                source: manifest.name.clone().unwrap_or_else(|| host_of(&self.base)),
                host: host_of(&stated_url),
                codec: "FLAC".to_owned(),
                quality: if quality.is_empty() { candidate.quality.clone() } else { quality },
                bitrate_kbps: Some(flac.bitrate_kbps).filter(|k| *k > 0),
                sample_rate: Some(flac.sample_rate),
                bit_depth: Some(flac.bit_depth),
                channels: Some(flac.channels),
                atmos: false,
                size,
                duration_ms: flac.duration_ms,
            };
            return Ok(Some(Stream { url: stated_url, size, duration_ms: flac.duration_ms, format: Format::File { hint: "flac", mime: Some("audio/flac".to_owned()) }, info, data: Some(flac.bytes) }));
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
            size = 512 * 1024 * 1024;
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
        Ok(Some(Stream { url: stated_url, size, duration_ms, format, info, data: None }))
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
        let url = base_url_with(&self.base, &["manifest.json"])?.into();
        let manifest = async {
            let json = get_json(http, &url).await?;
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
                match probe_search(http, &self.base).await {
                    Ok(n) => {
                        let suffix = if n == 1 { "result" } else { "results" };
                        Health::Ok(format!("{} · search works{}", name.unwrap_or_else(|| self.host()), (n > 0).then(|| format!(" · {n} {suffix}")).unwrap_or_default()))
                    }
                    Err(e) => Health::Rejected(format!("search failed: {e:#}")),
                }
            }
            Err(e) => {
                let reason = format!("{e:#}");
                // An auth-protected addon (or one that guards even its manifest) says
                // so in its answer; that's a verdict, not a server having a bad minute.
                if reason.contains(AUTH_DESKTOP_MSG) {
                    return Health::Rejected(reason);
                }
                match probe_search(http, &self.base).await {
                    Ok(n) => Health::Ok(format!("No manifest · search works · {n} result{}", if n == 1 { "" } else { "s" })),
                    // Transport trouble (DNS, connect, timeout) is a server having a bad
                    // minute; anything the server itself answered is the addon being wrong.
                    Err(_) if e.downcast_ref::<reqwest::Error>().is_some() => Health::Unreachable(reason),
                    Err(_) => Health::Rejected(reason),
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
async fn probe_search(http: &reqwest::Client, base: &str) -> anyhow::Result<usize> {
    let mut url = base_url_with(base, &["search"])?;
    url.query_pairs_mut().append_pair("q", PROBE_QUERY);
    let json = get_json(http, url.as_str()).await?;
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
    if let (Some(wanted), Some(got)) = (duration, candidate.duration)
        && wanted.abs_diff(got) > 30
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
    let atmos_score = if prefer_atmos && candidate.atmos { 8 } else { 0 };
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
    "sessions", "sped", "slowed", "reverb", "nightcore", "lofi", "orchestral", "symphonic", "part", "pt", "chapter",
];

// ── HTTP ─────────────────────────────────────────────────────────────────

async fn get_json(http: &reqwest::Client, url: &str) -> anyhow::Result<Value> {
    let response = http.get(url).timeout(REQUEST_TIMEOUT).header("Accept", "application/json").header("User-Agent", USER_AGENT).send().await?;
    let status = response.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        anyhow::bail!("not found");
    }
    if !status.is_success() {
        // BitChord addon auth answers 401 (503 while its verifier can't load keys)
        // with a JSON body naming itself; say what that means rather than "HTTP 401".
        let body = response.text().await.unwrap_or_default();
        if let Some(msg) = auth_answer(&body) {
            anyhow::bail!("{} {msg}", AUTH_DESKTOP_MSG);
        }
        // Other addons guard themselves behind a sign-in on their own site and point
        // at it ("login":"/auth/discord/login"): they work once the listener has
        // signed in there and pasted the personal link that comes out of it.
        if let Some(login_url) = login_url(url, &body) {
            anyhow::bail!("this addon needs your own sign-in link: open {login_url} in a browser, sign in, and paste the link it gives you here");
        }
        anyhow::bail!("HTTP {}", status.as_u16());
    }
    let bytes = response.bytes().await?;
    anyhow::ensure!(!bytes.is_empty(), "the addon answered with nothing");
    Ok(serde_json::from_slice(&bytes)?)
}

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
    if let Ok(response) = http.head(url).timeout(PROBE_TIMEOUT).header("User-Agent", USER_AGENT).send().await
        && response.status().is_success()
        && let Some(size) = response.content_length().filter(|n| *n > 0)
    {
        return Some(size);
    }
    let response = http.get(url).timeout(PROBE_TIMEOUT).header("User-Agent", USER_AGENT).header("Range", "bytes=0-0").header("Accept-Encoding", "identity").send().await.ok()?;
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
