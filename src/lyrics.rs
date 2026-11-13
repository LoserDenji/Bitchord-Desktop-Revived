//! Time-synced lyrics from every source the Better Lyrics extension uses, parsed the way
//! its parsers (`@braccato/parsers`) parse them, so any of them can be shown and the
//! listener can switch between them.
//!
//! - The Better Lyrics API streams its providers as server-sent events: Better Lyrics
//!   (Apple-style TTML), BiniLyrics (TTML), Musixmatch (word-by-word and line LRC), LRCLib
//!   (LRC and plain), Legato (KuGou LRC) and Portato (QQ QRC).
//! - Unison, the community lyrics service, answers a plain GET.
//! - lrc.red answers a plain GET of its own.
//! - The app's own, fetched like ForkChord's do: PaxSenix (Apple Music's timings),
//!   LyricsPlus (community mirrors) and Genius (plain text fallback), plus the two
//!   YouTube sources read straight off the playing video (its captions and its
//!   Lyrics tab, see `ytm.rs`).
//!
//! The API needs a JWT, obtained by exchanging a Cloudflare Turnstile token (see
//! turnstile.rs). JWTs last 24 h and are cached on disk.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use serde_json::Value;

const API: &str = "https://lyrics.api.dacubeking.com/";
const UNISON_API: &str = "https://unison.betterlyrics.org/lyrics";
const LRCRED_API: &str = "https://lrc.red/";

/// lrc.red's free API asks callers to identify themselves with a link.
const LRCRED_UA: &str = concat!("BitChord/", env!("CARGO_PKG_VERSION"), " (+https://github.com/LoserDenji/Bitchord-Desktop-Revived)");

/// One source of lyrics, in the extension's default priority order (its
/// `PROVIDER_CONFIGS`, without the two YouTube sources; the ones past the stream
/// are fetched by the app itself, like ForkChord's own providers fetch them).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Provider {
    BLyricsRich,
    UnisonRich,
    BinimumRich,
    UnisonWord,
    PortatoRich,
    PaxsenixRich,
    LyricsPlusRich,
    MusixmatchRich,
    LrcredRich,
    BLyricsSynced,
    UnisonSynced,
    BinimumSynced,
    LrclibSynced,
    LegatoSynced,
    MusixmatchSynced,
    LrcredSynced,
    UnisonPlain,
    YoutubeCaptions,
    YoutubePlain,
    LrclibPlain,
    GeniusPlain,
}

/// How finely a provider's lyrics are timed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SyncKind {
    Syllable,
    Word,
    Line,
    Unsynced,
}

impl Provider {
    pub const ALL: [Provider; 21] = [
        Provider::BLyricsRich,
        Provider::UnisonRich,
        Provider::BinimumRich,
        Provider::UnisonWord,
        Provider::PortatoRich,
        Provider::PaxsenixRich,
        Provider::LyricsPlusRich,
        Provider::MusixmatchRich,
        Provider::LrcredRich,
        Provider::BLyricsSynced,
        Provider::UnisonSynced,
        Provider::BinimumSynced,
        Provider::LrclibSynced,
        Provider::LegatoSynced,
        Provider::MusixmatchSynced,
        Provider::LrcredSynced,
        Provider::UnisonPlain,
        Provider::YoutubeCaptions,
        Provider::YoutubePlain,
        Provider::LrclibPlain,
        Provider::GeniusPlain,
    ];

    /// The extension's key for it (also how the order is stored).
    pub fn key(self) -> &'static str {
        match self {
            Provider::BLyricsRich => "bLyrics-richsynced",
            Provider::UnisonRich => "unison-richsynced",
            Provider::BinimumRich => "binimum-richsynced",
            Provider::UnisonWord => "unison-wordsynced",
            Provider::PortatoRich => "portato-richsynced",
            Provider::PaxsenixRich => "paxsenix-richsynced",
            Provider::LyricsPlusRich => "lyricsPlus-richsynced",
            Provider::MusixmatchRich => "musixmatch-richsync",
            Provider::LrcredRich => "lrcred-richsynced",
            Provider::BLyricsSynced => "bLyrics-synced",
            Provider::UnisonSynced => "unison-synced",
            Provider::BinimumSynced => "binimum-synced",
            Provider::LrclibSynced => "lrclib-synced",
            Provider::LegatoSynced => "legato-synced",
            Provider::MusixmatchSynced => "musixmatch-synced",
            Provider::LrcredSynced => "lrcred-synced",
            Provider::UnisonPlain => "unison-plain",
            Provider::YoutubeCaptions => "youtube-captions",
            Provider::YoutubePlain => "youtube-music",
            Provider::LrclibPlain => "lrclib-plain",
            Provider::GeniusPlain => "genius-plain",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.key() == key)
    }

    pub fn name(self) -> &'static str {
        match self {
            Provider::BLyricsRich | Provider::BLyricsSynced => "Better Lyrics",
            Provider::UnisonRich | Provider::UnisonWord | Provider::UnisonSynced | Provider::UnisonPlain => "Unison",
            Provider::BinimumRich | Provider::BinimumSynced => "BiniLyrics",
            Provider::PortatoRich => "Better Lyrics Portato",
            Provider::PaxsenixRich => "PaxSenix",
            Provider::LyricsPlusRich => "LyricsPlus",
            Provider::MusixmatchRich | Provider::MusixmatchSynced => "Musixmatch",
            Provider::LrclibSynced | Provider::LrclibPlain => "LRCLib",
            Provider::LrcredRich | Provider::LrcredSynced => "lrc.red",
            Provider::LegatoSynced => "Better Lyrics Legato",
            Provider::YoutubeCaptions => "YouTube captions",
            Provider::YoutubePlain => "YouTube Music",
            Provider::GeniusPlain => "Genius",
        }
    }

    pub fn sync(self) -> SyncKind {
        match self {
            Provider::BLyricsRich | Provider::UnisonRich | Provider::BinimumRich | Provider::LyricsPlusRich => SyncKind::Syllable,
            Provider::UnisonWord | Provider::PortatoRich | Provider::PaxsenixRich | Provider::MusixmatchRich | Provider::LrcredRich => SyncKind::Word,
            Provider::BLyricsSynced
            | Provider::UnisonSynced
            | Provider::BinimumSynced
            | Provider::LrclibSynced
            | Provider::LegatoSynced
            | Provider::MusixmatchSynced
            | Provider::LrcredSynced
            | Provider::YoutubeCaptions => SyncKind::Line,
            Provider::UnisonPlain | Provider::YoutubePlain | Provider::LrclibPlain | Provider::GeniusPlain => SyncKind::Unsynced,
        }
    }
}

/// How a set of lines is timed, decided from the lines themselves (`deriveSyncType`).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Sync {
    None,
    Line,
    Word,
}

/// A timed piece of a line: a syllable, a word or several words.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub start: i32,
    pub dur: i32,
    pub text: String,
    /// Background vocals, shown on their own row under the line.
    pub bg: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Line {
    pub start: i32,
    pub dur: i32,
    pub text: String,
    /// Empty for line-synced and plain lyrics.
    pub parts: Vec<Part>,
    /// Singer: `v1`, `v2`, … or `v1000` for everyone.
    pub agent: Option<String>,
    /// A gap of more than five seconds nobody sings over.
    pub instrumental: bool,
    /// Translations shipped with the lyrics, by language.
    pub translations: Vec<(String, String)>,
    /// Romanization shipped with the lyrics, and its timing when it has some.
    pub romanization: Option<String>,
    pub timed_roman: Vec<Part>,
}

impl Line {
    pub fn end(&self) -> i32 {
        self.start + self.dur
    }
}

#[derive(Clone, Debug)]
pub struct Lyrics {
    pub lines: Vec<Line>,
    pub sync: Sync,
    pub provider: Provider,
    /// Language the source says the lyrics are in.
    pub language: Option<String>,
    pub songwriters: Vec<String>,
}

impl Lyrics {
    pub(crate) fn new(provider: Provider, lines: Vec<Line>, language: Option<String>, songwriters: Vec<String>) -> Option<Self> {
        if lines.is_empty() {
            return None;
        }
        let sync = derive_sync(&lines);
        Some(Self { lines, sync, provider, language, songwriters })
    }
}

/// `deriveSyncType`: richsync if any sung line has a part with a duration.
fn derive_sync(lines: &[Line]) -> Sync {
    if lines.iter().any(|l| !l.instrumental && l.parts.iter().any(|p| p.dur != 0)) {
        Sync::Word
    } else if lines.iter().all(|l| l.start == 0) {
        Sync::None
    } else {
        Sync::Line
    }
}

/// Every source that had lyrics for a song, in default priority order.
#[derive(Clone, Debug, Default)]
pub struct LyricsSet {
    pub sources: Vec<Lyrics>,
}

impl LyricsSet {
    pub fn get(&self, p: Provider) -> Option<&Lyrics> {
        self.sources.iter().find(|l| l.provider == p)
    }
}

// ------------------------------------------------------------------- API client

pub struct Client {
    http: reqwest::Client,
    jwt_file: PathBuf,
    webview_profile: PathBuf,
    jwt: tokio::sync::Mutex<Option<String>>,
    /// When the Turnstile challenge last failed; it isn't retried for a while, so
    /// every new song doesn't start another WebView2.
    challenge_failed: std::sync::Mutex<Option<std::time::Instant>>,
    /// The last few songs' lyrics, for replays and going back.
    recent: std::sync::Mutex<std::collections::VecDeque<(String, LyricsSet)>>,
    /// Apple's web-player token, scraped once per session for PaxSenix's Apple
    /// lyrics (cleared when it stops working, so the next song tries again).
    apple_jwt: tokio::sync::Mutex<Option<String>>,
}

const CHALLENGE_RETRY: Duration = Duration::from_secs(15 * 60);
const RECENT: usize = 8;

pub struct Query<'a> {
    pub video_id: &'a str,
    pub song: &'a str,
    pub artist: &'a str,
    pub album: Option<&'a str>,
    pub duration: Option<u32>,
}

impl Client {
    pub fn new(http: reqwest::Client, data_dir: &Path) -> Self {
        let jwt_file = data_dir.join("lyrics_jwt");
        let cached = std::fs::read_to_string(&jwt_file).ok().filter(|t| jwt_seconds_left(t) > 3600);
        Self {
            http,
            jwt_file,
            webview_profile: data_dir.join("webview-lyrics"),
            jwt: tokio::sync::Mutex::new(cached),
            challenge_failed: Default::default(),
            recent: Default::default(),
            apple_jwt: tokio::sync::Mutex::new(None),
        }
    }

    /// Every source's lyrics for a song: the Better Lyrics stream, Unison, lrc.red,
    /// and the app's own providers (PaxSenix, LyricsPlus, Genius), together.
    pub async fn fetch(&self, q: &Query<'_>) -> anyhow::Result<LyricsSet> {
        if let Some((_, l)) = self.recent.lock().unwrap().iter().find(|(id, _)| id == q.video_id) {
            return Ok(l.clone());
        }
        let (stream, unison, lrcred, paxsenix, lyricsplus, genius) = tokio::join!(
            self.fetch_stream(q),
            self.fetch_unison(q),
            self.fetch_lrcred(q),
            self.fetch_paxsenix(q),
            self.fetch_lyricsplus(q),
            self.fetch_genius(q)
        );
        let mut sources = Vec::new();
        match stream {
            Ok(s) => sources.extend(s),
            Err(e) => log::warn!("better lyrics failed: {e:#}"),
        }
        match unison {
            Ok(s) => sources.extend(s),
            Err(e) => log::info!("unison: {e:#}"),
        }
        match lrcred {
            Ok(s) => sources.extend(s),
            Err(e) => log::info!("lrc.red: {e:#}"),
        }
        sources.extend(paxsenix);
        sources.extend(lyricsplus);
        sources.extend(genius);
        if sources.is_empty() {
            return Ok(LyricsSet::default());
        }
        sources.sort_by_key(|l| Provider::ALL.iter().position(|p| *p == l.provider));
        let set = LyricsSet { sources };
        let mut recent = self.recent.lock().unwrap();
        if recent.len() == RECENT {
            recent.pop_front();
        }
        recent.push_back((q.video_id.to_owned(), set.clone()));
        Ok(set)
    }

