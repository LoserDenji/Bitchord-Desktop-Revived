//! Parallel, seek-aware HTTP audio buffer.
//!
//! The file is split into fixed blocks fetched by a few concurrent range requests.
//! Workers always claim the first missing block at or after the one a reader is
//! waiting on, so playback start and seeks are served first and the rest fills in
//! around them. Reads of a missing block block until it arrives (or the download fails).
//!
//! Several readers may share one buffer (the audio engine opens a second, seekable
//! decoder when the user seeks); the download stops once the last reader is dropped.

use std::future::Future;
use std::io::{self, Read, Seek, SeekFrom};
use std::pin::Pin;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use bytes::Bytes;

const BLOCK: u64 = 256 * 1024;
/// The first block is smaller: a decoder opens after a few hundred bytes, so playback
/// waits on this one alone, and it still holds seconds of audio while the next arrive.
const FIRST: u64 = 64 * 1024;
const WORKERS: usize = 4;
/// Tries per block before the download gives up, waiting a little longer each time
/// (about half a minute in all): long enough to ride out a dropped connection.
const RETRIES: u32 = 6;
/// A response that delivers nothing for this long is dead; a slow one that keeps
/// delivering is fine, however long the block takes.
const STALL: Duration = Duration::from_secs(20);
/// googlevideo occasionally starts rejecting a URL partway through (HTTP 403);
/// a freshly resolved URL for the same video resumes the download.
const MAX_REFRESHES: u32 = 6;
/// A prefetch only fetches this much (~10 s of audio at 256 kbps, two HLS segments)
/// until playback actually opens it: enough to start at once, without holding a whole
/// extra song in memory.
const HEAD_BLOCKS: usize = 2;
/// Files larger than this (about 20 minutes at 256 kbps: long mixes, whole albums in
/// one video) aren't kept whole. Each reader keeps a window around where it reads,
/// `AHEAD` blocks forward (about 5 minutes at 256 kbps) and `BEHIND` back; the rest is dropped and
/// fetched again if a seek goes back to it.
const WINDOWED_FROM: u64 = 40 * 1024 * 1024;
const AHEAD: usize = 40;
const BEHIND: usize = 8;

/// Block `idx`'s first byte in a file.
fn block_start(idx: usize) -> u64 {
    if idx == 0 { 0 } else { FIRST + (idx as u64 - 1) * BLOCK }
}

/// The block holding byte `pos` of a file.
fn block_of(pos: u64) -> usize {
    if pos < FIRST { 0 } else { 1 + ((pos - FIRST) / BLOCK) as usize }
}

/// How many blocks a file of `total` bytes takes.
fn block_count(total: u64) -> usize {
    if total == 0 { 0 } else { block_of(total - 1) + 1 }
}

/// What a download fetches.
pub enum Media {
    /// One file, in ranges: `FIRST` bytes, then `BLOCK`s.
    File(String),
    /// A podcast feed's file on its own server: the same ranges, asked for in a
    /// `Range` header (googlevideo's `&range=` means nothing there).
    Feed(String),
    /// An HLS playlist's segments, one block each.
    Segments(Vec<String>),
}

/// Resolves fresh URLs for the same video and format.
pub type Refresh = Arc<dyn Fn() -> Pin<Box<dyn Future<Output = anyhow::Result<Media>> + Send>> + Send + Sync>;

/// The container and codec a stream carries.
#[derive(Clone, Debug, PartialEq)]
pub enum Format {
    /// AAC in MP4: YouTube's streams, and saved songs.
    Aac,
    /// Opus in WebM: YouTube without Premium, and addons that serve it.
    Opus,
    /// A file rodio opens with a hint and MIME type: FLAC, ALAC, WAV, MP3, Vorbis.
    File { hint: &'static str, mime: Option<String> },
    /// Interleaved signed 16-bit little-endian PCM (decoded in-process, e.g. Atmos).
    Pcm { rate: u32, channels: u16 },
}

impl Format {
    pub fn from_opus(opus: bool) -> Self {
        if opus { Self::Opus } else { Self::Aac }
    }
}

/// How a source takes byte ranges: YouTube's URLs carry them in the query string,
/// addon servers expect the standard `Range` header.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ranging {
    Query,
    Header,
}

struct Inner {
    blocks: Vec<Option<Bytes>>,
    in_flight: Vec<bool>,
    /// Block a reader most recently needed; workers prioritize from here.
    want: usize,
    error: Option<String>,
    /// The download stopped on HTTP 403s that fresh URLs didn't get past.
    refused: bool,
    readers: usize,
    /// Set once a reader existed and all were dropped.
    cancelled: bool,
    /// Blocks the workers may fetch; lifted to all of them when a reader opens.
    limit: usize,
    /// Each open reader's id and the block it last read (windowed files keep only
    /// what's near these).
    wants: Vec<(u64, usize)>,
    windowed: bool,
    /// Blocks fetched at least once, and how many: the download counts as finished
    /// once each was, even if a windowed file has dropped some since.
    ever: Vec<bool>,
    fetched: usize,
}

impl Inner {
    /// Whether block `i` belongs in memory now.
    fn in_window(&self, i: usize) -> bool {
        !self.windowed || self.wants.is_empty() || self.wants.iter().any(|&(_, w)| i + BEHIND >= w && i < w + AHEAD)
    }
}

enum Claim {
    Block(usize),
    /// Everything allowed so far is fetched or in flight; wait for the limit to lift.
    Wait,
    Done,
}

