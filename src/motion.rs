//! Animated covers, played locally and on every platform.
//!
//! The clip is found by [`crate::canvas`] (a port of ytm-rs's lookup). It is HLS, but
//! each variant's media playlist is a fragmented MP4, so it downloads as one file.
//! That file's H.264 is decoded on the CPU with OpenH264 — no system video api
//! involved, so the same code runs on Windows, macOS and Linux. Only the newest frame
//! is kept: the UI turns it into a texture and draws it in place of the still cover.
//!
//! The clip loops on its own clock, which only runs while the music plays: paused,
//! the cover holds still, the way Apple Music's does.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, Weak};
use std::time::{Duration, Instant};

use openh264::OpenH264API;
use openh264::decoder::{Decoder, DecoderConfig, Flush};
use openh264::formats::YUVSource;

pub struct Frame {
    pub w: usize,
    pub h: usize,
    pub rgba: Vec<u8>,
}

pub struct Motion {
    latest: Mutex<Option<Arc<Frame>>>,
    version: AtomicU64,
    stop: Arc<AtomicBool>,
    /// Whether the music is playing: the clip's clock only runs while it is.
    playing: Arc<AtomicBool>,
}

impl Motion {
    fn new() -> Self {
        Self { latest: Mutex::new(None), version: AtomicU64::new(0), stop: Arc::new(AtomicBool::new(false)), playing: Arc::new(AtomicBool::new(true)) }
    }

    /// The newest frame, with the counter that says it changed.
    pub fn latest(&self) -> Option<(u64, Arc<Frame>)> {
        let frame = self.latest.lock().ok()?.clone()?;
        Some((self.version.load(Ordering::Acquire), frame))
    }

    /// Stop decoding: the page that showed this clip is gone.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// Whether the clip's clock runs. Paused, the sleeve holds its last frame.
    pub fn set_playing(&self, on: bool) {
        self.playing.store(on, Ordering::Relaxed);
    }

    fn publish(&self, ctx: &egui::Context, w: usize, h: usize, rgba: Vec<u8>) {
        if let Ok(mut slot) = self.latest.lock() {
            *slot = Some(Arc::new(Frame { w, h, rgba }));
        }
        self.version.fetch_add(1, Ordering::Release);
        ctx.request_repaint();
    }
}

