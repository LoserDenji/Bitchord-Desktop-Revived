//! Apple's motion sleeves, played locally and on every platform.
//!
//! The clip is HLS, but each variant's media playlist is a single byte-ranged
//! fragmented MP4, so it downloads as one file. That file's H.264 is decoded on the
//! CPU with OpenH264 — no system video api involved, so the same code runs on
//! Windows, macOS and Linux. Only the newest frame is kept: the UI turns it into a
//! texture and draws it in place of the still sleeve.
//!
//! The clip loops on its own clock, which only runs while the music plays: paused,
//! the sleeve holds still, the way Apple Music's does.

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
    url: String,
    latest: Mutex<Option<Arc<Frame>>>,
    version: AtomicU64,
    stop: Arc<AtomicBool>,
    /// Whether the music is playing: the clip's clock only runs while it is.
    playing: Arc<AtomicBool>,
}

impl Motion {
    fn new(url: &str) -> Self {
        Self { url: url.to_owned(), latest: Mutex::new(None), version: AtomicU64::new(0), stop: Arc::new(AtomicBool::new(false)), playing: Arc::new(AtomicBool::new(true)) }
    }

    /// The newest frame, with the counter that says it changed.
    pub fn latest(&self) -> Option<(u64, Arc<Frame>)> {
        let frame = self.latest.lock().ok()?.clone()?;
        Some((self.version.load(Ordering::Acquire), frame))
    }