/// Stream buffers alive, for the `YTM_MEM` dev log.
pub static LIVE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

impl Drop for Shared {
    fn drop(&mut self) {
        LIVE.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    }
}

pub struct Shared {
    inner: Mutex<Inner>,
    ready: Condvar,
    pub total: u64,
    /// Track length reported by the stream metadata (0 if unknown).
    pub duration_ms: u64,
    /// YouTube's loudness for this stream, in dB above its -14 LUFS target.
    pub loudness_db: Option<f32>,
    pub tempo: Arc<std::sync::OnceLock<Option<f32>>>,
    /// The container and codec this buffer holds.
    pub format: Format,
    /// What the player can say about this stream, when its source reported it (addons).
    pub info: Option<crate::hires::Info>,
    /// A podcast feed's file (`Media::Feed`): MP3 mostly, which seeks by estimate,
    /// since an exact seek in MP3 reads every frame before the spot.
    pub feed: bool,
    /// HLS: each segment's start (s). Its blocks are the segments.
    pub segments: Option<Arc<[f64]>>,
    /// Flips to true once the download task has stopped (complete, failed or cancelled).
    done: tokio::sync::watch::Sender<bool>,
    /// Wakes waiting workers: `limit` lifted, a reader moved or left, or cancelled.
    changed: tokio::sync::Notify,
}

impl Shared {
    /// A buffer for `total` bytes. `head_only` (a prefetch) fetches just the start until
    /// a reader opens it.
    pub fn new(total: u64, duration_ms: u64, loudness_db: Option<f32>, format: Format, info: Option<crate::hires::Info>, head_only: bool) -> Arc<Self> {
        Self::build(block_count(total), total, total > WINDOWED_FROM, duration_ms, loudness_db, format, info, false, None, head_only)
    }

    /// A buffer for a podcast feed's file of `total` bytes.
    pub fn feed(total: u64, duration_ms: u64, head_only: bool) -> Arc<Self> {
        Self::build(block_count(total), total, total > WINDOWED_FROM, duration_ms, None, Format::File { hint: "mp3", mime: None }, None, true, None, head_only)
    }

    /// A buffer for an HLS stream's segments, starting at `starts` (s).
    pub fn segmented(starts: Vec<f64>, duration_ms: u64, loudness_db: Option<f32>, head_only: bool) -> Arc<Self> {
        // About as many segments (~7 s) as a windowed file has blocks (~8 s at 256 kbps).
        let windowed = starts.len() as u64 > WINDOWED_FROM / BLOCK;
        Self::build(starts.len(), 0, windowed, duration_ms, loudness_db, Format::Aac, None, false, Some(starts.into()), head_only)
    }

    #[allow(clippy::too_many_arguments)]
    fn build(n: usize, total: u64, windowed: bool, duration_ms: u64, loudness_db: Option<f32>, format: Format, info: Option<crate::hires::Info>, feed: bool, segments: Option<Arc<[f64]>>, head_only: bool) -> Arc<Self> {
        let limit = if head_only { HEAD_BLOCKS.min(n) } else { n };
        LIVE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Arc::new(Self {
            inner: Mutex::new(Inner {
                blocks: vec![None; n],
                in_flight: vec![false; n],
                want: 0,
                error: None,
                refused: false,
                readers: 0,
                cancelled: false,
                limit,
                wants: Vec::new(),
                windowed,
                ever: vec![false; n],
                fetched: 0,
            }),
            ready: Condvar::new(),
            total,
            duration_ms,
            loudness_db,
            tempo: Arc::new(std::sync::OnceLock::new()),
            format,
            info,
            feed,
            segments,
            done: tokio::sync::watch::Sender::new(false),
            changed: tokio::sync::Notify::new(),
        })
    }

    /// A buffer that already holds the whole file (a downloaded song).
    pub fn from_bytes(data: Vec<u8>, duration_ms: u64, loudness_db: Option<f32>) -> Arc<Self> {
        // Saved songs are AAC (downloads take that stream).
        Self::from_bytes_with_format(data, duration_ms, loudness_db, Format::Aac, None)
    }

    /// A buffer that's already whole, in whatever container it was decoded to.
    pub fn from_bytes_with_format(data: Vec<u8>, duration_ms: u64, loudness_db: Option<f32>, format: Format, info: Option<crate::hires::Info>) -> Arc<Self> {
        let total = data.len() as u64;
        let data = Bytes::from(data);
        let shared = Self::new(total, duration_ms, loudness_db, format, info, false);
        let n = shared.inner.lock().unwrap().blocks.len();
        let blocks: Vec<Option<Bytes>> = (0..n).map(|i| Some(data.slice(block_start(i) as usize..block_start(i + 1).min(total) as usize))).collect();
        {
            let mut inner = shared.inner.lock().unwrap();
            inner.blocks = blocks;
            inner.windowed = false;
            inner.ever = vec![true; n];
            inner.fetched = n;
        }
        shared.done.send_replace(true);
        shared
    }

    /// The whole file, once every block has arrived (a download nobody's reading).
    pub fn bytes(&self) -> Option<Vec<u8>> {
        let inner = self.inner.lock().unwrap();
        let mut out = Vec::with_capacity(self.total as usize);
        for b in &inner.blocks {
            out.extend_from_slice(b.as_ref()?);
        }
        Some(out)
    }

    /// Every block is in memory: readers never wait and the buffer can be replayed.
    pub fn is_complete(&self) -> bool {
        self.inner.lock().unwrap().blocks.iter().all(Option::is_some)
    }