impl Drop for Motion {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

static STREAMS: LazyLock<Mutex<HashMap<String, Weak<Motion>>>> = LazyLock::new(Default::default);

/// Clips the disk cache keeps, dropping the oldest first; a dropped one re-downloads
/// when its sleeve shows again.
const DISK_BUDGET: u64 = 128 * 1024 * 1024;

/// Keep the clip cache under its budget, dropping the least recently written files.
fn trim(dir: &std::path::Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<_> = rd
        .flatten()
        .filter_map(|f| {
            let m = f.metadata().ok()?;
            Some((m.modified().ok()?, m.len(), f.path()))
        })
        .collect();
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    files.sort();
    for (_, len, path) in files {
        if total <= DISK_BUDGET {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= len;
        }
    }
}

/// Start (or reuse) the clip at `master`, decoded no wider than `max_width`.
pub fn play(ctx: &egui::Context, master: &str, max_width: u32) -> Arc<Motion> {
    let key = format!("{max_width}:{master}");
    if let Ok(mut streams) = STREAMS.lock() {
        // Views come and go; without this their keys' dead Weaks pile up for the session.
        streams.retain(|_, w| w.strong_count() > 0);
        if let Some(m) = streams.get(&key).and_then(Weak::upgrade) {
            return m;
        }
        let motion = Arc::new(Motion::new());
        streams.insert(key, Arc::downgrade(&motion));
        let (this, ctx, url) = (motion.clone(), ctx.clone(), master.to_owned());
        let _ = std::thread::Builder::new().name("motion".into()).spawn(move || run(this, ctx, url, max_width));
        return motion;
    }
    Arc::new(Motion::new())
}

/// Start (or reuse) a clip that is one plain mp4 (Spotify's canvas), not Apple's HLS.
pub fn play_mp4(ctx: &egui::Context, url: &str, max_width: u32) -> Arc<Motion> {
    let key = format!("mp4:{max_width}:{url}");
    if let Ok(mut streams) = STREAMS.lock() {
        streams.retain(|_, w| w.strong_count() > 0);
        if let Some(m) = streams.get(&key).and_then(Weak::upgrade) {
            return m;
        }
        let motion = Arc::new(Motion::new());
        streams.insert(key, Arc::downgrade(&motion));
        let (this, ctx, url) = (motion.clone(), ctx.clone(), url.to_owned());
        let _ = std::thread::Builder::new().name("motion".into()).spawn(move || run_mp4(this, ctx, url, max_width));
        return motion;
    }
    Arc::new(Motion::new())
}

fn run(motion: Arc<Motion>, ctx: egui::Context, master: String, max_width: u32) {
    let Some(path) = fetch(&master, max_width) else {
        log::info!("animated cover: no clip downloaded");
        return;
    };
    decode(&motion, &path, &ctx, max_width);
}

fn run_mp4(motion: Arc<Motion>, ctx: egui::Context, url: String, max_width: u32) {
    let Some(path) = fetch_mp4(&url) else {
        log::info!("canvas: clip didn't download");
        return;
    };
    decode(&motion, &path, &ctx, max_width);
}

fn fetch_mp4(url: &str) -> Option<PathBuf> {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().ok()?;
    rt.block_on(cache_mp4(&crate::proxy::http(), &crate::data_dir(), url, usize::MAX)).ok()
}

/// Share the source clip between the canvas player and the Discord GIF encoder.
pub(crate) async fn cache_mp4(http: &reqwest::Client, data_dir: &std::path::Path, url: &str, limit: usize) -> anyhow::Result<PathBuf> {
    use sha1::{Digest, Sha1};
    static DOWNLOAD: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _download = DOWNLOAD.lock().await;
    let dir = data_dir.join("motion");
    std::fs::create_dir_all(&dir)?;
    let name = format!("{:x}", Sha1::digest(url.as_bytes()));
    let path = dir.join(format!("{}.mp4", &name[..32]));
    if let Ok(meta) = std::fs::metadata(&path)
        && meta.len() > 1024
    {
        anyhow::ensure!(meta.len() <= limit as u64, "canvas clip is too large");
        return Ok(path);
    }
    let mut reply = http
        .get(url)
        .timeout(Duration::from_secs(30))
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36")
        .send()
        .await?
        .error_for_status()?;
    let mut bytes = Vec::new();
    while let Some(chunk) = reply.chunk().await? {
        anyhow::ensure!(chunk.len() <= limit.saturating_sub(bytes.len()), "canvas clip is too large");
        bytes.extend_from_slice(&chunk);
    }
    let tmp = path.with_extension("part");
    let written = std::fs::write(&tmp, &bytes).and_then(|()| std::fs::rename(&tmp, &path));
    let _ = std::fs::remove_file(&tmp);
    written?;
    trim(&dir);
    log::info!("canvas: clip {:.0} KB cached", bytes.len() as f64 / 1024.0);
    Ok(path)
}

/// Pick a rendition and save the clip to the cache; a saved copy is reused. The
/// download is ytm-rs's [`crate::canvas::load`]: it picks the H.264 rendition for
/// `max_width` out of the master playlist and hands over the clip's init segment and
/// media segments, which together are one fragmented MP4.
fn fetch(master: &str, max_width: u32) -> Option<PathBuf> {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().ok()?;
    rt.block_on(async {
        use sha1::{Digest, Sha1};
        let http = crate::proxy::http();
        let clip = crate::canvas::load(&http, master, max_width).await.ok()?;
        let dir = crate::data_dir().join("motion");
        std::fs::create_dir_all(&dir).ok()?;
        // Per size: a wider view decodes a bigger rendition of the same playlist.
        let name = format!("{:x}", Sha1::digest(format!("{max_width}\u{1}{master}").as_bytes()));
        let path = dir.join(format!("{}.mp4", &name[..32]));
        if std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) > 1024 {
            return Some(path);
        }
        // Init segment first, then the media segments: together a complete fMP4 file.
        let mut bytes = clip.init.clone();
        for seg in &clip.segments {
            bytes.extend_from_slice(seg);
        }
        let tmp = path.with_extension("part");
        std::fs::write(&tmp, &bytes).ok()?;
        std::fs::rename(&tmp, &path).ok()?;
        trim(&dir);
        log::info!("animated cover: clip {:.0} KB cached", bytes.len() as f64 / 1024.0);
        Some(path)
    })
}