    pub fn url(&self) -> &str {
        &self.url
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

/// Start (or reuse) the clip at `master`, decoded no wider than `max_width`.
pub fn play(ctx: &egui::Context, master: &str, max_width: u32) -> Arc<Motion> {
    let key = format!("{max_width}:{master}");
    if let Ok(mut streams) = STREAMS.lock() {
        if let Some(m) = streams.get(&key).and_then(Weak::upgrade) {
            return m;
        }
        let motion = Arc::new(Motion::new(master));
        streams.insert(key, Arc::downgrade(&motion));
        let (this, ctx, url) = (motion.clone(), ctx.clone(), master.to_owned());
        let _ = std::thread::Builder::new().name("motion".into()).spawn(move || run(this, ctx, url, max_width));
        return motion;
    }
    Arc::new(Motion::new(master))
}

fn run(motion: Arc<Motion>, ctx: egui::Context, master: String, max_width: u32) {
    let Some(path) = fetch(&master, max_width) else {
        log::info!("apple: no motion clip downloaded");
        return;
    };
    decode(&motion, &path, &ctx, max_width);
}

/// Pick a variant and save the clip to the cache; a saved copy is reused.
fn fetch(master: &str, max_width: u32) -> Option<PathBuf> {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().ok()?;
    rt.block_on(async {
        use sha1::{Digest, Sha1};
        let http = crate::proxy::http();
        let media = crate::apple::variant(&http, master, max_width).await?;
        let file = crate::apple::media_file(&http, &media).await?;
        let dir = crate::data_dir().join("motion");
        std::fs::create_dir_all(&dir).ok()?;
        let name = format!("{:x}", Sha1::digest(file.as_bytes()));
        let path = dir.join(format!("{}.mp4", &name[..32]));
        if std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) > 1024 {
            return Some(path);
        }
        let bytes = http
            .get(&file)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .ok()?
            .error_for_status()
            .ok()?
            .bytes()
            .await
            .ok()?;
        let tmp = path.with_extension("part");
        std::fs::write(&tmp, &bytes).ok()?;
        std::fs::rename(&tmp, &path).ok()?;
        log::info!("apple: motion clip {:.0} KB cached", bytes.len() as f64 / 1024.0);
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
}

fn parse_track(moov: &[u8]) -> Option<Mp4Track> {
    let mdhd = find(moov, &["trak", "mdia", "mdhd"])?;
    let timescale = if mdhd[0] == 1 { be32(mdhd, 20)? } else { be32(mdhd, 12)? };
    let stsd = find(moov, &["trak", "mdia", "minf", "stbl", "stsd"])?;
    let at = stsd.windows(4).position(|w| w == b"avcC")? + 4;
    let c = stsd.get(at..)?;
    let nal_len = (*c.get(4)? & 3) as usize + 1;
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
    let start = find(moov, &["trak", "edts", "elst"]).and_then(|e| if e[0] == 1 { be64(e, 16).map(|v| v as i64) } else { be32(e, 12).map(|v| v as i32 as i64) }).filter(|v| *v > 0).unwrap_or(0);
    Some(Mp4Track { params, nal_len, timescale, default_duration, start })
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
    let mut time = if tfdt[0] == 1 { be64(tfdt, 4)? as i64 } else { be32(tfdt, 4)? as i64 };
    let scale = track.timescale.max(1) as f64;
    // Counted from the start of the `moof`, as Apple's files say to.
    let mut at = moof_at;
    // Every run of samples: Apple's clips have two in each fragment, and a run without
    // an offset of its own carries on from the one before.
    for trun in boxes(traf).filter(|(kind, _)| *kind == b"trun").map(|(_, body)| body) {
        let flags = be32(trun, 0)? & 0xff_ffff;
        let count = be32(trun, 4)? as usize;
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
fn scaled_rgba(yuv: &openh264::decoder::DecodedYUV, step: usize) -> (usize, usize, Vec<u8>) {
    let (w, h) = yuv.dimensions();
    let step = step.max(1);
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
            let (cu, cv) = (ur[sx / 2] as i32 - 128, vr[sx / 2] as i32 - 128);
            // The BT.601 weights write_rgba8 uses, in 1024ths.
            let c = |x: i32| x.clamp(0, 255) as u8;
            rgba.extend_from_slice(&[c(l + ((1436 * cv) >> 10)), c(l - ((352 * cu + 731 * cv) >> 10)), c(l + ((1815 * cu) >> 10)), 255]);
        }
    }
    (ow, oh, rgba)
}

/// The clip's frames, decoded lap after lap on its own clock, which only runs while the
/// music plays. Stops when the sleeve is dropped.
fn decode(motion: &Arc<Motion>, path: &std::path::Path, ctx: &egui::Context, max_width: u32) {
    let Ok(bytes) = std::fs::read(path) else { return };
    let Some(track) = find(&bytes, &["moov"]).and_then(parse_track) else {
        log::info!("apple: the motion clip isn't H.264 in MP4");
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
        log::info!("apple: no frames in the motion clip");
        return;
    }
    log::info!("apple: motion clip has {} frame(s)", samples.len());
    let side = max_width.max(64);
    let mut clock = 0.0_f64;
    let mut last = Instant::now();
    'laps: for _lap in 0u64.. {
        // A fresh decoder each lap: the clip starts on a key frame, with nothing held over.
        let mut decoder = match new_decoder(&track) {
            Ok(d) => d,
            Err(e) => {
                log::warn!("apple: motion decode failed: {e}");
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
            let (w, h, rgba) = scaled_rgba(&yuv, (yuv.dimensions().0 / side as usize).max(1));
            if motion.version.load(Ordering::Acquire) == 0 {
                log::info!("apple: motion playing at {w}x{h}");
            }
            motion.publish(ctx, w, h, rgba);
        }
        // The last few, which the decoder holds back for reordering.
        for yuv in decoder.flush_remaining().unwrap_or_default().iter() {
            let Some(Pts(due)) = pending.pop() else { break };
            if !wait_for(&motion.playing, &motion.stop, &mut clock, &mut last, due) {
                break 'laps;
            }
            let (w, h, rgba) = scaled_rgba(yuv, (yuv.dimensions().0 / side as usize).max(1));
            motion.publish(ctx, w, h, rgba);
        }
    }
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
    fn every_run_of_a_fragment_is_read() {
        // Two runs in one track fragment, as in Apple's clips: the second has no data
        // offset of its own and carries on in time and in the file from the first.
        let tfhd = boxed(b"tfhd", &u32s(&[0x8 | 0x10, 1, 1, 10]));
        let tfdt = boxed(b"tfdt", &u32s(&[0, 0]));
        let first = boxed(b"trun", &u32s(&[0x1, 2, 200]));
        let second = boxed(b"trun", &u32s(&[0, 3]));
        let traf = boxed(b"traf", &[tfhd, tfdt, first, second].concat());
        let moof = boxed(b"moof", &traf);
        let track = Mp4Track { params: vec![], nal_len: 4, timescale: 30, default_duration: 0, start: 0 };
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
        let track = Mp4Track { params: vec![], nal_len: 4, timescale: 1000, default_duration: 40, start: 80 };
        let samples = parse_fragment(&moof, &track);
        // First: decode time 0 + 80 - start 80. Second: decode time 40 + 120 - 80.
        assert_eq!(samples, [Sample { at: 500, size: 10, pts: 0.0 }, Sample { at: 510, size: 20, pts: 0.08 }]);
    }

    #[test]
    fn a_fragment_cut_short_has_no_made_up_samples() {
        let moof = boxed(b"moof", &[0; 40]);
        let track = Mp4Track { params: vec![], nal_len: 4, timescale: 1000, default_duration: 40, start: 0 };
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
        let motion = Arc::new(Motion::new("test"));
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