    /// Too big to keep whole: blocks behind the reader get dropped as it goes, so a
    /// seek to a spot whose block is gone would wait for a re-download.
    pub fn windowed(&self) -> bool {
        self.inner.lock().unwrap().windowed
    }

    /// Wait until the download stops, however it ends.
    pub async fn finished(&self) {
        let _ = self.done.subscribe().wait_for(|d| *d).await;
    }

    /// Why the download stopped early, if it failed.
    pub fn error(&self) -> Option<String> {
        self.inner.lock().unwrap().error.clone()
    }

    /// The download failed on HTTP 403s that fresh URLs didn't get past.
    pub fn refused(&self) -> bool {
        self.inner.lock().unwrap().refused
    }

    /// A block a reader may need now is still to come (a windowed file counts only its
    /// window): downloads in the background make way for it.
    pub fn filling(&self) -> bool {
        let inner = self.inner.lock().unwrap();
        !inner.cancelled && inner.error.is_none() && (0..inner.limit).any(|i| inner.blocks[i].is_none() && inner.in_window(i))
    }

    /// Stop downloading (e.g. a prefetch that was superseded).
    pub fn cancel(&self) {
        self.inner.lock().unwrap().cancelled = true;
        self.ready.notify_all();
        self.changed.notify_waiters();
    }

    fn claim(&self) -> Claim {
        let mut inner = self.inner.lock().unwrap();
        if inner.cancelled || inner.error.is_some() {
            return Claim::Done;
        }
        let n = inner.blocks.len();
        let start = inner.want.min(n.saturating_sub(1));
        let mut missing = false;
        for i in (start..n).chain(0..start) {
            if inner.blocks[i].is_some() || inner.in_flight[i] {
                continue;
            }
            missing = true;
            if i < inner.limit && inner.in_window(i) {
                inner.in_flight[i] = true;
                return Claim::Block(i);
            }
        }
        // A windowed file may need dropped blocks again while it's being read.
        if missing || (inner.windowed && inner.readers > 0) { Claim::Wait } else { Claim::Done }
    }

    fn put(&self, idx: usize, data: Bytes) {
        let mut inner = self.inner.lock().unwrap();
        inner.blocks[idx] = Some(data);
        inner.in_flight[idx] = false;
        if !std::mem::replace(&mut inner.ever[idx], true) {
            inner.fetched += 1;
            if inner.fetched == inner.blocks.len() {
                self.done.send_replace(true);
            }
        }
        if inner.windowed && !inner.wants.is_empty() {
            for i in 0..inner.blocks.len() {
                if inner.blocks[i].is_some() && !inner.in_window(i) {
                    inner.blocks[i] = None;
                }
            }
        }
        drop(inner);
        // Woken readers re-lock at once; holding the guard through the wake would hand
        // them the lock to take all over again — and one of them is the audio callback.
        self.ready.notify_all();
    }

    fn fail(&self, err: String, refused: bool) {
        log::error!("download failed: {err}");
        FAIL_FROM.store(usize::MAX, std::sync::atomic::Ordering::Relaxed);
        let mut inner = self.inner.lock().unwrap();
        inner.error = Some(err);
        inner.refused = refused;
        drop(inner);
        self.ready.notify_all();
    }

    fn range(&self, idx: usize) -> (u64, u64) {
        (block_start(idx), block_start(idx + 1).min(self.total) - 1)
    }
}

/// The current URLs plus how many times they have been replaced.
struct UrlSlot {
    url: tokio::sync::Mutex<(Media, u32)>,
    refresh: Option<Refresh>,
    /// Segments the buffer has: fresh ones must be the same.
    segments: Option<usize>,
}

impl UrlSlot {
    /// Replace the URL if `seen` is still current; returns false when out of refreshes.
    async fn renew(&self, seen: u32) -> bool {
        let mut slot = self.url.lock().await;
        if slot.1 != seen {
            return true; // another worker already refreshed it
        }
        let Some(refresh) = self.refresh.as_ref() else { return false };
        if slot.1 >= MAX_REFRESHES {
            log::warn!("stream URL still refused after {MAX_REFRESHES} refreshes");
            return false;
        }
        match refresh().await {
            // The buffer is laid out for the stream it started with.
            Ok(Media::Segments(urls)) if Some(urls.len()) != self.segments => {
                log::warn!("stream URL refresh: {} segments for a buffer of {:?}", urls.len(), self.segments);
                false
            }
            Ok(Media::File(_) | Media::Feed(_)) if self.segments.is_some() => {
                log::warn!("stream URL refresh: a file for a segmented buffer");
                false
            }
            Ok(media) => {
                log::info!("stream URL refreshed after 403");
                *slot = (media, slot.1 + 1);
                true
            }
            Err(e) => {
                log::warn!("stream URL refresh failed: {e:#}");
                false
            }
        }
    }
}

/// Download into `shared` with parallel range requests; returns when complete,
/// failed, or every reader was dropped.
pub async fn download(http: reqwest::Client, media: Media, ua: String, shared: Arc<Shared>, refresh: Option<Refresh>, ranging: Ranging) {
    download_behind(http, media, ua, shared, refresh, ranging, None).await;
}

/// The buffer a background download makes way for (the playing song's), if any.
pub type Ahead = Arc<dyn Fn() -> Option<Arc<Shared>> + Send + Sync>;