// ---------------------------------------------------------------------------- MP4

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn be64(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_be_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

/// The complete boxes in `b`: type, contents.
fn boxes(b: &[u8]) -> impl Iterator<Item = (&[u8], &[u8])> {
    let mut at = 0;
    std::iter::from_fn(move || {
        let size = be32(b, at)? as usize;
        let body = b.get(at + 8..at + size).filter(|_| size >= 8)?;
        let kind = &b[at + 4..at + 8];
        at += size;
        Some((kind, body))
    })
}

/// The first box along `path`.
fn find<'a>(b: &'a [u8], path: &[&str]) -> Option<&'a [u8]> {
    let (head, rest) = path.split_first()?;
    let body = boxes(b).find(|(kind, _)| *kind == head.as_bytes())?.1;
    if rest.is_empty() { Some(body) } else { find(body, rest) }
}

/// What the file's start (`moov`) says about all of it.
#[derive(Debug, PartialEq)]
struct Mp4Track {
    /// The decoder's setup, as length-prefixed-free NAL units with start codes.
    params: Vec<u8>,
    /// Bytes in each NAL unit's length prefix.
    nal_len: usize,
    timescale: u32,
    default_duration: u32,
    /// The presentation time of the first sample, in timescale units.
    start: i64,
    /// How the clip's numbers turn into colours, as its stream's header says.
    colors: crate::video::Colors,
}

fn h264_track(moov: &[u8]) -> Option<&[u8]> {
    boxes(moov)
        .filter(|(kind, _)| *kind == b"trak")
        .map(|(_, b)| b)
        .find(|b| find(b, &["mdia", "minf", "stbl", "stsd"]).is_some_and(|s| s.windows(4).any(|w| w == b"avcC")))
}

fn parse_track(moov: &[u8]) -> Option<Mp4Track> {
    let video = h264_track(moov)?;
    let mdhd = find(video, &["mdia", "mdhd"])?;
    let timescale = if *mdhd.first()? == 1 { be32(mdhd, 20)? } else { be32(mdhd, 12)? };
    let stsd = find(video, &["mdia", "minf", "stbl", "stsd"])?;
    let at = stsd.windows(4).position(|w| w == b"avcC")? + 4;
    let c = stsd.get(at..)?;
    let nal_len = (*c.get(4)? & 3) as usize + 1;
    // The first SPS's: the one a stream starts with.
    let colors = c.get(6..8).and_then(|n| c.get(8..8 + u16::from_be_bytes([n[0], n[1]]) as usize)).and_then(crate::video::sps_colors).unwrap_or_default();
    let mut params = Vec::new();
    let mut o = 6;
    let mut copy = |count: usize, o: &mut usize| -> Option<()> {
        for _ in 0..count {
            let n = u16::from_be_bytes([*c.get(*o)?, *c.get(*o + 1)?]) as usize;
            params.extend_from_slice(&[0, 0, 0, 1]);
            params.extend_from_slice(c.get(*o + 2..*o + 2 + n)?);
            *o += 2 + n;
        }
        Some(())
    };
    copy((*c.get(5)? & 31) as usize, &mut o)?;
    let pps = *c.get(o)? as usize;
    o += 1;
    copy(pps, &mut o)?;
    let default_duration = find(moov, &["mvex", "trex"]).and_then(|t| be32(t, 12)).unwrap_or(0);
    // With B-frames the first frame shows late; the edit list takes that back out.
    let start = find(video, &["edts", "elst"])
        .and_then(|e| if *e.first()? == 1 { be64(e, 16).map(|v| v as i64) } else { be32(e, 12).map(|v| v as i32 as i64) })
        .filter(|v| *v > 0)
        .unwrap_or(0);
    Some(Mp4Track { params, nal_len, timescale, default_duration, start, colors })
}

/// A frame's bytes in the fragment, and when it shows.
#[derive(Debug, PartialEq)]
struct Sample {
    at: usize,
    size: usize,
    /// Seconds.
    pts: f64,
}