    async fn fetch_stream(&self, q: &Query<'_>) -> anyhow::Result<Vec<Lyrics>> {
        let mut jwt = self.token(false).await?;
        for attempt in 0..2 {
            let mut form = vec![("videoId", q.video_id.to_owned())];
            if !q.song.is_empty() {
                form.push(("song", q.song.to_owned()));
            }
            if !q.artist.is_empty() {
                form.push(("artist", q.artist.to_owned()));
            }
            if let Some(d) = q.duration.filter(|d| *d > 0) {
                form.push(("duration", d.to_string()));
            }
            if let Some(a) = q.album {
                form.push(("album", a.to_owned()));
            }
            form.push(("alwaysFetchMetadata", "false".to_owned()));
            form.push(("token", jwt.clone()));
            let resp = self.http.post(format!("{API}v2/lyrics")).form(&form).timeout(Duration::from_secs(20)).send().await?;
            if resp.status() == reqwest::StatusCode::FORBIDDEN && attempt == 0 {
                jwt = self.token(true).await?;
                continue;
            }
            let body = resp.error_for_status()?.text().await?;
            return Ok(from_stream(&body, q));
        }
        unreachable!()
    }

    async fn fetch_unison(&self, q: &Query<'_>) -> anyhow::Result<Vec<Lyrics>> {
        let mut params = vec![("v", q.video_id.to_owned()), ("song", q.song.to_owned()), ("artist", q.artist.to_owned())];
        params.push(("duration", q.duration.unwrap_or(0).to_string()));
        if let Some(a) = q.album {
            params.push(("album", a.to_owned()));
        }
        let resp = self.http.get(UNISON_API).query(&params).timeout(Duration::from_secs(10)).send().await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(vec![]);
        }
        let v: Value = resp.error_for_status()?.json().await?;
        Ok(from_unison(&v["data"], q.duration.unwrap_or(0) as i32 * 1000))
    }

    /// lrc.red: find the track in its catalogue, then fetch its TTML (word or line timed).
    async fn fetch_lrcred(&self, q: &Query<'_>) -> anyhow::Result<Vec<Lyrics>> {
        if q.song.trim().is_empty() {
            return Ok(vec![]);
        }
        let query = match q.artist.trim().is_empty() {
            true => q.song.to_owned(),
            false => format!("{} {}", q.song, q.artist),
        };
        let hits: Value = self
            .http
            .get(format!("{LRCRED_API}search.json"))
            .query(&[("q", query)])
            .header(reqwest::header::USER_AGENT, LRCRED_UA)
            .timeout(Duration::from_secs(8))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let Some(hit) = best_lrcred_hit(hits["hits"].as_array().map(Vec::as_slice).unwrap_or_default(), q) else {
            return Ok(vec![]);
        };
        let Some(isrc) = hit["isrc"].as_str().filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric())) else {
            return Ok(vec![]);
        };
        let text = self
            .http
            .get(format!("{LRCRED_API}s/{isrc}.ttml"))
            .header(reqwest::header::USER_AGENT, LRCRED_UA)
            .timeout(Duration::from_secs(8))
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        let t = parse_ttml(&text, q.duration.unwrap_or(0) as i32 * 1000);
        // lrc.red declares its own timing on the root; fall back to the parsed spans.
        let word = if text.contains("lrc:timing=\"Line\"") {
            false
        } else if text.contains("lrc:timing=\"Word\"") || text.contains("lrc:timing=\"Syllable\"") {
            true
        } else {
            t.word_synced
        };
        let p = if word { Provider::LrcredRich } else { Provider::LrcredSynced };
        Ok(Lyrics::new(p, t.lines, t.language, t.songwriters).into_iter().collect())
    }

    /// A valid JWT, from cache or by running the Turnstile challenge.
    async fn token(&self, force_new: bool) -> anyhow::Result<String> {
        let mut slot = self.jwt.lock().await;
        if let Some(t) = slot.as_ref().filter(|t| !force_new && jwt_seconds_left(t) > 60) {
            return Ok(t.clone());
        }
        if self.challenge_failed.lock().unwrap().is_some_and(|at| at.elapsed() < CHALLENGE_RETRY) {
            anyhow::bail!("lyrics verification failed recently; retrying later");
        }
        let profile = self.webview_profile.clone();
        let turnstile = match tokio::task::spawn_blocking(move || crate::turnstile::obtain(profile)).await? {
            Ok(t) => t,
            Err(e) => {
                *self.challenge_failed.lock().unwrap() = Some(std::time::Instant::now());
                return Err(e);
            }
        };
        let resp: Value = self
            .http
            .post(format!("{API}verify-turnstile"))
            .json(&serde_json::json!({ "token": turnstile }))
            .timeout(Duration::from_secs(15))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let jwt = resp["jwt"].as_str().ok_or_else(|| anyhow::anyhow!("no JWT in response"))?.to_owned();
        let _ = std::fs::write(&self.jwt_file, &jwt);
        log::info!("lyrics: new access token ({}h)", jwt_seconds_left(&jwt) / 3600);
        *slot = Some(jwt.clone());
        Ok(jwt)
    }
}

fn jwt_seconds_left(jwt: &str) -> i64 {
    use base64::Engine;
    let Some(payload) = jwt.split('.').nth(1) else { return 0 };
    let Ok(bytes) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')) else {
        return 0;
    };
    let exp = serde_json::from_slice::<Value>(&bytes).ok().and_then(|v| v["exp"].as_i64()).unwrap_or(0);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    exp - now
}

// ------------------------------------------------------------------- the app's own providers
//
// The sources past the Better Lyrics stream, fetched the way ForkChord's providers
// fetch them (PaxSenix.kt, LyricsPlus.kt, Genius.kt): Apple's timings through
// PaxSenix's keyless API, LyricsPlus's community mirrors, and Genius as the plain
// fallback. Each answers at most one Lyrics, or nothing.

const APPLE_WEB: &str = "https://music.apple.com";
const PAXSENIX: &str = "https://lyrics.paxsenix.org";
const APPLE_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0 Safari/537.36";

/// Apple's web-player token, the same shape apple.rs scrapes for its catalog reads.
static APPLE_TOKEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"eyJ[A-Za-z0-9_-]+\.eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+").unwrap());

/// LyricsPlus's mirrors (atomix.one, prjktla.workers.dev and two vercel.app
/// deployments died on 2026-10-17, like ForkChord's list says).
const LYRICSPLUS_MIRRORS: &[&str] = &["https://lyricsplus.prjktla.my.id", "https://lyricsplus.binimum.org"];

/// A YouTube title as the lyrics databases indexed it: credits and packaging off,
/// version words kept (`forLyricsSearch`).
static CREDITS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        Regex::new(r"(?i)\s*[(\[]\s*(feat|ft|featuring|with)\b[^)\]]*[)\]]").unwrap(),
        Regex::new(r"(?i)\s+(feat|ft|featuring)\.?\s+.*$").unwrap(),
        Regex::new(r"(?i)\s*[(\[]\s*(official\s*)?(music\s*)?(video|audio|visuali[sz]er|lyrics?\s*video|lyrics?|m/?v|hd|hq|4k|full\s*song)\s*[)\]]").unwrap(),
        Regex::new(r"(?i)\s*[(\[]\s*official\s*[)\]]").unwrap(),
    ]
});

/// What Genius itself leaves in its titles (Genius.kt's `GENIUS_NOISE`, plus the
/// music-note characters its pages sprinkle in).
static GENIUS_NOISE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\s*[(\[]\s*(?:from|feat\.?|ft\.?|featuring|with|prod\.?|produced by|official|lyrical|video|audio|remix|music video|visualizer|mv|hd|4k|hq|full song)[^)\]]*[)\]]|\s*\b(?:official\s+(?:music\s+)?(?:video|audio)|lyrical(?:\s+video)?|full\s+song|4k\s+video|hd\s+video|music\s+video)\b").unwrap()
});

/// The cleaned title and artist the outside providers are asked with.
fn lyrics_search_terms(q: &Query<'_>) -> (String, String) {
    let mut title = q.song.to_owned();
    for r in CREDITS.iter() {
        title = r.replace_all(&title, " ").into_owned();
    }
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ").trim_end_matches([',', '-', '–', '—']).trim().to_owned();
    let title = if title.is_empty() { q.song.trim().to_owned() } else { title };
    let artist = q.artist.trim().strip_suffix(" - Topic").unwrap_or(q.artist.trim()).trim().to_owned();
    (title, artist)
}

impl Client {
    /// PaxSenix: Apple Music's own syllable timings through its keyless API. The
    /// recording is found on Apple first (its catalog search, with the web player's
    /// scraped token), then PaxSenix is asked for that recording's lyrics.
    async fn fetch_paxsenix(&self, q: &Query<'_>) -> Vec<Lyrics> {
        if q.song.trim().is_empty() {
            return vec![];
        }
        match self.paxsenix_inner(q).await {
            Ok(l) => l,
            Err(e) => {
                log::info!("paxsenix: {e:#}");
                vec![]
            }
        }
    }

    async fn paxsenix_inner(&self, q: &Query<'_>) -> anyhow::Result<Vec<Lyrics>> {
        let (title, artist) = lyrics_search_terms(q);
        let dur_ms = q.duration.unwrap_or(0) as i32 * 1000;
        let Some(token) = self.apple_web_token().await else { return Ok(vec![]) };
        let search: String = self
            .http
            .get("https://amp-api.music.apple.com/v1/catalog/us/search")
            .query(&[("term", format!("{title} {artist}")), ("types", "songs".to_owned()), ("limit", "10".to_owned()), ("l", "en-US".to_owned())])
            .header("User-Agent", APPLE_UA)
            .bearer_auth(&token)
            .header("Origin", APPLE_WEB)
            .timeout(Duration::from_secs(10))
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        let v: Value = serde_json::from_str(&search)?;
        let Some(id) = best_candidate(&v, &title, &artist, dur_ms) else { return Ok(vec![]) };
        let body = self
            .http
            .get(format!("{PAXSENIX}/apple-music/lyrics"))
            .query(&[("id", id.as_str()), ("ttml", "true")])
            .timeout(Duration::from_secs(12))
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        Ok(pax_response(&body, dur_ms))
    }

    /// Apple's web-player token, read the way the browser gets it: load the player,
    /// find the JS bundle it pulls in, take the token out. Cached for the session;
    /// cleared on failure so the next song scrapes again.
    async fn apple_web_token(&self) -> Option<String> {
        if let Some(t) = self.apple_jwt.lock().await.clone() {
            return Some(t);
        }
        let page = self.http.get(format!("{APPLE_WEB}/us/new")).header("User-Agent", APPLE_UA).timeout(Duration::from_secs(12)).send().await.ok()?.text().await.ok()?;
        let at = page.find("/assets/index~")?;
        let end = at + page[at..].find(['"', '\''])?;
        let script = format!("{APPLE_WEB}{}", &page[at..end]);
        let source = self.http.get(&script).header("User-Agent", APPLE_UA).timeout(Duration::from_secs(12)).send().await.ok()?.text().await.ok()?;
        let token = APPLE_TOKEN.find(&source)?.as_str().to_owned();
        log::info!("lyrics: apple web token scraped");
        *self.apple_jwt.lock().await = Some(token.clone());
        Some(token)
    }

    /// LyricsPlus: syllable-timed lyrics from its community mirrors, all asked at
    /// once and the first usable answer taken (LyricsPlus.kt races them too).
    async fn fetch_lyricsplus(&self, q: &Query<'_>) -> Vec<Lyrics> {
        if q.song.trim().is_empty() {
            return vec![];
        }
        let (title, artist) = lyrics_search_terms(q);
        let params = [
            ("title", title),
            ("artist", artist),
            ("duration", q.duration.unwrap_or(0).to_string()),
            ("album", q.album.unwrap_or_default().to_owned()),
        ];
        let asks = LYRICSPLUS_MIRRORS.iter().map(|host| {
            let url = format!("{host}/v2/lyrics/get");
            let params = params.clone();
            async move { self.http.get(&url).query(&params).timeout(Duration::from_secs(12)).send().await?.error_for_status()?.text().await }
        });
        let answers = futures_util::future::join_all(asks).await;
        for body in answers.into_iter().flatten() {
            let Ok(v) = serde_json::from_str::<Value>(&body) else { continue };
            let lines = from_lyricsplus(&v, q.duration.unwrap_or(0) as i32 * 1000);
            if let Some(l) = Lyrics::new(Provider::LyricsPlusRich, lines, None, vec![]) {
                return vec![l];
            }
        }
        vec![]
    }