/// `download`, but before each block it waits while `ahead`'s buffer still has blocks
/// to come that a reader may need: the playing song gets the bandwidth first.
pub async fn download_behind(http: reqwest::Client, media: Media, ua: String, shared: Arc<Shared>, refresh: Option<Refresh>, ranging: Ranging, ahead: Option<Ahead>) {
    // Addon servers may ignore ranges and answer 200 with the whole file: then one
    // sequential download fills the buffer, rather than every worker pulling the file.
    if let Media::File(url) = &media {
        if ranging == Ranging::Header && !ranges_supported(&http, url, &ua).await {
            download_whole(&http, url, &ua, &shared).await;
            return;
        }
    }
    let segments = shared.segments.as_ref().map(|s| s.len());
    let slot = Arc::new(UrlSlot { url: tokio::sync::Mutex::new((media, 0)), refresh, segments });
    let mut set = tokio::task::JoinSet::new();
    for _ in 0..WORKERS {
        let (http, ua, shared, slot, ahead) = (http.clone(), ua.clone(), shared.clone(), slot.clone(), ahead.clone());
        set.spawn(async move {
            loop {
                if let Some(ahead) = &ahead {
                    while ahead().is_some_and(|s| !Arc::ptr_eq(&s, &shared) && s.filling()) {
                        tokio::time::sleep(Duration::from_millis(250)).await;
                    }
                }
                // Registered before claiming, so a change in between isn't missed.
                let changed = shared.changed.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                let idx = match shared.claim() {
                    Claim::Block(idx) => idx,
                    Claim::Wait => {
                        changed.await;
                        continue;
                    }
                    Claim::Done => break,
                };
                let (a, b) = if segments.is_none() { shared.range(idx) } else { (0, 0) };
                let what = if segments.is_some() { format!("segment {idx}") } else { format!("range {a}-{b}") };
                let mut attempt = 0;
                loop {
                    let (url, expect, version) = {
                        let slot = slot.url.lock().await;
                        match &slot.0 {
                            // The range rides the URL for YouTube's players (Query); addon
                            // servers take it as a header (Header) and keep a clean URL.
                            Media::File(url) => (if ranging == Ranging::Query { format!("{url}&range={a}-{b}") } else { url.clone() }, Some(b - a + 1), slot.1),
                            // A podcast feed's file: its own server, ranges as a header.
                            Media::Feed(url) => (url.clone(), Some(b - a + 1), slot.1),
                            Media::Segments(urls) => (urls[idx].clone(), None, slot.1),
                        }
                    };
                    // A refreshed URL is tied to the address that resolved it, which is the
                    // current shared client's (a proxy may have switched exits since).
                    let client = if version > 0 { crate::proxy::http() } else { http.clone() };
                    let res = if idx >= FAIL_FROM.load(std::sync::atomic::Ordering::Relaxed) {
                        Err(anyhow::anyhow!("simulated network failure"))
                    } else if segments.is_none() && idx >= REFUSE_FROM.load(std::sync::atomic::Ordering::Relaxed) {
                        Err(anyhow::Error::new(SimulatedRefusal))
                    } else {
                        match ranging {
                            Ranging::Query if segments.is_none() => fetch(&client, &url, &ua, expect).await,
                            Ranging::Header if segments.is_none() => fetch_range(&client, &url, &ua, a, b, shared.total).await,
                            _ => fetch(&client, &url, &ua, None).await,
                        }
                    };
                    match res {
                        Ok(data) => {
                            shared.put(idx, data);
                            break;
                        }
                        Err(e) if is_forbidden(&e) && slot.renew(version).await => {}
                        Err(e) if attempt < RETRIES => {
                            attempt += 1;
                            let delay_ms = 400 * (attempt * attempt) as u64;
                            if is_forbidden(&e) {
                                log::warn!("{what}: HTTP 403 Forbidden; retrying with backoff ({delay_ms} ms, attempt {attempt}/{RETRIES})");
                                tokio::time::sleep(Duration::from_millis(delay_ms.max(800))).await;
                                let _ = slot.renew(version).await;
                            } else {
                                log::warn!("{what}: {e:#} (retry {attempt}/{RETRIES})");
                                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                            }
                        }
                        Err(e) => {
                            // How many fresh URLs were tried tells a URL that keeps being
                            // refused (the player's URLs, the address) from a one-off.
                            let refreshed = slot.url.lock().await.1;
                            let refused = is_forbidden(&e);
                            let tail = if refused { format!(" after {refreshed} URL refresh(es)") } else { String::new() };
                            shared.fail(format!("{what}: {e:#}{tail}"), refused);
                            return;
                        }
                    }
                }
            }
        });
    }
    while set.join_next().await.is_some() {}
    shared.done.send_replace(true);
}

fn is_forbidden(e: &anyhow::Error) -> bool {
    e.is::<SimulatedRefusal>()
        || e.downcast_ref::<reqwest::Error>()
            .and_then(reqwest::Error::status)
            .is_some_and(|s| s == reqwest::StatusCode::FORBIDDEN)
}

/// `YTM_STREAM_REFUSE`'s answer: what googlevideo says to a range it won't serve.
#[derive(Debug)]
struct SimulatedRefusal;

impl std::fmt::Display for SimulatedRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HTTP status client error (403 Forbidden) (simulated)")
    }
}

impl std::error::Error for SimulatedRefusal {}

/// Dev hook: YTM_STREAM_FAIL=<block> makes the first download fail from that block on,
/// as if the connection died mid-song; later downloads work.
static FAIL_FROM: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(usize::MAX);