/// The samples of a whole fragment (`moof` and `mdat` pairs), in decode order.
fn parse_fragment(b: &[u8], track: &Mp4Track) -> Vec<Sample> {
    let mut out = Vec::new();
    let mut start = 0;
    while let Some(size) = be32(b, start).map(|s| s as usize) {
        if size < 8 || start + size > b.len() {
            break;
        }
        if &b[start + 4..start + 8] == b"moof" {
            parse_moof(&b[start + 8..start + size], start, track, &mut out);
        }
        start += size;
    }
    out
}

fn parse_moof(moof: &[u8], moof_at: usize, track: &Mp4Track, out: &mut Vec<Sample>) -> Option<()> {
    let traf = find(moof, &["traf"])?;
    let tfhd = find(traf, &["tfhd"])?;
    let flags = be32(tfhd, 0)? & 0xff_ffff;
    let mut o = 8;
    if flags & 0x1 != 0 {
        o += 8;
    }
    if flags & 0x2 != 0 {
        o += 4;
    }
    let default_duration = if flags & 0x8 != 0 {
        o += 4;
        be32(tfhd, o - 4)?
    } else {
        track.default_duration
    };
    let default_size = if flags & 0x10 != 0 { be32(tfhd, o)? } else { 0 };
    let tfdt = find(traf, &["tfdt"])?;
    let mut time = if *tfdt.first()? == 1 { be64(tfdt, 4)? as i64 } else { be32(tfdt, 4)? as i64 };
    let scale = track.timescale.max(1) as f64;
    // Counted from the start of the `moof`, as Apple's files say to.
    let mut at = moof_at;
    // Every run of samples: Apple's clips have two in each fragment, and a run without
    // an offset of its own carries on from the one before.
    for trun in boxes(traf).filter(|(kind, _)| *kind == b"trun").map(|(_, body)| body) {
        let flags = be32(trun, 0)? & 0xff_ffff;
        let count = be32(trun, 4)? as usize;
        if count > 8_000usize.saturating_sub(out.len()) {
            return None;
        }
        let mut o = 8;
        if flags & 0x1 != 0 {
            at = (moof_at as i64 + be32(trun, o)? as i32 as i64) as usize;
            o += 4;
        }
        if flags & 0x4 != 0 {
            o += 4;
        }
        for _ in 0..count {
            let duration = if flags & 0x100 != 0 {
                o += 4;
                be32(trun, o - 4)?
            } else {
                default_duration
            };
            let size = if flags & 0x200 != 0 {
                o += 4;
                be32(trun, o - 4)?
            } else {
                default_size
            } as usize;
            if flags & 0x400 != 0 {
                o += 4;
            }
            let offset = if flags & 0x800 != 0 {
                o += 4;
                be32(trun, o - 4)? as i32 as i64
            } else {
                0
            };
            out.push(Sample { at, size, pts: (time + offset - track.start) as f64 / scale });
            at += size;
            time += duration as i64;
        }
    }
    Some(())
}

/// A non-fragmented MP4 (Spotify usually serves these) indexes samples by chunk.
fn flat_samples(moov: &[u8], track: &Mp4Track) -> Option<Vec<Sample>> {
    let tables = find(h264_track(moov)?, &["mdia", "minf", "stbl"])?;
    let sizes = find(tables, &["stsz"])?;
    let count = be32(sizes, 8)? as usize;
    if count > 8000 {
        return None;
    }
    let fixed = be32(sizes, 4)? as usize;
    let durations = sample_runs(find(tables, &["stts"])?, count)?;
    let offsets = find(tables, &["ctts"]).and_then(|b| sample_runs(b, count));
    let chunks = find(tables, &["stsc"])?;
    let (offsets_box, width) = find(tables, &["stco"]).map(|b| (b, 4)).or_else(|| find(tables, &["co64"]).map(|b| (b, 8)))?;
    let chunk_count = be32(offsets_box, 4)? as usize;
    if chunk_count > count || 8 + chunk_count * width > offsets_box.len() {
        return None;
    }
    let mut rule = 8;
    let mut time = 0i64;
    let mut samples = Vec::with_capacity(count);
    for chunk in 1..=chunk_count {
        if be32(chunks, rule + 12) == Some(chunk as u32) {
            rule += 12;
        }
        let per_chunk = be32(chunks, rule + 4)? as usize;
        let start = 8 + (chunk - 1) * width;
        let mut at = if width == 4 { be32(offsets_box, start)? as usize } else { usize::try_from(be64(offsets_box, start)?).ok()? };
        for _ in 0..per_chunk {
            let i = samples.len();
            let size = if fixed == 0 { be32(sizes, 12 + i * 4)? as usize } else { fixed };
            let offset = offsets.as_ref().and_then(|o| o.get(i)).copied().unwrap_or_default();
            let pts = (time + i64::from(offset) - track.start) as f64 / f64::from(track.timescale.max(1));
            samples.push(Sample { at, size, pts });
            at = at.checked_add(size)?;
            time += i64::from(*durations.get(i)?);
        }
    }
    (samples.len() == count).then_some(samples)
}