    /// Genius: plain text from the biggest catalogue there is, as the last resort.
    /// Its web search is scored like ForkChord's (translations and tracklists down),
    /// then the song page's lyrics container is read out of its HTML.
    async fn fetch_genius(&self, q: &Query<'_>) -> Vec<Lyrics> {
        if q.song.trim().is_empty() {
            return vec![];
        }
        match self.genius_inner(q).await {
            Ok(l) => l,
            Err(e) => {
                log::info!("genius: {e:#}");
                vec![]
            }
        }
    }

    async fn genius_inner(&self, q: &Query<'_>) -> anyhow::Result<Vec<Lyrics>> {
        static BARE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*[\(\[].*?[\)\]]").unwrap());
        let (title, artist) = {
            let (t, a) = lyrics_search_terms(q);
            (genius_clean(&t), genius_clean(&a))
        };
        // The title without its brackets and punctuation: Genius files songs under
        // plainer names than YouTube's titles are (`Genius.kt`).
        let bare = BARE.replace_all(&title, " ").into_owned();
        let bare = bare.split(|c: char| !c.is_alphanumeric() && !c.is_whitespace()).collect::<Vec<_>>().join(" ");
        let bare = bare.split_whitespace().collect::<Vec<_>>().join(" ");
        let mut attempts: Vec<String> = vec![format!("{artist} {title}").trim().to_owned(), format!("{artist} {bare}").trim().to_owned()];
        if !bare.is_empty() {
            attempts.push(bare.clone());
        }
        attempts.dedup();
        for query in attempts {
            let body: String = match self
                .http
                .get("https://genius.com/api/search/multi")
                .query(&[("q", query)])
                .header("User-Agent", APPLE_UA)
                .timeout(Duration::from_secs(10))
                .send()
                .await
            {
                Ok(r) => match r.error_for_status() {
                    Ok(r) => r.text().await?,
                    Err(e) => {
                        log::info!("genius search: {e}");
                        return Ok(vec![]);
                    }
                },
                Err(e) => return Err(e.into()),
            };
            let v: Value = serde_json::from_str(&body)?;
            let hits = v["response"]["sections"]
                .as_array()
                .and_then(|sections| sections.iter().find(|s| s["type"] == "song"))
                .and_then(|s| s["hits"].as_array())
                .map(Vec::as_slice)
                .unwrap_or_default();
            let t = (if bare.is_empty() { title.clone() } else { bare.clone() }).to_lowercase();
            let a = artist.to_lowercase();
            let mut scored: Vec<(i32, String)> = hits
                .iter()
                .filter_map(|hit| {
                    let ht = hit["result"]["title"].as_str()?.trim().to_lowercase();
                    let ha = hit["result"]["artist_names"].as_str()?.trim().to_lowercase();
                    let title_match = !t.is_empty() && (ht == t || ht.contains(&t) || t.contains(&ht));
                    let artist_match = !a.is_empty() && (ha == a || ha.contains(&a) || a.contains(&ha));
                    if !title_match && !artist_match {
                        return None;
                    }
                    let mut score = (ht == t) as i32 * 50 + title_match as i32 * 25 + artist_match as i32 * if ha == a { 40 } else { 20 };
                    let path = hit["result"]["path"].as_str().unwrap_or_default();
                    if path.to_lowercase().contains("translation") && !t.contains("translation") {
                        score -= 30;
                    }
                    if path.to_lowercase().contains("tracklist") || path.to_lowercase().contains("album-art") {
                        score -= 50;
                    }
                    (score > 0).then(|| {
                        let url = hit["result"]["url"].as_str().unwrap_or_default();
                        (score, url.to_owned())
                    })
                })
                .collect();
            scored.sort_by(|x, y| y.0.cmp(&x.0));
            let Some((_, url)) = scored.first() else { continue };
            if url.is_empty() {
                continue;
            }
            let html = self.http.get(url).header("User-Agent", APPLE_UA).timeout(Duration::from_secs(15)).send().await?.error_for_status()?.text().await?;
            let lines = parse_genius_html(&html);
            if let Some(l) = Lyrics::new(Provider::GeniusPlain, lines, None, vec![]) {
                return Ok(vec![l]);
            }
        }
        Ok(vec![])
    }

    /// Songs fetched lately that came back since `fetch` cached them (YouTube's own
    /// sources arrive after): fold them into the cached set too, so replays keep them.
    pub fn merge_cached(&self, video_id: &str, extra: Vec<Lyrics>) {
        if extra.is_empty() {
            return;
        }
        let mut recent = self.recent.lock().unwrap();
        if let Some((_, set)) = recent.iter_mut().find(|(id, _)| id == video_id) {
            for l in extra {
                if !set.sources.iter().any(|s| s.provider == l.provider) {
                    set.sources.push(l);
                }
            }
            set.sources.sort_by_key(|l| Provider::ALL.iter().position(|p| *p == l.provider));
        }
    }
}

/// PaxSenix's answer as lyrics: its structured Apple payload first, whatever shape
/// it hands over second (`paxResponse`).
fn pax_response(body: &str, dur_ms: i32) -> Vec<Lyrics> {
    if let Some(lines) = parse_paxsenix_timed(body, dur_ms) {
        if let Some(l) = Lyrics::new(Provider::PaxsenixRich, lines, None, vec![]) {
            return vec![l];
        }
    }
    match parse_provider_any(body, dur_ms) {
        Some(lines) => Lyrics::new(Provider::PaxsenixRich, lines, None, vec![]).into_iter().collect(),
        None => vec![],
    }
}

/// The structured Apple payload PaxSenix answers with, keeping its syllable timings
/// (`PaxSenix.parseTimedApple`): rows of `timestamp` plus `text` (word objects with
/// their own timestamps), wherever the answer nests them.
fn parse_paxsenix_timed(raw: &str, song_ms: i32) -> Option<Vec<Line>> {
    let v: Value = serde_json::from_str(raw).ok()?;
    let rows = find_timed_rows(&v)?;
    let mut lines: Vec<Line> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let Some(start) = row["timestamp"].as_i64().map(|t| t as i32) else { continue };
        let Some(words) = row["text"].as_array().filter(|w| !w.is_empty()) else { continue };
        let next_line = rows.get(index + 1).and_then(|r| r["timestamp"].as_i64()).map(|t| t as i32);
        let mut parts: Vec<Part> = Vec::new();
        let mut plain: Vec<&str> = Vec::new();
        for (i, word) in words.iter().enumerate() {
            let Some(t) = word["text"].as_str().map(str::trim).filter(|t| !t.is_empty()) else { continue };
            plain.push(t);
            let Some(ws) = word["timestamp"].as_i64() else { continue };
            let end = word["endtime"]
                .as_i64()
                .or_else(|| words.get(i + 1).and_then(|w| w["timestamp"].as_i64()))
                .or(next_line.map(|n| n as i64))
                .unwrap_or(ws + 800);
            // Apple's karaoke words carry their spacing in a trailing space; a
            // `part: true` word is one half of a split word and keeps its own.
            let text = if word["part"] == true { t.to_owned() } else { format!("{t} ") };
            parts.push(Part { start: ws as i32, dur: ((end - ws).max(0)) as i32, text, bg: false });
        }
        let timed = !plain.is_empty() && parts.len() == plain.len();
        let text = if timed {
            collapse_spaces(&parts.iter().map(|p| p.text.clone()).collect::<String>())
        } else {
            plain.join(" ")
        };
        if text.trim().is_empty() {
            continue;
        }
        let first = parts.first().map(|p| p.start).unwrap_or(start);
        let line_start = start.min(first);
        let dur = match (&next_line, parts.last()) {
            (Some(n), _) => (*n - line_start).max(0),
            (None, Some(p)) => (p.start + p.dur - line_start).max(0),
            (None, None) => 0,
        };
        lines.push(Line { start: line_start, dur, text, parts: if timed { parts } else { vec![] }, ..Default::default() });
    }
    let last = lines.last().map(|l| l.end()).unwrap_or(0);
    let out = insert_instrumentals(lines, song_ms.max(last));
    (!out.is_empty()).then_some(out)
}

/// Genius's own titles carry their own noise: music-note symbols, bracketed
/// credits, packaging words and the producers tail (`geniusClean`).
fn genius_clean(s: &str) -> String {
    static CHARS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\u{266c}\u{266b}\u{2605}\u{2606}\u{3010}\u{3011}\u{300a}\u{300b}\u{300c}\u{300d}~_]").unwrap());
    static PROD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(?:prod(?:uced)?\.?(?:\s+by)?)\s+.*$").unwrap());
    let t = CHARS.replace_all(s, " ");
    let t = GENIUS_NOISE.replace_all(&t, " ");
    let t = PROD.replace_all(&t, " ");
    collapse_spaces(t.split(" | ").next().unwrap_or(""))
}

/// The first array anywhere whose elements are timed rows (`timestamp` plus a
/// `text` list), depth first.
fn find_timed_rows(v: &Value) -> Option<&Vec<Value>> {
    if let Some(items) = v.as_array() {
        return items.iter().find_map(find_timed_rows);
    }
    let o = v.as_object()?;
    if let Some(content) = o.get("content").and_then(Value::as_array) {
        if content.iter().any(|c| c.as_object().is_some_and(|c| c.contains_key("timestamp"))) {
            return Some(content);
        }
    }
    o.values().find_map(find_timed_rows)
}

/// Whatever a provider answered that isn't its structured form, shaped like
/// ForkChord's `parseProvider`: TTML when it looks like it, QQ's karaoke LRC when
/// it's stamped like it, then enhanced LRC / LRC, then plain text.
fn parse_provider_any(raw: &str, dur_ms: i32) -> Option<Vec<Line>> {
    static TTML: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<tt(\s|>)").unwrap());
    static KARAOKE_LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\[\d{1,8},\d{1,8}\]").unwrap());
    static KARAOKE_WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\(\d{1,8},\d{1,8}(?:,\d{1,8})?\)").unwrap());
    let content = unwrap_lyrics(raw)?;
    if content.contains("http://www.w3.org/ns/ttml") || TTML.is_match(&content) {
        let t = parse_ttml(&content, dur_ms);
        return (!t.lines.is_empty()).then_some(t.lines);
    }
    if content.trim_start().starts_with('<') {
        return None;
    }
    let looks_karaoke = content.contains("LyricContent=\"")
        || content.split('\n').any(|l| KARAOKE_LINE.is_match(l.trim()) && KARAOKE_WORD.is_match(l));
    let lines = if looks_karaoke {
        parse_qrc(&content, dur_ms, None)
    } else {
        parse_lrc(&content, dur_ms)
    };
    if lines.iter().any(|l| !l.text.trim().is_empty()) {
        return Some(lines);
    }
    let plain = parse_plain(&content);
    plain.iter().any(|l| !l.text.trim().is_empty()).then_some(plain)
}

/// A provider answer that is a JSON wrapper around the lyrics (`unwrap`): the
/// string under "lyrics", or the strings of an array, or the text itself.
fn unwrap_lyrics(raw: &str) -> Option<String> {
    let value = raw.trim().trim_start_matches('\u{feff}').trim();
    if value.is_empty() {
        return None;
    }
    match serde_json::from_str::<Value>(value) {
        Ok(v) => extract_lyrics(&v),
        Err(_) => Some(value.to_owned()),
    }
}