/// Dev hook: YTM_STREAM_REFUSE=<block> answers every file stream's ranges from that
/// block on with HTTP 403, fresh URLs too, as YouTube does past 1 MiB (in block 4) where
/// it wants a PO token for the visionOS player's URLs. HLS segments get through.
static REFUSE_FROM: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(usize::MAX);

pub fn init_dev_hooks() {
    if let Some(b) = std::env::var("YTM_STREAM_FAIL").ok().and_then(|v| v.parse().ok()) {
        FAIL_FROM.store(b, std::sync::atomic::Ordering::Relaxed);
    }
    if let Some(b) = std::env::var("YTM_STREAM_REFUSE").ok().and_then(|v| v.parse().ok()) {
        REFUSE_FROM.store(b, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Where a podcast feed's file ends up after its redirects (trackers, mostly), and its
/// size: asked with a one-byte range, which also shows the server serves ranges.
pub async fn probe_feed(http: &reqwest::Client, url: &str, ua: &str) -> anyhow::Result<(String, u64)> {
    let resp = tokio::time::timeout(STALL, http.get(url).header("User-Agent", ua).header("Range", "bytes=0-0").header("Accept-Encoding", "identity").send())
        .await
        .map_err(|_| anyhow::anyhow!("the podcast's server didn't answer"))?
        .and_then(reqwest::Response::error_for_status)
        .map_err(reqwest::Error::without_url)?;
    anyhow::ensure!(resp.status() == reqwest::StatusCode::PARTIAL_CONTENT, "the podcast's server doesn't serve ranges ({})", resp.status());
    let total = resp
        .headers()
        .get(reqwest::header::CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.rsplit_once('/'))
        .and_then(|(_, t)| t.parse().ok())
        .ok_or_else(|| anyhow::anyhow!("the podcast's server didn't give the file's size"))?;
    Ok((resp.url().to_string(), total))
}

/// `expect`: the exact length a range must come back with (a segment's isn't known).
async fn fetch(http: &reqwest::Client, url: &str, ua: &str, expect: Option<u64>) -> anyhow::Result<Bytes> {
    let stalled = || anyhow::anyhow!("no data for {} s", STALL.as_secs());
    // Errors leave the URL out: it carries the client's IP address and signatures (logs).
    let mut resp = tokio::time::timeout(STALL, http.get(url).header("User-Agent", ua).send())
        .await
        .map_err(|_| stalled())?
        .and_then(reqwest::Response::error_for_status)
        .map_err(reqwest::Error::without_url)?;
    let mut buf = bytes::BytesMut::with_capacity(expect.or(resp.content_length()).unwrap_or(0) as usize);
    while let Some(chunk) = tokio::time::timeout(STALL, resp.chunk()).await.map_err(|_| stalled())?.map_err(reqwest::Error::without_url)? {
        buf.extend_from_slice(&chunk);
    }
    if let Some(expect) = expect {
        anyhow::ensure!(buf.len() as u64 == expect, "short read: {} of {expect} bytes", buf.len());
    }
    Ok(buf.freeze())
}

/// A block from a server that takes standard `Range` headers. A 200 (the server ignored
/// the range) still yields this block out of the whole body.
async fn fetch_range(http: &reqwest::Client, url: &str, ua: &str, a: u64, b: u64, total: u64) -> anyhow::Result<Bytes> {
    let stalled = || anyhow::anyhow!("no data for {} s", STALL.as_secs());
    let mut resp = tokio::time::timeout(
        STALL,
        http.get(url)
            .header("User-Agent", ua)
            .header("Range", format!("bytes={a}-{b}"))
            .header("Accept-Encoding", "identity")
            .send(),
    )
    .await
    .map_err(|_| stalled())?
    .and_then(reqwest::Response::error_for_status)
    .map_err(reqwest::Error::without_url)?;
    let partial = resp.status() == reqwest::StatusCode::PARTIAL_CONTENT;
    let expect = if partial { b - a + 1 } else { total };
    let mut buf = bytes::BytesMut::with_capacity(expect as usize);
    while let Some(chunk) = tokio::time::timeout(STALL, resp.chunk()).await.map_err(|_| stalled())?.map_err(reqwest::Error::without_url)? {
        buf.extend_from_slice(&chunk);
    }
    anyhow::ensure!(buf.len() as u64 == expect, "short read: {} of {expect} bytes", buf.len());
    if partial {
        return Ok(buf.freeze());
    }
    Ok(buf.freeze().slice(a as usize..=b as usize))
}

/// Whether the server honours a `Range` header with a 206. A server that answers 200
/// gets one sequential download instead. A probe that can't connect leaves the workers
/// to try ranges themselves (a 200 there is still handled).
async fn ranges_supported(http: &reqwest::Client, url: &str, ua: &str) -> bool {
    match tokio::time::timeout(STALL, http.get(url).header("User-Agent", ua).header("Range", "bytes=0-0").header("Accept-Encoding", "identity").send()).await {
        Ok(Ok(r)) => r.status() == reqwest::StatusCode::PARTIAL_CONTENT,
        _ => true,
    }
}

/// One sequential download for a server that won't serve ranges, filling `shared` block
/// by block as bytes arrive so playback can start before the file is complete.
async fn download_whole(http: &reqwest::Client, url: &str, ua: &str, shared: &Arc<Shared>) {
    let stalled = || anyhow::anyhow!("no data for {} s", STALL.as_secs());
    let result = async {
        let mut resp = tokio::time::timeout(
            STALL,
            http.get(url).header("User-Agent", ua).header("Accept-Encoding", "identity").send(),
        )
        .await
        .map_err(|_| stalled())?
        .and_then(reqwest::Response::error_for_status)
        .map_err(reqwest::Error::without_url)?;
        let blocks = shared.inner.lock().unwrap().blocks.len();
        // The first block is smaller than the rest (see `FIRST`).
        let lens: Vec<usize> = (0..blocks).map(|i| {
            let (a, b) = shared.range(i);
            (b - a + 1) as usize
        }).collect();
        let mut buf = bytes::BytesMut::with_capacity(BLOCK as usize);
        let (mut idx, mut received) = (0usize, 0u64);
        while let Some(chunk) = tokio::time::timeout(STALL, resp.chunk()).await.map_err(|_| stalled())?.map_err(reqwest::Error::without_url)? {
            received += chunk.len() as u64;
            buf.extend_from_slice(&chunk);
            while idx < blocks && buf.len() >= lens[idx] {
                shared.put(idx, buf.split_to(lens[idx]).freeze());
                idx += 1;
            }
            if shared.inner.lock().unwrap().cancelled {
                return anyhow::Ok(());
            }
        }
        anyhow::ensure!(received == shared.total, "short read: {received} of {} bytes", shared.total);
        while idx < blocks {
            shared.put(idx, buf.split_to(lens[idx]).freeze());
            idx += 1;
        }
        anyhow::Ok(())
    }
    .await;
    match result {
        Ok(()) => {
            shared.done.send_replace(true);
        }
        Err(e) => {
            if !shared.inner.lock().unwrap().cancelled {
                shared.fail(format!("stream: {e:#}"), false);
            }
        }
    }
}

pub struct StreamReader {
    shared: Arc<Shared>,
    pos: u64,
    id: u64,
    /// The block this reader last read.
    block: usize,
}

impl StreamReader {
    pub fn new(shared: Arc<Shared>) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        {
            let mut inner = shared.inner.lock().unwrap();
            inner.readers += 1;
            inner.limit = inner.blocks.len();
            inner.wants.push((id, 0));
        }
        shared.changed.notify_waiters();
        Self { shared, pos: 0, id, block: 0 }
    }
}

impl Drop for StreamReader {
    fn drop(&mut self) {
        let mut inner = self.shared.inner.lock().unwrap();
        inner.readers -= 1;
        inner.wants.retain(|&(id, _)| id != self.id);
        if inner.readers == 0 {
            // Stops the workers once their current request finishes.
            inner.cancelled = true;
        }
        self.shared.changed.notify_waiters();
    }
}

impl StreamReader {
    /// Block `idx`, once it's here; an error if the download failed or stopped first.
    pub fn block_at(&mut self, idx: usize) -> io::Result<Bytes> {
        let mut inner = self.shared.inner.lock().unwrap();
        inner.want = idx;
        if idx != self.block {
            self.block = idx;
            if let Some(w) = inner.wants.iter_mut().find(|(id, _)| *id == self.id) {
                w.1 = idx;
            }
            if inner.windowed {
                self.shared.changed.notify_waiters();
            }
        }
        loop {
            if let Some(b) = &inner.blocks[idx] {
                return Ok(b.clone());
            }
            if let Some(e) = &inner.error {
                return Err(io::Error::other(e.clone()));
            }
            if inner.cancelled {
                return Err(io::Error::other("download cancelled"));
            }
            inner = self.shared.ready.wait(inner).unwrap();
        }
    }
}

impl Read for StreamReader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() || self.pos >= self.shared.total {
            return Ok(0);
        }
        let idx = block_of(self.pos);
        let block = self.block_at(idx)?;
        let off = (self.pos - block_start(idx)) as usize;
        let n = out.len().min(block.len() - off);
        out[..n].copy_from_slice(&block[off..off + n]);
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for StreamReader {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let total = self.shared.total as i64;
        let new = match to {
            SeekFrom::Start(p) => p as i64,
            SeekFrom::End(off) => total + off,
            SeekFrom::Current(off) => self.pos as i64 + off,
        };
        if new < 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "seek before start"));
        }
        self.pos = new as u64;
        Ok(self.pos)
    }
}