fn sample_runs(table: &[u8], count: usize) -> Option<Vec<i32>> {
    let mut values = Vec::with_capacity(count);
    let entries = be32(table, 4)? as usize;
    for pair in table.get(8..)?.as_chunks::<8>().0.iter().take(entries) {
        let run = be32(pair, 0)? as usize;
        let value = be32(pair, 4)? as i32;
        let end = values.len().checked_add(run)?;
        if end > count {
            return None;
        }
        values.resize(end, value);
    }
    (values.len() == count).then_some(values)
}

fn clip_samples(bytes: &[u8], moov: &[u8], track: &Mp4Track) -> Vec<Sample> {
    let fragments = parse_fragment(bytes, track);
    if fragments.is_empty() { flat_samples(moov, track).unwrap_or_default() } else { fragments }
}

/// Decode a single lap, selecting only the frames a bounded Discord GIF will keep.
pub(crate) fn visit_clip(bytes: &[u8], side: u32, fps: u8, mut frame: impl FnMut(usize, usize, Vec<u8>, u32) -> anyhow::Result<()>) -> anyhow::Result<()> {
    use anyhow::{Context, ensure};
    let moov = find(bytes, &["moov"]).context("clip has no MP4 header")?;
    let track = parse_track(moov).context("clip isn't H.264")?;
    let samples = clip_samples(bytes, moov, &track);
    ensure!((2..=8000).contains(&samples.len()), "clip has an unsupported frame count");
    let (times, length) = loop_times(&samples.iter().map(|s| s.pts).collect::<Vec<_>>());
    ensure!((0.04..=60.0).contains(&length), "clip has an unsupported duration");
    let length_cs = (length * 100.0).round() as u32;
    let interval = (100.0 / f64::from(fps)).max(f64::from(length_cs) / 359.0);
    let mut decoder = new_decoder(&track)?;
    let mut pending = std::collections::BinaryHeap::new();
    let mut previous = None;
    let mut next = 0.0;
    let mut selected = 0;
    let mut take = |yuv: &openh264::decoder::DecodedYUV, Pts(t): Pts| -> anyhow::Result<()> {
        let at = (t * 100.0).round() as u32;
        if (t * 100.0 + 0.5 < next && selected != 0) || at + 2 > length_cs || selected >= 360 {
            return Ok(());
        }
        let (w, h) = yuv.dimensions();
        ensure!(w <= 4096 && h <= 4096, "clip frame is too large");
        let (w, h, rgba) = scaled_rgba(yuv, (w.min(h) / side as usize).max(1), track.colors);
        if let Some((before, w, h, rgba)) = previous.replace((at, w, h, rgba)) {
            frame(w, h, rgba, (at - before) * 10)?;
        }
        selected += 1;
        next = (f64::from(selected) * interval).max(f64::from(at + 2));
        Ok(())
    };
    for (sample, t) in samples.into_iter().zip(times) {
        let raw = bytes.get(sample.at..sample.at.checked_add(sample.size).context("invalid sample size")?).context("invalid sample offset")?;
        pending.push(Pts(t));
        if let Some(yuv) = decoder.decode(&annex_b(raw, track.nal_len))? {
            take(&yuv, pending.pop().context("missing sample time")?)?;
        }
    }
    for yuv in decoder.flush_remaining()?.iter() {
        take(yuv, pending.pop().context("missing sample time")?)?;
    }
    ensure!(selected >= 2, "clip has no animation");
    if let Some((at, w, h, rgba)) = previous {
        frame(w, h, rgba, (length_cs - at) * 10)?;
    }
    Ok(())
}