fn extract_lyrics(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => {
            let t = s.trim();
            // One more layer: providers wrap escaped JSON inside strings.
            if (t.starts_with('{') || t.starts_with('[')) && serde_json::from_str::<Value>(t).is_ok() {
                return serde_json::from_str::<Value>(t).ok().and_then(|n| extract_lyrics(&n)).or_else(|| Some(t.to_owned()));
            }
            (!t.is_empty()).then(|| t.to_owned())
        }
        Value::Array(items) => {
            let joined = items.iter().filter_map(extract_lyrics).collect::<Vec<_>>().join("\n");
            (!joined.is_empty()).then_some(joined)
        }
        Value::Object(o) => o.get("lyrics").and_then(extract_lyrics),
        _ => None,
    }
}

/// LyricsPlus's answer as lines (`parseLyricsPlus`): every timed row, its syllabus
/// as parts when it has one, singers named v1/v2 in order of appearance.
fn from_lyricsplus(v: &Value, song_ms: i32) -> Vec<Line> {
    let rows = v["lyrics"].as_array().map(Vec::as_slice).unwrap_or_default();
    let mut singer_names: Vec<String> = Vec::new();
    let mut lines: Vec<Line> = rows
        .iter()
        .filter_map(|l| {
            let Some(time) = l["time"].as_i64().map(|t| t as i32) else { return None };
            let parts: Vec<Part> = l["syllabus"]
                .as_array()
                .map(|syll| {
                    syll.iter()
                        .filter_map(|s| {
                            let text = s["text"].as_str()?;
                            let st = s["time"].as_i64()?;
                            let dur = s["duration"].as_i64().unwrap_or(0);
                            Some(Part { start: st as i32, dur: dur.max(0) as i32, text: text.to_owned(), bg: false })
                        })
                        .collect()
                })
                .unwrap_or_default();
            let text = if parts.is_empty() {
                l["text"].as_str().map(|t| t.trim().to_owned()).filter(|t| !t.is_empty())
            } else {
                let joined = collapse_spaces(&parts.iter().map(|p| p.text.clone()).collect::<String>());
                (!joined.is_empty()).then_some(joined)
            };
            let text = text?;
            let dur = l["duration"].as_i64().map(|d| d.max(0) as i32).unwrap_or(0);
            let agent = l["element"].as_object().and_then(|e| e.get("singer")).and_then(Value::as_str).map(|singer| {
                let at = match singer_names.iter().position(|n| n == singer) {
                    Some(i) => i,
                    None => {
                        singer_names.push(singer.to_owned());
                        singer_names.len() - 1
                    }
                };
                format!("v{}", at + 1)
            });
            let line = if parts.is_empty() {
                Line { start: time, dur: if dur > 0 { dur } else { 0 }, text, agent, ..Default::default() }
            } else {
                // Parts carry the line's clock: keep the row's start when it leads them.
                let first = parts.iter().map(|p| p.start).min().unwrap_or(time);
                Line { start: time.min(first), dur, text, parts, agent, ..Default::default() }
            };
            Some(line)
        })
        .collect();
    lines.sort_by_key(|l| l.start);
    collapse_zero(insert_instrumentals(lines, song_ms))
}

/// Genius's page HTML as plain lines (`parseGeniusHtml`): the lyrics containers,
/// their tags off, `<br>`s as line breaks, section headers like "[Chorus]" kept
/// and stanza breaks kept.
fn parse_genius_html(html: &str) -> Vec<Line> {
    const CONTAINER: &str = "data-lyrics-container=\"true\"";
    if !html.contains(CONTAINER) {
        // The legacy layout keeps the lyrics in one div.lyrics.
        return genius_block(html, "<div class=\"lyrics\"");
    }
    let mut text = String::new();
    // Every container: the attribute opens it, the matching `</div` closes it
    // (its depth walked, however nested the bits Genius puts inside are).
    let mut at = 0;
    while let Some(found) = html[at..].find(CONTAINER) {
        let Some((start, end)) = element_body(html, at + found) else { break };
        text.push_str(&strip_genius_extras(&html[start..end]));
        text.push('\n');
        at = end;
    }
    genius_lines(&text)
}

/// The lyrics out of the container whose tag opens at `tag_at`, when the page
/// serves the legacy `<div class="lyrics">` block instead.
fn genius_block(html: &str, opener: &str) -> Vec<Line> {
    let Some(at) = html.find(opener) else { return vec![] };
    match element_body(html, at) {
        Some((start, end)) => genius_lines(&strip_genius_extras(&html[start..end])),
        None => vec![],
    }
}

/// The inside of the element whose opening tag starts at `tag_at` (at the `<`):
/// from after its own `>` to the matching close, its tag name and depth honoured.
/// The element may hold more of itself; anything else inside is somebody else's.
fn element_body(html: &str, tag_at: usize) -> Option<(usize, usize)> {
    let after = &html[tag_at..];
    let name: String = after[1..].chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
    if name.is_empty() {
        return None;
    }
    let gt = after.find('>')? + tag_at + 1;
    let start = gt;
    let open = format!("<{name}");
    let close = format!("</{name}");
    let mut depth = 1usize;
    let mut scan = start;
    while scan < html.len() {
        let open_at = html[scan..].find(&open).map(|i| scan + i);
        let close_at = html[scan..].find(&close).map(|i| scan + i);
        match (open_at, close_at) {
            (Some(o), Some(c)) if o < c => {
                depth += 1;
                scan = o + open.len();
            }
            (_, Some(c)) => {
                depth -= 1;
                if depth == 0 {
                    return Some((start, c.min(html.len())));
                }
                scan = c + close.len();
            }
            (Some(o), None) => {
                depth += 1;
                scan = o + open.len();
            }
            _ => break,
        }
    }
    // Never closed: the rest of the page is the best there is.
    Some((start, html.len()))
}

/// The bits Genius nests inside a lyrics container that aren't lyrics: scripts,
/// styles, buttons, and the header/ad/footer blocks its classes and attributes
/// name, all removed with their whole elements.
fn strip_genius_extras(container: &str) -> String {
    static BLOCKS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)<script\b.*?</script>|<style\b.*?</style>|<button\b.*?</button>").unwrap());
    let mut out = BLOCKS.replace_all(container, "\n").into_owned();
    for marker in ["data-exclude-from-selection=\"true\"", "LyricsHeader__Container", "SongBioPreview", "InreadAd", "LyricsFooter"] {
        while let Some(i) = out.find(marker) {
            let Some(tag_at) = out[..i].rfind('<') else { break };
            if let Some((_content_start, close_at)) = element_body(&out, tag_at) {
                // The whole element goes, closing tag and all.
                let end = out[close_at..].find('>').map(|g| close_at + g + 1).unwrap_or(out.len());
                out.replace_range(tag_at..end.min(out.len()), "\n");
            } else {
                break;
            }
        }
    }
    out
}

/// A cleaned container's HTML as Genius's own text: `<br>`s and paragraph ends as
/// newlines, every other tag off, entities decoded, the Embed, "You might also
/// like" and ad tails off, stanza breaks kept as empty lines.
fn genius_lines(container_html: &str) -> Vec<Line> {
    static BR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)<br\s*/?>|</p>|</h\d>|</blockquote>").unwrap());
    static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]*>").unwrap());
    static EMBED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\d*Embed\s*$").unwrap());
    static ALSO_LIKE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\d*You might also like").unwrap());
    let with_breaks = BR.replace_all(container_html, "\n").into_owned();
    let no_tags = TAG.replace_all(&with_breaks, "").into_owned();
    let decoded = decode_entities(&no_tags.replace('\u{feff}', " ").replace('\u{200b}', " "));
    let mut out: Vec<Line> = Vec::new();
    for raw in decoded.split('\n') {
        let t = collapse_spaces(&ALSO_LIKE.replace_all(raw, " "));
        let t = EMBED.replace_all(&t, "").trim().to_owned();
        if t.is_empty() {
            // Stanza breaks stay as one empty line between sung lines.
            if out.last().is_some_and(|l| !l.text.is_empty()) {
                out.push(Line { text: String::new(), ..Default::default() });
            }
        } else {
            out.push(Line { text: t, ..Default::default() });
        }
    }
    while out.first().is_some_and(|l| l.text.is_empty()) {
        out.remove(0);
    }
    while out.last().is_some_and(|l| l.text.is_empty()) {
        out.pop();
    }
    out
}

/// The entities Genius's text carries: the named ones and the numeric forms.
fn decode_entities(text: &str) -> String {
    static NAMED: &[(&str, &str)] = &[("&quot;", "\""), ("&apos;", "'"), ("&#39;", "'"), ("&#x27;", "'"), ("&#039;", "'"), ("&lt;", "<"), ("&gt;", ">"), ("&nbsp;", "\u{a0}"), ("&amp;", "&")];
    static NUMERIC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"&#(x?)([0-9a-fA-F]+);").unwrap());
    let mut out = text.to_owned();
    for (from, to) in NAMED {
        out = out.replace(from, to);
    }
    NUMERIC.replace_all(&out, |c: &regex::Captures| {
        let (hex, digits) = (&c[1], &c[2]);
        let code = u32::from_str_radix(digits, if hex.is_empty() { 10 } else { 16 }).ok().and_then(char::from_u32);
        code.map(|c| c.to_string()).unwrap_or_default()
    })
    .into_owned()
}

/// Any run of spaces down to one, spaces at both ends off.
fn collapse_spaces(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Plain lines keep their stanza breaks; a run of empties is one empty.
fn collapse_zero(lines: Vec<Line>) -> Vec<Line> {
    let mut out: Vec<Line> = Vec::new();
    for l in lines {
        if l.text.trim().is_empty() && l.parts.is_empty() && out.last().is_some_and(|p| p.text.trim().is_empty() && p.parts.is_empty()) {
            continue;
        }
        out.push(l);
    }
    out
}

/// A search answer's most likely recording (PaxSenix.bestCandidate): every object
/// in the answer that names an id and a title is a candidate, scored by how its
/// title and artist match and how close its length is.
fn best_candidate(root: &Value, title: &str, artist: &str, dur_ms: i32) -> Option<String> {
    let mut best: Option<(i32, String)> = None;
    walk_candidates(root, &mut |id, c_title, c_artist, c_dur| {
        let mut score = text_score(c_title, title, 20, 10) + text_score(c_artist, artist, 15, 5);
        if dur_ms > 0 && c_dur > 0 {
            let diff = (c_dur - dur_ms).abs();
            score += if diff < 3000 { 10 } else if diff < 10_000 { 5 } else { 0 };
        }
        if best.as_ref().is_none_or(|(s, _)| score > *s) {
            best = Some((score, id.to_owned()));
        }
    });
    best.filter(|(s, _)| *s >= 10).map(|(_, id)| id)
}

fn walk_candidates(v: &Value, into: &mut impl FnMut(&str, &str, &str, i32)) {
    match v {
        Value::Array(items) => items.iter().for_each(|i| walk_candidates(i, into)),
        Value::Object(o) => {
            // The id may sit on the node or its attributes; the title always on the
            // attributes (Apple) or the node itself (Spotify-shaped answers).
            let attrs = o.get("attributes").filter(|a| a.is_object()).unwrap_or(v);
            let id = first_string(v, &["id", "trackId", "track_id", "realId"]).or_else(|| first_string(attrs, &["id", "trackId", "track_id", "realId"]));
            let title = first_string(attrs, &["name", "title", "trackName", "track_name"]);
            if let (Some(id), Some(title)) = (id, title) {
                let artist = first_string(attrs, &["artistName", "artist_name"]).or_else(|| artist_names(attrs)).unwrap_or_default();
                let dur = first_number(attrs, &["durationInMillis", "durationMs", "duration_ms", "duration"]).map(to_duration_ms).unwrap_or(0);
                into(&id, &title, &artist, i32::try_from(dur).unwrap_or(0));
            }
            o.values().for_each(|c| walk_candidates(c, into));
        }
        _ => {}
    }
}

/// The first key of `keys` holding a non-blank string (numbers come as strings).
fn first_string(o: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        match o.get(key) {
            Some(Value::String(s)) if !s.trim().is_empty() => return Some(s.trim().to_owned()),
            Some(Value::Number(n)) => return Some(n.to_string()),
            _ => {}
        }
    }
    None
}