/// Open a decoder over `shared`. Non-seekable AAC decoders start after the first
/// fragment; seekable ones index every fragment first (i.e. effectively wait for the
/// download). Opus ones are always seekable: WebM's index is at the start. Raw PCM
/// (decoded in-process, e.g. Dolby Atmos) plays straight from the buffer.
pub fn decoder(shared: Arc<Shared>, seekable: bool) -> anyhow::Result<TrackDecoder> {
    if let Some(starts) = shared.segments.clone() {
        let duration_ms = shared.duration_ms;
        return Ok(TrackDecoder::Hls(Box::new(crate::hls::HlsSource::new(StreamReader::new(shared), starts, duration_ms)?)));
    }
    let size = shared.total;
    // A feed's file is probed for what it is: MP3, or AAC in an MP4 whose index may
    // follow the audio (only a seekable reader gets to it). Without a VBR header, an
    // MP3's length is guessed from its first frames, and gapless trimming would cut
    // whatever runs past the guess.
    if shared.feed {
        let builder = rodio::Decoder::builder().with_data(StreamReader::new(shared)).with_byte_len(size).with_seekable(true).with_coarse_seek(true).with_gapless(false);
        return Ok(TrackDecoder::Rodio(builder.build()?));
    }
    match shared.format {
        Format::Opus => {
            let duration_ms = shared.duration_ms;
            Ok(TrackDecoder::Opus(Box::new(crate::opus::OpusSource::new(StreamReader::new(shared), size, duration_ms)?)))
        }
        Format::Pcm { rate, channels } => Ok(TrackDecoder::Pcm(PcmSource::new(shared, rate, channels))),
        Format::Aac => {
            let builder = rodio::Decoder::builder()
                .with_data(StreamReader::new(shared.clone()))
                .with_byte_len(size)
                .with_seekable(seekable)
                .with_mime_type("audio/mp4")
                .with_hint("m4a");
            Ok(TrackDecoder::Rodio(builder.build()?))
        }
        Format::File { ref hint, ref mime } => {
            let hint = *hint;
            let mime = mime.clone();
            let builder = rodio::Decoder::builder()
                .with_data(StreamReader::new(shared))
                .with_byte_len(size)
                .with_seekable(seekable)
                .with_hint(hint);
            let builder = match mime.as_deref() {
                Some(mime) => builder.with_mime_type(mime),
                None => builder,
            };
            Ok(TrackDecoder::Rodio(builder.build()?))
        }
    }
}