/// A sample's length-prefixed NAL units with start codes, as the decoder takes them.
fn annex_b(sample: &[u8], nal_len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(sample.len() + 16);
    let mut i = 0;
    while i + nal_len <= sample.len() {
        let len = sample[i..i + nal_len].iter().fold(0usize, |n, b| n << 8 | *b as usize);
        i += nal_len;
        let Some(nal) = sample.get(i..i + len) else { break };
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(nal);
        i += len;
    }
    out
}

fn new_decoder(track: &Mp4Track) -> anyhow::Result<Decoder> {
    // Apple's H.264 has B-frames: flushing after every sample runs the decoder out of
    // picture buffers within a few frames.
    let config = DecoderConfig::new().flush_after_decode(Flush::NoFlush);
    let mut decoder = Decoder::with_api_config(OpenH264API::from_source(), config)?;
    for nal in openh264::nal_units(&track.params) {
        decoder.decode(nal)?;
    }
    Ok(decoder)
}

/// `yuv` at 1/`step` of its size, as RGBA the texture takes. Smaller in the conversion
/// itself: it costs as much as decoding at full size, and the sleeve shows no more than
/// its own size. Each pixel's light is the average of the 2×2 it starts.
fn scaled_rgba(yuv: &openh264::decoder::DecodedYUV, step: usize, colors: crate::video::Colors) -> (usize, usize, Vec<u8>) {
    let (w, h) = yuv.dimensions();
    let step = step.max(1);
    let m = colors.to_rgb();
    let (ow, oh) = (w / step, h / step);
    let (ys, us, vs) = yuv.strides();
    let (y, u, v) = (yuv.y(), yuv.u(), yuv.v());
    let mut rgba = Vec::with_capacity(ow * oh * 4);
    for oy in 0..oh {
        let sy = oy * step;
        let (row, below) = (&y[sy * ys..], &y[(sy + (step > 1) as usize) * ys..]);
        let (ur, vr) = (&u[sy / 2 * us..], &v[sy / 2 * vs..]);
        for ox in 0..ow {
            let sx = ox * step;
            let l = if step > 1 { (row[sx] as i32 + row[sx + 1] as i32 + below[sx] as i32 + below[sx + 1] as i32 + 2) >> 2 } else { row[sx] as i32 };
            let p = m.rgb(l, ur[sx / 2], vr[sx / 2]);
            rgba.extend_from_slice(&[p.r(), p.g(), p.b(), 255]);
        }
    }
    (ow, oh, rgba)
}

/// The clip's frames, decoded lap after lap on its own clock, which only runs while the
/// music plays. Stops when the sleeve is dropped.
fn decode(motion: &Arc<Motion>, path: &std::path::Path, ctx: &egui::Context, max_width: u32) {
    let Ok(bytes) = std::fs::read(path) else { return };
    let Some(track) = find(&bytes, &["moov"]).and_then(parse_track) else {
        log::info!("animated cover: the clip isn't H.264 in MP4");
        return;
    };
    // Every frame of the clip, as the decoder takes it: a sleeve is short, and this is
    // a few MB at most.
    let samples: Vec<(Vec<u8>, f64)> = parse_fragment(&bytes, &track)
        .into_iter()
        .filter_map(|s| {
            let data = annex_b(bytes.get(s.at..s.at + s.size)?, track.nal_len);
            (!data.is_empty()).then_some((data, s.pts))
        })
        .collect();
    if samples.is_empty() {
        log::info!("animated cover: no frames in the clip");
        return;
    }
    // Every frame at its own pace, from the first frame's time: some clips showed only
    // about half their frames and played the rest too fast, so the picture cut and jumped.
    let (times, length) = loop_times(&samples.iter().map(|s| s.1).collect::<Vec<_>>());
    let samples: Vec<(Vec<u8>, f64)> = samples.into_iter().zip(times).map(|((data, _), t)| (data, t)).collect();
    log::info!("animated cover: {} frames, {length:.1} s", samples.len());
    let side = max_width.max(64);
    let mut last = Instant::now();
    'laps: for _lap in 0u64.. {
        // Each lap runs the clock from the top: the frame times start over with it.
        let mut clock = 0.0_f64;
        // A fresh decoder each lap: the clip starts on a key frame, with nothing held over.
        let mut decoder = match new_decoder(&track) {
            Ok(d) => d,
            Err(e) => {
                log::warn!("animated cover: decode failed: {e}");
                return;
            }
        };
        let mut pending: std::collections::BinaryHeap<Pts> = Default::default();
        for (data, pts) in &samples {
            pending.push(Pts(*pts));
            let Ok(Some(yuv)) = decoder.decode(data) else { continue };
            // The decoder hands frames back in display order; its pending times say when.
            let Some(Pts(due)) = pending.pop() else { continue };
            if !wait_for(&motion.playing, &motion.stop, &mut clock, &mut last, due) {
                break 'laps;
            }
            let (w, h, rgba) = scaled_rgba(&yuv, (yuv.dimensions().0 / side as usize).max(1), track.colors);
            if motion.version.load(Ordering::Acquire) == 0 {
                log::info!("animated cover: playing at {w}x{h}");
            }
            motion.publish(ctx, w, h, rgba);
        }
        // The last few, which the decoder holds back for reordering.
        for yuv in decoder.flush_remaining().unwrap_or_default().iter() {
            let Some(Pts(due)) = pending.pop() else { break };
            if !wait_for(&motion.playing, &motion.stop, &mut clock, &mut last, due) {
                break 'laps;
            }
            let (w, h, rgba) = scaled_rgba(yuv, (yuv.dimensions().0 / side as usize).max(1), track.colors);
            motion.publish(ctx, w, h, rgba);
        }
        // The lap's length: the last frame gets its step before the clip starts over.
        if !wait_for(&motion.playing, &motion.stop, &mut clock, &mut last, length) {
            break 'laps;
        }
    }
}