fn first_number(o: &Value, keys: &[&str]) -> Option<i64> {
    keys.iter().find_map(|k| o.get(k).and_then(Value::as_i64))
}

/// Search answers give lengths in ms or in seconds; 9999 and under can only be
/// seconds (`toDurationMs`).
fn to_duration_ms(n: i64) -> i64 {
    if n <= 0 {
        0
    } else if n < 10_000 {
        n * 1000
    } else {
        n
    }
}

/// The names of an answer's artists, however they are shaped: a string, an array
/// of names or objects, or one object (`artistNames`).
fn artist_names(o: &Value) -> Option<String> {
    let artists = o.get("artists").or_else(|| o.get("artist"))?;
    match artists {
        Value::String(s) => (!s.trim().is_empty()).then(|| s.trim().to_owned()),
        Value::Array(items) => {
            let joined = items
                .iter()
                .filter_map(|a| match a {
                    Value::String(s) => Some(s.trim().to_owned()),
                    Value::Object(_) => first_string(a, &["name", "artistName", "title"]),
                    _ => None,
                })
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(", ");
            (!joined.is_empty()).then_some(joined)
        }
        Value::Object(_) => first_string(artists, &["name", "artistName", "title"]),
        _ => None,
    }
}

/// 0 when either text is blank, `exact` when they are the same, `partial` when one
/// contains the other (`textScore`).
fn text_score(candidate: &str, wanted: &str, exact: i32, partial: i32) -> i32 {
    let c = candidate.trim().to_lowercase();
    let w = wanted.trim().to_lowercase();
    if c.is_empty() || w.is_empty() {
        return 0;
    }
    if c == w {
        exact
    } else if c.contains(&w) || w.contains(&c) {
        partial
    } else {
        0
    }
}

// ------------------------------------------------------------------- provider results

/// Every provider's lyrics out of a complete SSE response (`processStreamData`).
pub fn from_stream(body: &str, q: &Query) -> Vec<Lyrics> {
    let mut out: Vec<Lyrics> = Vec::new();
    let mut dur_ms = q.duration.unwrap_or(0) as i32 * 1000;
    let (mut title, mut artist) = (q.song.to_owned(), q.artist.to_owned());
    for block in body.split("\n\n").flat_map(|b| b.split("\r\n\r\n")) {
        let mut event = "";
        let mut data = String::new();
        for line in block.lines() {
            if let Some(e) = line.strip_prefix("event:") {
                event = e.trim();
            } else if let Some(d) = line.strip_prefix("data:") {
                data.push_str(d.trim());
            }
        }
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&data) else { continue };
        if event == "metadata" {
            if let Some(d) = v["duration"].as_str().and_then(|d| d.parse::<f64>().ok()).or_else(|| v["duration"].as_f64()) {
                dur_ms = (d * 1000.0) as i32;
            }
            if let Some(s) = v["song"].as_str() {
                title = s.to_owned();
            }
            if let Some(a) = v["artist"].as_str() {
                artist = a.to_owned();
            }
            continue;
        }
        let r = &v["results"];
        let s = |k: &str| r[k].as_str().filter(|s| !s.is_empty());
        let mut push = |p: Provider, lines: Vec<Line>, lang: Option<String>, writers: Vec<String>| {
            if let Some(l) = Lyrics::new(p, lines, lang, writers) {
                out.retain(|o| o.provider != p);
                out.push(l);
            }
        };
        match v["provider"].as_str().unwrap_or_default() {
            "musixmatch" => {
                if let Some(t) = s("wordByWord") {
                    let mut lines = parse_lrc(t, dur_ms);
                    lrc_fixers(&mut lines);
                    push(Provider::MusixmatchRich, lines, None, lrc_songwriters(t));
                }
                if let Some(t) = s("synced") {
                    push(Provider::MusixmatchSynced, parse_lrc(t, dur_ms), None, lrc_songwriters(t));
                }
            }
            "lrclib" => {
                if let Some(t) = s("synced") {
                    push(Provider::LrclibSynced, parse_lrc(t, dur_ms), None, lrc_songwriters(t));
                }
                if let Some(t) = s("plain") {
                    push(Provider::LrclibPlain, parse_plain(t), None, vec![]);
                }
            }
            "kugou" => {
                if let Some(t) = s("lyrics").and_then(|t| serde_json::from_str::<Value>(t).ok()).and_then(|j| j["lyrics"].as_str().map(str::to_owned)) {
                    push(Provider::LegatoSynced, parse_lrc(&t, dur_ms), None, lrc_songwriters(&t));
                }
            }
            "qq" => {
                if let Some(t) = s("lyrics").and_then(|t| serde_json::from_str::<Value>(t).ok()).and_then(|j| j["lyrics"].as_str().map(str::to_owned)) {
                    push(Provider::PortatoRich, parse_qrc(&t, dur_ms, Some((&title, &artist))), None, qrc_songwriters(&t));
                }
            }
            "golyrics" => {
                if let Some(raw) = s("lyrics") {
                    let ttml = serde_json::from_str::<Value>(raw).ok().and_then(|j| j["ttml"].as_str().map(str::to_owned)).unwrap_or_else(|| raw.to_owned());
                    let t = parse_ttml(&ttml, dur_ms);
                    let p = if t.word_synced { Provider::BLyricsRich } else { Provider::BLyricsSynced };
                    push(p, t.lines, t.language, t.songwriters);
                }
            }
            "binimum" => {
                if let Some(raw) = s("lyrics") {
                    let t = parse_ttml(raw, dur_ms);
                    let word = match r["timingType"].as_str() {
                        Some("syllable") => true,
                        Some("line") => false,
                        _ => t.word_synced,
                    };
                    let p = if word { Provider::BinimumRich } else { Provider::BinimumSynced };
                    push(p, t.lines, t.language, t.songwriters);
                }
            }
            _ => {}
        }
    }
    out
}

/// A Unison answer (`unison.ts`): TTML, LRC or plain text.
fn from_unison(data: &Value, dur_ms: i32) -> Vec<Lyrics> {
    let (Some(format), Some(text)) = (data["format"].as_str(), data["lyrics"].as_str()) else { return vec![] };
    let lyrics = match format {
        "ttml" => {
            let t = parse_ttml(text, dur_ms);
            let p = if t.word_synced { Provider::UnisonRich } else { Provider::UnisonSynced };
            Lyrics::new(p, t.lines, t.language, t.songwriters)
        }
        "lrc" => {
            let p = if data["syncType"] == "richsync" { Provider::UnisonWord } else { Provider::UnisonSynced };
            Lyrics::new(p, parse_lrc(text, dur_ms), data["language"].as_str().map(str::to_owned), lrc_songwriters(text))
        }
        "plain" => Lyrics::new(Provider::UnisonPlain, parse_plain(text), None, vec![]),
        _ => None,
    };
    lyrics.into_iter().collect()
}

/// The lrc.red search hit closest to the track: same title (a remaster or live
/// suffix aside), a matching artist, and within twenty seconds of the known length.
fn best_lrcred_hit<'a>(hits: &'a [Value], q: &Query<'_>) -> Option<&'a Value> {
    let title = fold_title(q.song);
    let artist = fold(q.artist);
    let dur = q.duration.filter(|d| *d > 0).map(f64::from);
    let mut best: Option<(i32, &Value)> = None;
    for h in hits {
        let ht = fold_title(h["title"].as_str().unwrap_or_default());
        let ha = fold(h["artist"].as_str().unwrap_or_default());
        let title_ok = !title.is_empty() && (ht == title || ht.contains(&title) || title.contains(&ht));
        let artist_ok = artist.is_empty() || ha == artist || ha.contains(&artist) || artist.contains(&ha);
        if !title_ok || !artist_ok {
            continue;
        }
        let diff = match (dur, h["duration"].as_f64()) {
            (Some(a), Some(b)) => (a - b).abs(),
            _ => 0.0,
        };
        if diff > 20.0 {
            continue;
        }
        let base = if ht == title { 100 } else { 60 };
        let score = base + (20.0 - diff) as i32;
        if best.is_none_or(|(s, _)| score > s) {
            best = Some((score, h));
        }
    }
    best.map(|(_, h)| h)
}

/// Letters and digits only, lower case, single-spaced.
fn fold(s: &str) -> String {
    s.chars().map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { ' ' }).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A title without its bracketed or dashed qualifiers: "(Remastered 2011)", " - Live".
fn fold_title(s: &str) -> String {
    let s = s.split(" (").next().unwrap_or(s);
    let s = s.split(" [").next().unwrap_or(s);
    let s = s.split(" - ").next().unwrap_or(s);
    fold(s)
}

// ------------------------------------------------------------------- shared helpers

/// Gaps over this long between sung lines become instrumental lines.
const INSTRUMENTAL_GAP_MS: i32 = 5000;

/// `et`: instrumental lines for long gaps before, between and after the sung lines.
fn insert_instrumentals(lines: Vec<Line>, song_ms: i32) -> Vec<Line> {
    if lines.is_empty() {
        return lines;
    }
    let gap = |start: i32, dur: i32| Line { start, dur, instrumental: true, ..Default::default() };
    let mut out = Vec::with_capacity(lines.len() + 4);
    if lines[0].start > INSTRUMENTAL_GAP_MS {
        out.push(gap(0, lines[0].start));
    }
    let n = lines.len();
    let last_end = lines[n - 1].end();
    for (i, l) in lines.iter().enumerate() {
        out.push(l.clone());
        if i + 1 < n {
            let end = l.end();
            let f = lines[i + 1].start - end;
            if f > INSTRUMENTAL_GAP_MS {
                out.push(gap(end, f));
            }
        }
    }
    if song_ms - last_end > INSTRUMENTAL_GAP_MS {
        out.push(gap(last_end, song_ms - last_end));
    }
    out
}

// Credit lines ("作词 : …", "Written by: …") that some providers put in the lyrics.
const WRITER_TAGS: &[&str] = &["词", "詞", "作词", "作詞", "曲", "作曲", "词曲", "詞曲", "作词作曲", "作詞作曲", "writtenby", "lyricsby", "composedby", "lyricist", "composer"];
const CREDIT_TAGS: &[&str] = &[
    "编曲", "編曲", "和声", "和聲", "混音", "吉他", "制作人", "製作人", "演唱", "原唱", "翻唱", "后期", "後期", "和音", "录音", "錄音", "策划", "策劃", "伴奏", "美工", "海报", "海報", "旁白", "producedby",
    "arrangedby", "mixing", "mastering", "vocal", "vocals", "guitar", "bass", "drums", "producer", "arranger",
];
const CREDIT_SUFFIXES: &[char] = &['词', '詞', '曲', '声', '聲', '音'];

fn norm_tag(t: &str) -> String {
    t.to_lowercase().split_whitespace().collect()
}

fn is_credit_tag(t: &str) -> bool {
    WRITER_TAGS.contains(&t) || CREDIT_TAGS.contains(&t) || (t.chars().count() <= 4 && t.ends_with(CREDIT_SUFFIXES))
}

fn tag_pieces(n: &str) -> Vec<String> {
    let no_latin: String = n.chars().filter(|c| !c.is_ascii_lowercase()).collect();
    no_latin.split(['/', '&', '、', ',', '，', '・', '·']).filter(|s| !s.is_empty()).map(str::to_owned).collect()
}

/// `X`: a label that only names credits.
fn is_credit_label(t: &str) -> bool {
    let n = norm_tag(t);
    if WRITER_TAGS.contains(&n.as_str()) || CREDIT_TAGS.contains(&n.as_str()) {
        return true;
    }
    let pieces = tag_pieces(&n);
    !pieces.is_empty() && pieces.iter().all(|p| is_credit_tag(p))
}

fn is_writer_label(t: &str) -> bool {
    let n = norm_tag(t);
    WRITER_TAGS.contains(&n.as_str()) || (is_credit_label(t) && tag_pieces(&n).iter().any(|p| WRITER_TAGS.contains(&p.as_str())))
}