/// What the decoder is being fed, for stats for nerds: the codec, the rate and channels
/// it decodes to, and the bitrate. The bit depth is the one a hi-res source states, as
/// rodio's decoders don't report theirs.
#[derive(Clone, Debug, PartialEq)]
pub struct Stats {
    pub codec: String,
    pub bit_depth: Option<u32>,
    pub sample_rate: u32,
    pub channels: u16,
    pub bitrate_kbps: Option<u32>,
}

impl Stats {
    /// `shared` as `decoder` reads it.
    pub fn of(shared: &Shared, decoder: &TrackDecoder) -> Self {
        let rate = rodio::Source::sample_rate(decoder).get();
        let channels = rodio::Source::channels(decoder).get();
        Self::from_parts(&shared.format, shared.info.as_ref(), shared.total, shared.duration_ms, rate, channels)
    }

    fn from_parts(format: &Format, info: Option<&crate::hires::Info>, size: u64, duration_ms: u64, sample_rate: u32, channels: u16) -> Self {
        let codec = match info.map(|i| i.codec.trim()).filter(|c| !c.is_empty()) {
            Some(c) => codec_label(c),
            None => match format {
                Format::Aac => "AAC".to_owned(),
                Format::Opus => "Opus".to_owned(),
                Format::File { hint, .. } => codec_label(hint),
                Format::Pcm { .. } => "PCM".to_owned(),
            },
        };
        let bit_depth = info.and_then(|i| i.bit_depth);
        let measured = (duration_ms > 0 && size != crate::hires::UNKNOWN_SIZE).then(|| (size * 8 / duration_ms) as u32);
        let bitrate_kbps = match bit_depth {
            Some(bits) if info.is_some_and(|i| i.lossless()) => Some(sample_rate * bits * channels as u32 / 1000),
            _ => info.and_then(|i| i.bitrate_kbps).or(measured),
        };
        Self { codec, bit_depth, sample_rate, channels, bitrate_kbps: bitrate_kbps.filter(|k| *k > 0) }
    }
}

fn codec_label(codec: &str) -> String {
    match codec.to_ascii_lowercase().as_str() {
        "opus" => "Opus".to_owned(),
        "vorbis" => "Vorbis".to_owned(),
        _ => codec.to_ascii_uppercase(),
    }
}

/// A song's decoder: Opus (our own), in-process PCM (decoded Atmos), or a file
/// rodio decodes (AAC/MP4 included).
pub enum TrackDecoder {
    Rodio(rodio::Decoder<StreamReader>),
    /// Boxed: its decoder state is about 11 KB.
    Opus(Box<crate::opus::OpusSource>),
    Pcm(PcmSource),
    /// AAC from HLS segments.
    Hls(Box<crate::hls::HlsSource>),
}

impl Iterator for TrackDecoder {
    type Item = rodio::Sample;

    fn next(&mut self) -> Option<rodio::Sample> {
        match self {
            Self::Rodio(d) => d.next(),
            Self::Opus(d) => d.next(),
            Self::Pcm(d) => d.next(),
            Self::Hls(d) => d.next(),
        }
    }
}

impl rodio::Source for TrackDecoder {
    fn current_span_len(&self) -> Option<usize> {
        match self {
            Self::Rodio(d) => d.current_span_len(),
            Self::Opus(d) => d.current_span_len(),
            Self::Pcm(d) => d.current_span_len(),
            Self::Hls(d) => d.current_span_len(),
        }
    }
    fn channels(&self) -> rodio::ChannelCount {
        match self {
            Self::Rodio(d) => d.channels(),
            Self::Opus(d) => d.channels(),
            Self::Pcm(d) => d.channels(),
            Self::Hls(d) => d.channels(),
        }
    }
    fn sample_rate(&self) -> rodio::SampleRate {
        match self {
            Self::Rodio(d) => d.sample_rate(),
            Self::Opus(d) => d.sample_rate(),
            Self::Pcm(d) => d.sample_rate(),
            Self::Hls(d) => d.sample_rate(),
        }
    }
    fn total_duration(&self) -> Option<Duration> {
        match self {
            Self::Rodio(d) => d.total_duration(),
            Self::Opus(d) => d.total_duration(),
            Self::Pcm(d) => d.total_duration(),
            Self::Hls(d) => d.total_duration(),
        }
    }
    fn try_seek(&mut self, pos: Duration) -> Result<(), rodio::source::SeekError> {
        match self {
            Self::Rodio(d) => d.try_seek(pos),
            Self::Opus(d) => d.try_seek(pos),
            Self::Pcm(d) => d.try_seek(pos),
            Self::Hls(d) => d.try_seek(pos),
        }
    }
}