/// A clip's frame times from its first frame, and how long one lap lasts: up to the last
/// frame's end, a frame's usual step after it.
fn loop_times(pts: &[f64]) -> (Vec<f64>, f64) {
    let mut sorted = pts.to_vec();
    sorted.sort_by(f64::total_cmp);
    let (Some(first), Some(last)) = (sorted.first().copied(), sorted.last().copied()) else { return (Vec::new(), 0.0) };
    // The step most frames are apart, counted by whole milliseconds and averaged.
    let mut counts: std::collections::HashMap<i64, (usize, f64)> = std::collections::HashMap::new();
    for w in sorted.windows(2) {
        let d = w[1] - w[0];
        let ms = (d * 1000.0).round() as i64;
        if ms > 0 {
            let c = counts.entry(ms).or_default();
            *c = (c.0 + 1, c.1 + d);
        }
    }
    let step = counts.into_values().max_by(|a, b| a.0.cmp(&b.0).then(b.1.total_cmp(&a.1))).map_or(1.0 / 30.0, |(n, sum)| sum / n as f64);
    (pts.iter().map(|p| p - first).collect(), last - first + step)
}

/// Wait until the clip's clock reaches `due`. The clock only advances while the music
/// plays; false once the sleeve is dropped.
fn wait_for(playing: &AtomicBool, stop: &AtomicBool, clock: &mut f64, last: &mut Instant, due: f64) -> bool {
    loop {
        if stop.load(Ordering::Relaxed) {
            return false;
        }
        let now = Instant::now();
        if playing.load(Ordering::Relaxed) {
            *clock += (now - *last).as_secs_f64().min(0.25);
        }
        *last = now;
        if *clock >= due {
            return true;
        }
        std::thread::sleep(Duration::from_secs_f64((due - *clock).clamp(0.004, 0.05)));
    }
}