fn split_label(t: &str) -> Option<(&str, &str)> {
    let i = t.find([':', '：'])?;
    let sep = t[i..].chars().next()?.len_utf8();
    let (name, rest) = (&t[..i], t[i + sep..].trim_start());
    (!name.is_empty() && !rest.is_empty()).then_some((name, rest))
}

/// `yt`: a "Label: names" credit line rather than a lyric.
fn is_credit_line(t: &str) -> bool {
    split_label(t.trim()).is_some_and(|(name, _)| is_credit_label(name))
}

fn split_names(t: &str) -> Vec<String> {
    t.split(['/', '、', '，', ',']).map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect()
}

/// `tt`: the writers named on a credit line.
fn writers_on(t: &str) -> Vec<String> {
    match split_label(t.trim()) {
        Some((name, rest)) if is_writer_label(name) => split_names(rest),
        _ => vec![],
    }
}

fn dedup(names: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for n in names {
        let n = n.trim().to_owned();
        if !n.is_empty() && !out.contains(&n) {
            out.push(n);
        }
    }
    out
}

// ------------------------------------------------------------------- LRC

/// `V`: an LRC timestamp (`mm:ss.xx`, `h:mm:ss.xx` or seconds) in ms.
fn lrc_time(t: &str) -> i32 {
    let p: Vec<&str> = t.split(':').collect();
    let s = match p.len() {
        1 => p[0].parse::<f64>().unwrap_or(0.0) * 1000.0,
        2 => p[0].parse::<f64>().unwrap_or(0.0).trunc() * 60_000.0 + p[1].parse::<f64>().unwrap_or(0.0) * 1000.0,
        3 => p[0].parse::<f64>().unwrap_or(0.0).trunc() * 3_600_000.0 + p[1].parse::<f64>().unwrap_or(0.0).trunc() * 60_000.0 + p[2].parse::<f64>().unwrap_or(0.0) * 1000.0,
        _ => 0.0,
    };
    s.round() as i32
}

const LRC_META_TAGS: &[&str] = &["ti", "ar", "al", "au", "lr", "length", "by", "offset", "re", "tool", "ve", "#"];

/// `[mm:ss.xx]` tags: (byte range, time text).
fn line_stamps(s: &str) -> Vec<(std::ops::Range<usize>, &str)> {
    stamps(s, '[', ']')
}

/// `<mm:ss.xx>` word stamps.
fn word_stamps(s: &str) -> Vec<(std::ops::Range<usize>, &str)> {
    stamps(s, '<', '>')
}

fn stamps(s: &str, open: char, close: char) -> Vec<(std::ops::Range<usize>, &str)> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(o) = s[i..].find(open) {
        let o = i + o;
        let Some(c) = s[o + 1..].find(close) else { break };
        let inner = &s[o + 1..o + 1 + c];
        let is_time = inner.split_once(':').is_some_and(|(m, sec)| {
            !m.is_empty() && m.bytes().all(|b| b.is_ascii_digit()) && sec.split_once('.').is_some_and(|(a, b)| !a.is_empty() && !b.is_empty() && a.bytes().chain(b.bytes()).all(|b| b.is_ascii_digit()))
        });
        if is_time {
            out.push((o..o + c + 2, inner));
            i = o + c + 2;
        } else {
            i = o + open.len_utf8();
        }
    }
    out
}

/// Text and stamps alternating, the way JS `split` with a capture group returns them.
fn split_stamps(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut at = 0;
    for (r, t) in word_stamps(s) {
        out.push(&s[at..r.start]);
        out.push(t);
        at = r.end;
    }
    out.push(&s[at..]);
    out
}

fn remove_ranges(s: &str, ranges: &[(std::ops::Range<usize>, &str)]) -> String {
    let mut out = String::with_capacity(s.len());
    let mut at = 0;
    for (r, _) in ranges {
        out.push_str(&s[at..r.start]);
        at = r.end;
    }
    out.push_str(&s[at..]);
    out
}

/// `parseLRC`: line LRC, enhanced (word-by-word) LRC and `[bg: …]` background vocals.
pub fn parse_lrc(text: &str, song_ms: i32) -> Vec<Line> {
    let mut lines: Vec<Line> = Vec::new();
    let mut meta: HashMap<String, String> = HashMap::new();
    for raw in text.split('\n') {
        let a = raw.trim();
        if let Some(inner) = a.strip_prefix('[').and_then(|r| r.strip_suffix(']'))
            && let Some((tag, val)) = inner.split_once(':')
            && !tag.is_empty()
            && tag.chars().all(|c| c.is_alphanumeric() || c == '_')
            && LRC_META_TAGS.contains(&tag)
        {
            meta.insert(tag.to_owned(), val.to_owned());
            continue;
        }
        let ls = line_stamps(a);
        if ls.is_empty() {
            continue;
        }
        let times: Vec<i32> = ls.iter().map(|(_, t)| lrc_time(t)).collect();
        let (c, u) = (*times.iter().min().unwrap(), *times.iter().max().unwrap());
        let g = remove_ranges(a, &ls).trim().to_owned();
        // `[bg: …]` at the end of the line.
        let (main, bg) = match g.rfind("[bg:") {
            Some(i) if g[i..].trim_end().ends_with(']') => {
                let inner = g[i + 4..].trim_end();
                (g[..i].to_owned(), Some(inner[..inner.len() - 1].to_owned()))
            }
            _ => (g.clone(), None),
        };
        let o = split_stamps(&main);
        // Whitespace-only text between two stamps: the words are spaced by their own segments.
        let spaced = o.iter().enumerate().any(|(e, w)| e % 2 == 0 && e > 0 && e < o.len() - 1 && !w.is_empty() && w.trim().is_empty());
        let mut parts: Vec<Part> = Vec::new();
        let mut prev: Option<i32> = None;
        let mut b = String::new();
        let mut walk = |w: &str, is_bg: bool, parts: &mut Vec<Part>, b: &mut String| {
            for (m, seg) in split_stamps(w).into_iter().enumerate() {
                if m % 2 == 0 {
                    let mut d = seg.to_owned();
                    if spaced {
                        let t = d.trim();
                        d = if t.is_empty() && !d.is_empty() { " ".to_owned() } else { t.to_owned() };
                    }
                    b.push_str(&d);
                    if let Some(last) = parts.last_mut() {
                        last.text.push_str(&d);
                    }
                } else {
                    let d = lrc_time(seg);
                    if let (Some(p), Some(last)) = (prev, parts.last_mut()) {
                        last.dur = d - p;
                    }
                    parts.push(Part { start: d, dur: 0, text: String::new(), bg: is_bg });
                    prev = Some(d);
                }
            }
        };
        walk(&main, false, &mut parts, &mut b);
        if let Some(bg) = &bg {
            walk(bg, true, &mut parts, &mut b);
        }
        let plain: String = remove_ranges(&main, &word_stamps(&main));
        if is_credit_line(&plain) {
            continue;
        }
        lines.push(Line { start: c, dur: u - c, text: b.trim().to_owned(), parts, ..Default::default() });
    }
    let n = lines.len();
    for l in 0..n {
        if l + 1 < n {
            let next_start = lines[l + 1].start;
            let a = &mut lines[l];
            if a.dur == 0 {
                a.dur = (next_start - a.start).max(0);
            }
            if !a.parts.is_empty() {
                let mut c = next_start;
                for f in &a.parts {
                    c = c.max(f.start);
                }
                let last = a.parts.last_mut().unwrap();
                last.dur = (next_start - last.start).max(0);
                a.dur = (c - a.start).max(0);
            }
        } else {
            let a = &mut lines[l];
            let i = song_ms - a.start;
            if a.dur == 0 && i > 0 {
                a.dur = i;
            }
            if let Some(last) = a.parts.last_mut() {
                let u = song_ms - last.start;
                if u > 0 {
                    last.dur = u;
                }
            }
        }
    }
    if let Some(off) = meta.get("offset") {
        let off = (off.trim().parse::<f64>().unwrap_or(0.0) * 1000.0) as i32;
        for a in &mut lines {
            a.start -= off;
            for p in &mut a.parts {
                p.start -= off;
            }
        }
    }
    lines
}

/// `lrcFixers`: Musixmatch word-by-word quirks (spaces carrying a word's tail, and
/// files where most words were given a token 0–100 ms).
pub fn lrc_fixers(lines: &mut [Line]) {
    for e in lines.iter_mut() {
        for r in 1..e.parts.len() {
            let (l, o) = {
                let (a, b) = e.parts.split_at_mut(r);
                (&mut a[r - 1], &mut b[0])
            };
            if o.text == " " && l.text != " " {
                let a = o.dur - l.dur;
                if a.abs() <= 15 || o.dur <= 100 {
                    let i = o.dur;
                    l.dur += i;
                    o.dur -= i;
                    o.start += i;
                }
            }
        }
    }
    let (mut short, mut total) = (0, 0);
    for e in lines.iter() {
        if e.parts.is_empty() {
            continue;
        }
        for o in e.parts.iter().take(e.parts.len().saturating_sub(2)) {
            if o.text != " " {
                if o.dur <= 100 {
                    short += 1;
                }
                total += 1;
            }
        }
    }
    if total > 0 && short as f32 / total as f32 > 0.5 {
        for e in 0..lines.len() {
            for o in 0..lines[e].parts.len() {
                if lines[e].parts[o].text == " " || lines[e].parts[o].dur > 400 {
                    continue;
                }
                let next = if o + 1 < lines[e].parts.len() {
                    Some((e, o + 1))
                } else if e + 1 < lines.len() && !lines[e + 1].parts.is_empty() {
                    Some((e + 1, 0))
                } else {
                    None
                };
                match next {
                    None => lines[e].parts[o].dur = 300,
                    Some((ne, no)) => {
                        let a = lines[ne].parts[no].clone();
                        if a.text == " " {
                            lines[e].parts[o].dur += a.dur;
                            let a = &mut lines[ne].parts[no];
                            a.start += a.dur;
                            a.dur = 0;
                        } else {
                            lines[e].parts[o].dur = a.start - lines[e].parts[o].start;
                        }
                    }
                }
            }
        }
    }
}

fn lrc_songwriters(text: &str) -> Vec<String> {
    let mut n = Vec::new();
    for s in text.split('\n') {
        let e = s.trim();
        if let Some(inner) = e.strip_prefix('[').and_then(|r| r.strip_suffix(']'))
            && let Some((tag, val)) = inner.split_once(':')
            && (tag == "au" || tag == "lr")
        {
            n.extend(split_names(val));
            continue;
        }
        let ls = line_stamps(e);
        if !ls.is_empty() {
            let o = remove_ranges(e, &ls);
            let o = match o.rfind("[bg:") {
                Some(i) => o[..i].to_owned(),
                None => o,
            };
            n.extend(writers_on(&remove_ranges(&o, &word_stamps(&o))));
        }
    }
    dedup(n)
}

pub fn parse_plain(text: &str) -> Vec<Line> {
    text.split('\n').map(|s| Line { text: s.to_owned(), ..Default::default() }).collect()
}

// ------------------------------------------------------------------- QRC

struct QrcLine {
    time: i32,
    duration: i32,
    text: String,
    syllables: Vec<(String, i32, i32)>,
    agent: Option<String>,
}

struct Singers {
    aliases: HashMap<String, String>,
    next: u32,
    current: Option<String>,
}

fn qrc_content(t: &str) -> String {
    let Some(i) = t.find("LyricContent=\"") else { return t.to_owned() };
    let rest = &t[i + 14..];
    // Up to the closing quote that ends the attribute (`"/>`, `">` or `" name=`).
    let mut end = None;
    for (j, _) in rest.match_indices('"') {
        let after = rest[j + 1..].trim_start();
        if after.starts_with("/>") || after.starts_with('>') || after.split_once('=').is_some_and(|(k, _)| !k.is_empty() && k.chars().all(|c| c.is_ascii_alphabetic())) {
            end = Some(j);
            break;
        }
    }
    let Some(end) = end else { return t.to_owned() };
    rest[..end].replace("&quot;", "\"").replace("&amp;", "&")
}