/// Interleaved signed 16-bit little-endian PCM read from the stream buffer.
pub struct PcmSource {
    reader: StreamReader,
    rate: u32,
    channels: u16,
    duration: Option<Duration>,
    /// PCM pulled from the reader at once: a sample at a time would take the buffer's
    /// lock per sample, on the device's thread.
    chunk: Box<[u8; CHUNK]>,
    /// How much of `chunk` holds unread PCM.
    len: usize,
    at: usize,
}

/// Two spans of the resampler's view at stereo, and past that just a slice of a block.
const CHUNK: usize = 8192;

impl PcmSource {
    fn new(shared: Arc<Shared>, rate: u32, channels: u16) -> Self {
        let duration_ms = shared.duration_ms;
        Self {
            reader: StreamReader::new(shared),
            rate,
            channels,
            duration: (duration_ms > 0).then(|| Duration::from_millis(duration_ms)),
            chunk: Box::new([0; CHUNK]),
            len: 0,
            at: 0,
        }
    }
}

impl Iterator for PcmSource {
    type Item = rodio::Sample;
    fn next(&mut self) -> Option<rodio::Sample> {
        if self.at + 2 > self.len {
            self.at = 0;
            // A short read stops at a block boundary: a shorter chunk serves the next
            // refill sooner. A trailing odd byte can't make a sample, and reading again
            // past the end returns 0.
            self.len = self.reader.read(&mut self.chunk[..]).ok()?;
            if self.len == 0 {
                return None;
            }
        }
        let s = i16::from_le_bytes([self.chunk[self.at], self.chunk[self.at + 1]]);
        self.at += 2;
        Some(s as f32 / 32768.0)
    }
}

impl rodio::Source for PcmSource {
    /// A finite span, as rodio's own decoders give per decoded packet. The resampler
    /// around the player's queue reads the song's rate when a span ends, and a `None`
    /// here freezes it: every song after a decoded-DASH one would then be converted
    /// at this song's rate — a 44.1 kHz song after 48 kHz PCM plays 9 % fast, a
    /// 96 kHz one an octave low.
    fn current_span_len(&self) -> Option<usize> {
        let ch = self.channels.max(1) as usize;
        Some((4096 / ch) * ch)
    }
    fn channels(&self) -> rodio::ChannelCount { std::num::NonZeroU16::new(self.channels).unwrap_or(std::num::NonZeroU16::MIN) }
    fn sample_rate(&self) -> rodio::SampleRate { std::num::NonZeroU32::new(self.rate).unwrap_or(std::num::NonZeroU32::MIN) }
    fn total_duration(&self) -> Option<Duration> { self.duration }
    fn try_seek(&mut self, pos: Duration) -> Result<(), rodio::source::SeekError> {
        let frame = (pos.as_secs_f64() * self.rate as f64) as u64;
        let byte = frame * self.channels as u64 * 2;
        self.reader.seek(SeekFrom::Start(byte)).map_err(|e| rodio::source::SeekError::Other(std::sync::Arc::new(e)))?;
        self.at = 0;
        self.len = 0;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use rodio::Source;

    /// A real podcast's newest episode played the way an RSS episode is: found past its
    /// tracking redirects, downloaded in `Range` requests, decoded as MP3, seeked.
    /// `cargo test --release -- --ignored feed_file_live --nocapture`
    #[tokio::test]
    #[ignore]
    async fn feed_file_live() {
        let (http, ua) = (reqwest::Client::new(), crate::webmusic::UA);
        let rss = http.get("https://feeds.npr.org/510289/podcast.xml").header("User-Agent", ua).send().await.unwrap().text().await.unwrap();
        let tag = &rss[rss.find("<enclosure").unwrap()..];
        let url = tag[tag.find(" url=\"").unwrap() + 6..].split('"').next().unwrap().replace("&amp;", "&");
        let (file, size) = super::probe_feed(&http, &url, ua).await.unwrap();
        println!("{url}\n  -> {file}: {size} bytes");
        let shared = super::Shared::feed(size, 0, false);
        let t = Instant::now();
        super::download(http, super::Media::Feed(file), ua.to_owned(), shared.clone(), None, super::Ranging::Header).await;
        assert!(shared.error().is_none(), "{:?}", shared.error());
        println!("  downloaded in {:?}", t.elapsed());
        let mut d = super::decoder(shared.clone(), true).unwrap();
        let (ch, rate) = (d.channels().get() as usize, d.sample_rate().get() as usize);
        let secs = d.by_ref().count() as f64 / (ch * rate) as f64;
        println!("  {ch} ch {rate} Hz, {secs:.1} s");
        assert!(secs > 60.0, "too short: {secs} s");
        let mut d = super::decoder(shared, true).unwrap();
        let half = Duration::from_secs_f64(secs / 2.0);
        let t = Instant::now();
        d.try_seek(half).unwrap();
        println!("  seek to {:.1} s took {:?}", half.as_secs_f64(), t.elapsed());
        let left = d.count() as f64 / (ch * rate) as f64;
        println!("  {left:.1} s left");
        // Coarse: an estimate from the bitrate.
        assert!((left - (secs - half.as_secs_f64())).abs() < 5.0, "seek off");
    }
}