/// Times as sorted on the way out of the decoder, which returns frames in display order.
#[derive(PartialEq)]
struct Pts(f64);
impl Eq for Pts {}
impl PartialOrd for Pts {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Pts {
    // Smallest first out of the heap.
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other.0.total_cmp(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boxed(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(body);
        v
    }

    fn u32s(values: &[u32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_be_bytes()).collect()
    }

    #[test]
    fn a_clip_keeps_its_own_times_and_loops_after_its_last_frame() {
        // Decode order, B-frames shown out of it, starting late as Apple's do.
        let frame = 1.0 / 30.0;
        let pts = [10.0, 13.0, 11.0, 12.0].map(|n| n * frame);
        let (times, length) = loop_times(&pts);
        for (t, w) in times.iter().zip([0.0, 3.0, 1.0, 2.0].map(|n| n * frame)) {
            assert!((t - w).abs() < 1e-9, "{times:?}");
        }
        assert!((length - 4.0 * frame).abs() < 1e-9);
        assert_eq!(loop_times(&[]).0, Vec::<f64>::new());
    }

    #[test]
    fn every_run_of_a_fragment_is_read() {
        // Two runs in one track fragment, as in Apple's clips: the second has no data
        // offset of its own and carries on in time and in the file from the first.
        let tfhd = boxed(b"tfhd", &u32s(&[0x8 | 0x10, 1, 1, 10]));
        let tfdt = boxed(b"tfdt", &u32s(&[0, 0]));
        let first = boxed(b"trun", &u32s(&[0x1, 2, 200]));
        let second = boxed(b"trun", &u32s(&[0, 3]));
        let traf = boxed(b"traf", &[tfhd, tfdt, first, second].concat());
        let moof = boxed(b"moof", &traf);
        let track = Mp4Track { params: vec![], nal_len: 4, timescale: 30, default_duration: 0, start: 0, colors: Default::default() };
        let samples = parse_fragment(&moof, &track);
        assert_eq!(samples.len(), 5);
        assert_eq!(samples.iter().map(|s| s.at).collect::<Vec<_>>(), [200, 210, 220, 230, 240]);
        assert!((samples[4].pts - 4.0 / 30.0).abs() < 1e-9);
    }

    #[test]
    fn samples_show_in_display_order_with_the_first_at_zero() {
        // tfhd: no defaults. tfdt v0: base 0. trun: data offset, size and composition offset per sample.
        let tfhd = boxed(b"tfhd", &u32s(&[0, 1]));
        let tfdt = boxed(b"tfdt", &u32s(&[0, 0]));
        let flags = 0x1 | 0x200 | 0x800;
        let trun = boxed(b"trun", &u32s(&[flags, 2, 500, 10, 80, 20, 120]));
        let traf = boxed(b"traf", &[tfhd, tfdt, trun].concat());
        let moof = boxed(b"moof", &traf);
        let track = Mp4Track { params: vec![], nal_len: 4, timescale: 1000, default_duration: 40, start: 80, colors: Default::default() };
        let samples = parse_fragment(&moof, &track);
        // First: decode time 0 + 80 - start 80. Second: decode time 40 + 120 - 80.
        assert_eq!(samples, [Sample { at: 500, size: 10, pts: 0.0 }, Sample { at: 510, size: 20, pts: 0.08 }]);
    }

    #[test]
    fn a_fragment_cut_short_has_no_made_up_samples() {
        let moof = boxed(b"moof", &[0; 40]);
        let track = Mp4Track { params: vec![], nal_len: 4, timescale: 1000, default_duration: 40, start: 0, colors: Default::default() };
        assert!(parse_fragment(&moof[..20], &track).is_empty());
        assert!(parse_fragment(&moof, &track).is_empty());
    }

    #[test]
    fn frames_leave_the_decoder_soonest_first() {
        let mut heap = std::collections::BinaryHeap::new();
        for p in [0.2, 0.0, 0.1] {
            heap.push(Pts(p));
        }
        assert_eq!([heap.pop().unwrap().0, heap.pop().unwrap().0, heap.pop().unwrap().0], [0.0, 0.1, 0.2]);
    }

    /// Decoding a real cached sleeve. Needs a clip in the cache (play something in the
    /// app first): `cargo test --release -- --ignored decodes_a_cached_clip`.
    #[test]
    #[ignore]
    fn decodes_a_cached_clip() {
        let dir = crate::data_dir().join("motion");
        let path = std::fs::read_dir(&dir)
            .expect("no motion cache")
            .flatten()
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|e| e == "mp4"))
            .expect("no cached clip");
        let ctx = egui::Context::default();
        let motion = Arc::new(Motion::new());
        let this = motion.clone();
        std::thread::spawn(move || decode(&this, &path, &ctx, 400));
        let start = Instant::now();
        while motion.latest().is_none() && start.elapsed() < Duration::from_secs(15) {
            std::thread::sleep(Duration::from_millis(50));
        }
        motion.stop();
        let (_, frame) = motion.latest().expect("no frame decoded");
        assert!(frame.w > 0 && frame.h > 0, "empty frame");
        assert_eq!(frame.rgba.len(), frame.w * frame.h * 4);
    }
}