/// `[start,duration]` at the start of a QRC line.
fn qrc_header(t: &str) -> Option<(i32, i32, &str)> {
    let inner = t.strip_prefix('[')?;
    let close = inner.find(']')?;
    let (a, b) = inner[..close].split_once(',')?;
    Some((a.parse().ok()?, b.parse().ok()?, &inner[close + 1..]))
}

/// `lt`: `text(start,duration)` syllables.
fn qrc_syllables(t: &str) -> Vec<(String, i32, i32)> {
    let mut out = Vec::new();
    let mut s = 0;
    let mut i = 0;
    while let Some(o) = t[i..].find('(') {
        let o = i + o;
        let parsed = t[o + 1..].find(')').and_then(|c| {
            let (a, b) = t[o + 1..o + 1 + c].split_once(',')?;
            let ok = |x: &str| !x.is_empty() && x.bytes().all(|b| b.is_ascii_digit());
            (ok(a) && ok(b)).then(|| (a.parse::<i32>().ok(), b.parse::<i32>().ok(), o + 2 + c))
        });
        match parsed {
            Some((Some(a), Some(b), end)) => {
                out.push((t[s..o].to_owned(), a, b));
                s = end;
                i = end;
            }
            _ => i = o + 1,
        }
    }
    out
}

fn assign_singer(name: &str, line: &mut QrcLine, st: &mut Singers) {
    if !st.aliases.contains_key(name) {
        let up = name.to_uppercase();
        let group = up == "合" || up == "ALL" || up == "合唱";
        let id = if group {
            "v1000".to_owned()
        } else {
            let id = format!("v{}", st.next);
            st.next += 1;
            id
        };
        st.aliases.insert(name.to_owned(), id);
    }
    line.agent = st.aliases.get(name).cloned();
}

fn retime(line: &mut QrcLine) {
    if let (Some(f), Some(l)) = (line.syllables.first(), line.syllables.last()) {
        line.time = f.1;
        line.duration = l.1 + l.2 - f.1;
    }
}

/// `jt`: Sørensen–Dice similarity of character bigrams.
fn similarity(a: &str, b: &str) -> f32 {
    let (a, b): (Vec<char>, Vec<char>) = (a.to_lowercase().chars().collect(), b.to_lowercase().chars().collect());
    if a.len() < 2 || b.len() < 2 {
        return 0.0;
    }
    let mut map: HashMap<(char, char), i32> = HashMap::new();
    for w in a.windows(2) {
        *map.entry((w[0], w[1])).or_default() += 1;
    }
    let mut hits = 0;
    for w in b.windows(2) {
        if let Some(c) = map.get_mut(&(w[0], w[1]))
            && *c > 0
        {
            *c -= 1;
            hits += 1;
        }
    }
    hits as f32 * 2.0 / (a.len() + b.len() - 2) as f32
}

/// `Bt`: keep a QRC line? Drops title/credit lines and pulls "Singer: …" prefixes into agents.
fn keep_qrc_line(t: &mut QrcLine, st: &mut Singers, early: bool, meta: Option<(&str, &str)>) -> bool {
    if t.syllables.is_empty() {
        if let Some(c) = &st.current {
            t.agent = Some(c.clone());
        }
        return true;
    }
    if early {
        if let Some((title, artist)) = meta
            && (!title.is_empty() || !artist.is_empty())
        {
            let b = |p: &str| p.to_lowercase().chars().filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || ('\u{3000}'..='\u{9fff}').contains(c) || ('\u{ac00}'..='\u{d7af}').contains(c)).collect::<String>();
            let (h, tt, r) = (b(&t.text), b(title), b(artist));
            if (!tt.is_empty() && h.contains(&tt) && !r.is_empty() && h.contains(&r))
                || (!tt.is_empty() && h.contains(&tt) && r.is_empty() && h.chars().count() < tt.chars().count() + 15)
                || (!r.is_empty() && h == r)
            {
                return false;
            }
            if similarity(&t.text, &format!("{title} {artist}")) > 0.5 {
                return false;
            }
        }
        if t.syllables.len() > 2 && t.syllables.iter().all(|s| (s.2 - t.syllables[0].2).abs() < 10) {
            return false;
        }
    }
    if let Some(c) = &st.current {
        t.agent = Some(c.clone());
    }
    let mut r = String::new();
    let mut o = 0;
    for p in &t.syllables {
        r.push_str(&p.0);
        o += 1;
        if r.contains(':') || r.contains('：') {
            break;
        }
        if r.chars().count() > 40 {
            return true;
        }
    }
    let colon = r.find([':', '：']);
    let Some(ci) = colon else { return true };
    let name = r[..ci].trim().to_owned();
    let after = &r[ci + r[ci..].chars().next().unwrap().len_utf8()..];
    if name.is_empty() {
        return true;
    }
    if after.trim().is_empty() {
        // "Singer:" on its own.
        if is_credit_label(&name) {
            return false;
        }
        if name.chars().count() <= 30 {
            t.syllables.drain(..o);
            t.text = t.text.get(r.len()..).unwrap_or_default().to_owned();
            assign_singer(&name, t, st);
            st.current = st.aliases.get(&name).cloned();
            retime(t);
        }
        return true;
    }
    if is_credit_label(&name) {
        return false;
    }
    if name.chars().count() > 20 {
        return true;
    }
    let rest = after.trim_start().to_owned();
    let u = o - 1;
    let tail = t.syllables[u].0.find([':', '：']).map(|i| t.syllables[u].0[i..].chars().skip(1).collect::<String>().trim_start().to_owned()).unwrap_or_default();
    if !tail.is_empty() {
        t.syllables[u].0 = tail;
        t.syllables.drain(..u);
    } else {
        t.syllables.drain(..o);
    }
    assign_singer(&name, t, st);
    st.current = st.aliases.get(&name).cloned();
    t.text = rest;
    retime(t);
    true
}

/// `parseQRC`: QQ Music's syllable format.
pub fn parse_qrc(text: &str, song_ms: i32, meta: Option<(&str, &str)>) -> Vec<Line> {
    if text.is_empty() {
        return vec![];
    }
    let content = qrc_content(text);
    let mut r: Vec<QrcLine> = Vec::new();
    let mut st = Singers { aliases: HashMap::new(), next: 1, current: None };
    for a in content.split('\n') {
        let i = a.trim();
        if i.is_empty() || (i.starts_with('[') && i[1..].find(':').is_some_and(|c| c > 0 && i[1..1 + c].chars().all(|ch| ch.is_ascii_alphabetic()))) {
            continue;
        }
        let Some((time, duration, rest)) = qrc_header(i) else { continue };
        let syllables = qrc_syllables(rest);
        let text = syllables.iter().map(|s| s.0.as_str()).collect::<String>();
        let mut g = QrcLine { time, duration, text, syllables, agent: None };
        let early = r.len() < 5;
        if keep_qrc_line(&mut g, &mut st, early, meta) {
            r.push(g);
        }
    }
    if r.is_empty() {
        return vec![];
    }
    let n = r.len();
    let lines = (0..n)
        .map(|i| {
            let a = &r[i];
            let mut parts: Vec<Part> = a.syllables.iter().map(|(t, s, d)| Part { start: *s, dur: *d, text: t.clone(), bg: false }).collect();
            let mut u = a.duration;
            if u == 0 && i + 1 < n {
                u = (r[i + 1].time - a.time).max(0);
            } else if u == 0 {
                u = song_ms - a.time;
            }
            if let Some(f) = parts.last_mut()
                && f.dur == 0
            {
                f.dur = if i + 1 < n { (r[i + 1].time - f.start).max(0) } else { song_ms - f.start };
            }
            Line { start: a.time, dur: u, text: a.text.clone(), parts, agent: a.agent.clone(), ..Default::default() }
        })
        .collect();
    insert_instrumentals(lines, song_ms)
}

fn qrc_songwriters(text: &str) -> Vec<String> {
    let mut n = Vec::new();
    for s in qrc_content(text).split('\n') {
        if let Some((_, _, rest)) = qrc_header(s.trim()) {
            let t: String = qrc_syllables(rest).into_iter().map(|s| s.0).collect();
            n.extend(writers_on(&t));
        }
    }
    dedup(n)
}

// ------------------------------------------------------------------- TTML

pub struct Ttml {
    pub lines: Vec<Line>,
    pub word_synced: bool,
    pub language: Option<String>,
    pub songwriters: Vec<String>,
}

/// `C` (parseTTMLTime): `1.5s` / `2m` / `1h` / `250ms`, or `h:m:s.fff`.
pub fn ttml_time(t: &str) -> i32 {
    let t = t.trim();
    if t.is_empty() {
        return 0;
    }
    for (suffix, mul) in [("ms", 1.0), ("h", 3_600_000.0), ("m", 60_000.0), ("s", 1000.0)] {
        if let Some(n) = t.strip_suffix(suffix)
            && !n.is_empty()
            && n.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        {
            return (n.parse::<f64>().unwrap_or(0.0) * mul).round() as i32;
        }
    }
    let s: Vec<f64> = t.split(':').map(|p| p.chars().filter(|c| c.is_ascii_digit() || *c == '.').collect::<String>().parse::<f64>().unwrap_or(0.0)).collect();
    let ms = match s.len() {
        1 => s[0] * 1000.0,
        2 => s[0].trunc() * 60_000.0 + s[1] * 1000.0,
        3 => s[0].trunc() * 3_600_000.0 + s[1].trunc() * 60_000.0 + s[2] * 1000.0,
        _ => 0.0,
    };
    ms.round() as i32
}

/// `At`: declare prefixes the file uses without binding (an XML parser refuses them).
fn bind_prefixes(t: &str) -> String {
    let Some(start) = t.find("<tt") else { return t.to_owned() };
    let Some(len) = t[start..].find('>') else { return t.to_owned() };
    let tag = &t[start..start + len + 1];
    let mut declared: Vec<String> = vec!["xml".into(), "xmlns".into()];
    for (i, _) in tag.match_indices("xmlns:") {
        let name: String = tag[i + 6..].chars().take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '-')).collect();
        declared.push(name);
    }
    let mut used: Vec<String> = Vec::new();
    let bytes: Vec<char> = t.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        // `<prefix:` / `</prefix:` and ` prefix:attr=`
        let at_tag = bytes[i] == '<' || (bytes[i] == '/' && i > 0 && bytes[i - 1] == '<');
        let at_attr = bytes[i].is_whitespace();
        if at_tag || at_attr {
            let mut j = i + 1;
            let mut name = String::new();
            while j < bytes.len() && (bytes[j].is_alphanumeric() || matches!(bytes[j], '_' | '.' | '-')) {
                name.push(bytes[j]);
                j += 1;
            }
            if !name.is_empty() && name.starts_with(|c: char| c.is_ascii_alphabetic()) && j < bytes.len() && bytes[j] == ':' {
                let ok = if at_attr {
                    let mut k = j + 1;
                    while k < bytes.len() && (bytes[k].is_alphanumeric() || matches!(bytes[k], '_' | '.' | '-')) {
                        k += 1;
                    }
                    while k < bytes.len() && bytes[k].is_whitespace() {
                        k += 1;
                    }
                    k < bytes.len() && bytes[k] == '='
                } else {
                    true
                };
                if ok && !used.contains(&name) {
                    used.push(name);
                }
            }
        }
        i += 1;
    }
    let missing: Vec<&String> = used.iter().filter(|u| !declared.contains(u)).collect();
    if missing.is_empty() {
        return t.to_owned();
    }
    let extra: String = missing.iter().map(|m| format!(" xmlns:{m}=\"urn:braccato:unbound:{m}\"")).collect();
    let new_tag = format!("{}{}>", &tag[..tag.len() - 1], extra);
    t.replacen(tag, &new_tag, 1)
}

fn attr<'a>(n: roxmltree::Node<'a, '_>, name: &str) -> Option<&'a str> {
    n.attributes().find(|a| a.name() == name).map(|a| a.value())
}

/// `D`: text of a node and its spans.
fn span_text(n: roxmltree::Node) -> String {
    let mut s = String::new();
    for c in n.children() {
        if let Some(t) = c.text().filter(|_| c.is_text()) {
            s.push_str(t);
        } else if c.is_element() && c.tag_name().name() == "span" {
            s.push_str(&span_text(c));
        }
    }
    s
}

/// `U`: a paragraph's (or transliteration's) children as parts and text.
fn ttml_parts(node: roxmltree::Node, line_start: i32) -> (Vec<Part>, String, bool) {
    let mut s = String::new();
    let mut e: Vec<Part> = Vec::new();
    let mut timed = false;
    for u in node.children() {
        let bg = u.is_element() && attr(u, "role") == Some("x-bg");
        let group: Vec<roxmltree::Node> = if bg { u.children().collect() } else { vec![u] };
        for m in group {
            if m.is_text() {
                let t = m.text().unwrap_or_default();
                if t.is_empty() {
                    continue;
                }
                s.push_str(t);
                let start = e.last().map_or(line_start, |p| p.start + p.dur);
                e.push(Part { start, dur: 0, text: t.to_owned(), bg });
            } else if m.is_element() && m.tag_name().name() == "span" {
                let p = span_text(m);
                if p.is_empty() {
                    continue;
                }
                let b = ttml_time(attr(m, "begin").unwrap_or_default());
                let h = ttml_time(attr(m, "end").unwrap_or_default());
                e.push(Part { start: b, dur: h - b, text: p.clone(), bg });
                s.push_str(&p);
                timed = true;
            }
        }
    }
    if !timed {
        e.clear();
    }
    (e, s, timed)
}

/// `parseTTMLContent`: Apple-style TTML with agents, background vocals, and the
/// translations and transliterations in its iTunes metadata.
pub fn parse_ttml(text: &str, song_ms: i32) -> Ttml {
    let empty = |language| Ttml { lines: vec![], word_synced: false, language, songwriters: vec![] };
    let src = bind_prefixes(&text.replace("\\\"", "\""));
    let Ok(doc) = roxmltree::Document::parse_with_options(&src, roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() }) else {
        return empty(None);
    };
    let tt = doc.root_element();
    if tt.tag_name().name() != "tt" {
        return empty(None);
    }
    let body = tt.children().find(|n| n.tag_name().name() == "body");
    let language = attr(tt, "lang").or_else(|| body.and_then(|b| attr(b, "lang"))).map(str::to_owned);
    let Some(body) = body else { return empty(language) };
    let head_meta = tt.children().find(|n| n.tag_name().name() == "head").and_then(|h| h.children().find(|n| n.tag_name().name() == "metadata"));
    // Agents: people and characters are v1, v2, …; anything else sings for everyone.
    let mut agents: HashMap<String, String> = HashMap::new();
    if let Some(m) = head_meta {
        let mut k = 0;
        for a in m.children().filter(|n| n.tag_name().name() == "agent") {
            let Some(id) = attr(a, "id") else { continue };
            let kind = attr(a, "type").unwrap_or_default();
            if kind == "person" || kind == "character" {
                k += 1;
                agents.insert(id.to_owned(), format!("v{k}"));
            } else {
                agents.insert(id.to_owned(), "v1000".to_owned());
            }
        }
    }
    let ps: Vec<roxmltree::Node> = body.children().filter(|n| n.tag_name().name() == "div").flat_map(|d| d.children().filter(|n| n.tag_name().name() == "p")).collect();
    if ps.is_empty() {
        return empty(language);
    }
    let mut lines: Vec<Line> = Vec::new();
    let mut keys: HashMap<String, Vec<usize>> = HashMap::new();
    let mut word_synced = false;
    for p in ps {
        let Some(begin) = attr(p, "begin") else { continue };
        let start = ttml_time(begin);
        let end = ttml_time(attr(p, "end").unwrap_or_default());
        let (parts, text, timed) = ttml_parts(p, start);
        word_synced |= timed;
        let agent = attr(p, "agent").map(|a| agents.get(a).cloned().unwrap_or_else(|| a.to_owned()));
        if let Some(k) = attr(p, "key") {
            keys.entry(k.to_owned()).or_default().push(lines.len());
        }
        lines.push(Line { start, dur: end - start, text, parts, agent, ..Default::default() });
    }
    // Translations and transliterations, matched to lines by key.
    let meta_root = head_meta.and_then(|m| m.descendants().find(|n| n.tag_name().name() == "iTunesMetadata")).or(head_meta);
    if let Some(m) = meta_root {
        if let Some(ts) = m.descendants().find(|n| n.tag_name().name() == "translations") {
            let default_lang = attr(ts, "lang");
            for t in ts.children().filter(|n| n.tag_name().name() == "translation") {
                let lang = attr(t, "lang").or(default_lang);
                for item in t.children().filter(|n| n.is_element()) {
                    let Some(key) = attr(item, "for").or_else(|| attr(t, "for")) else { continue };
                    let txt = span_text(item);
                    let (Some(lang), false) = (lang, txt.is_empty()) else { continue };
                    for &i in keys.get(key).map(Vec::as_slice).unwrap_or_default() {
                        lines[i].translations.push((lang.to_owned(), txt.clone()));
                    }
                }
            }
        }
        if let Some(ts) = m.descendants().find(|n| n.tag_name().name() == "transliterations") {
            for t in ts.children().filter(|n| n.tag_name().name() == "transliteration") {
                for item in t.children().filter(|n| n.is_element()) {
                    let Some(key) = attr(item, "for") else { continue };
                    for &i in keys.get(key).map(Vec::as_slice).unwrap_or_default() {
                        let (parts, text, _) = ttml_parts(item, lines[i].start);
                        if text.is_empty() {
                            continue;
                        }
                        lines[i].romanization = Some(text);
                        lines[i].timed_roman = parts;
                    }
                }
            }
        }
    }
    let song = attr(body, "dur").map(ttml_time).unwrap_or(song_ms);
    let songwriters = {
        let mut n = Vec::new();
        if let Some(m) = head_meta {
            if let Some(sw) = m.descendants().find(|n| n.tag_name().name() == "songwriters") {
                for w in sw.children().filter(|n| n.tag_name().name() == "songwriter") {
                    n.push(span_text(w));
                }
            }
            for meta in m.children().filter(|n| n.tag_name().name() == "meta") {
                if matches!(attr(meta, "key"), Some("songwriter" | "songwriters"))
                    && let Some(v) = attr(meta, "value")
                {
                    n.push(v.to_owned());
                }
            }
        }
        dedup(n)
    };
    Ttml { lines: insert_instrumentals(lines, song), word_synced, language, songwriters }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enhanced_lrc_parts() {
        let l = parse_lrc("[00:31.69] <00:31.69> Like <00:35.63>   <00:35.64> the <00:35.69>\n[00:40.00] Next", 60_000);
        assert_eq!(l[0].text, "Like the");
        assert_eq!(l[0].parts[0].start, 31690);
        // Spaced files: word segments are trimmed, spaces get their own parts.
        assert_eq!(l[0].parts[0].text, "Like");
        assert_eq!(l[0].parts[1].text, " ");
        assert_eq!(l[0].parts[0].dur, 3940);
        assert_eq!(l[1].start, 40000);
        // Last line runs to the end of the song.
        assert_eq!(l[1].dur, 20000);
    }

    #[test]
    fn lrc_skips_credits_and_reads_offset() {
        // The extension reads `offset` in seconds.
        let l = parse_lrc("[offset:+0.5]\n[00:01.00]作词 : 野田洋次郎\n[00:05.00]Hello\n[00:07.00]World", 10_000);
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].start, 4500);
        assert_eq!(lrc_songwriters("[00:01.00]作词 : 野田洋次郎\n"), vec!["野田洋次郎"]);
    }

    #[test]
    fn lrc_fixers_move_space_time_to_word() {
        let mut l = parse_lrc("[00:01.00]<00:01.00>a<00:01.50> <00:02.00>b<00:02.50>\n[00:03.00]x", 5000);
        lrc_fixers(&mut l);
        assert_eq!(l[0].parts[0].dur, 1000);
    }

    #[test]
    fn qrc_drops_title_and_reads_syllables() {
        let xml = "<Lyric_1 LyricContent=\"[ti:x]\n[0,6350]Song - Artist(0,198)\n[1000,2000]Get(1000,500) (1500,10)Lucky(1510,490)\n\"/>";
        let l = parse_qrc(xml, 3000, Some(("Song", "Artist")));
        let sung: Vec<&Line> = l.iter().filter(|l| !l.instrumental).collect();
        assert_eq!(sung.len(), 1);
        assert_eq!(sung[0].text, "Get Lucky");
        assert_eq!(sung[0].parts[2].start, 1510);
    }

    #[test]
    fn ttml_parts_background_agents_and_extras() {
        let xml = r#"<tt xmlns="http://www.w3.org/ns/ttml" xmlns:itunes="http://music.apple.com/lyric-ttml-internal" xmlns:ttm="http://www.w3.org/ns/ttml#metadata" xml:lang="ja"><head><metadata><ttm:agent type="person" xml:id="v1"/><ttm:agent type="person" xml:id="v2"/><iTunesMetadata xmlns="http://music.apple.com/lyric-ttml-internal"><translations><translation xml:lang="en"><text for="L1">Hello world</text></translation></translations><songwriters><songwriter>野田洋次郎</songwriter></songwriters><transliterations><transliteration xml:lang="ja-Latn"><text for="L1"><span begin="1.0" end="2.0">kon</span> <span begin="2.0" end="3.0">nichiwa</span></text></transliteration></transliterations></iTunesMetadata></metadata></head><body dur="20.0"><div>
<p begin="1.0" end="3.0" itunes:key="L1" ttm:agent="v1"><span begin="1.0" end="1.5">こん</span><span begin="1.5" end="2.0">に</span> <span begin="2.0" end="3.0">ちは</span><span ttm:role="x-bg"><span begin="2.5" end="3.5">(yeah)</span></span></p>
<p begin="10.0" end="11.0" itunes:key="L2" ttm:agent="v2">Line two</p></div></body></tt>"#;
        let t = parse_ttml(xml, 0);
        assert!(t.word_synced);
        assert_eq!(t.language.as_deref(), Some("ja"));
        assert_eq!(t.songwriters, vec!["野田洋次郎"]);
        let l = &t.lines[0];
        assert_eq!(l.text, "こんに ちは(yeah)");
        assert_eq!(l.parts.len(), 5);
        assert_eq!(l.parts[2], Part { start: 2000, dur: 0, text: " ".into(), bg: false });
        assert!(l.parts[4].bg && l.parts[4].start == 2500);
        assert_eq!(l.agent.as_deref(), Some("v1"));
        assert_eq!(l.translations, vec![("en".to_owned(), "Hello world".to_owned())]);
        assert_eq!(l.romanization.as_deref(), Some("kon nichiwa"));
        assert_eq!(l.timed_roman.len(), 3);
        // A gap of more than five seconds becomes an instrumental line.
        assert!(t.lines[1].instrumental && t.lines[1].start == 3000);
        assert_eq!(t.lines[2].agent.as_deref(), Some("v2"));
        assert!(t.lines[2].parts.is_empty());
        assert!(t.lines.last().unwrap().instrumental);
    }

    #[test]
    fn ttml_times() {
        assert_eq!(ttml_time("1:02.345"), 62345);
        assert_eq!(ttml_time("12.5s"), 12500);
        assert_eq!(ttml_time("250ms"), 250);
        assert_eq!(ttml_time("01:00:01.000"), 3_601_000);
    }

    #[test]
    fn real_stream_parses_every_provider() {
        let Ok(body) = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/research/nandemonaiya.sse")) else { return };
        let q = Query { video_id: "n89SKAymNfA", song: "Nandemonaiya - movie ver.", artist: "RADWIMPS", album: None, duration: Some(343) };
        let got = from_stream(&body, &q);
        let keys: Vec<&str> = got.iter().map(|l| l.provider.key()).collect();
        assert!(keys.contains(&"bLyrics-richsynced"), "{keys:?}");
        let bl = got.iter().find(|l| l.provider == Provider::BLyricsRich).unwrap();
        assert_eq!(bl.sync, Sync::Word);
        assert!(bl.lines.iter().any(|l| l.timed_roman.len() > 3));
        assert_eq!(bl.songwriters, vec!["野田洋次郎"]);
    }
}
