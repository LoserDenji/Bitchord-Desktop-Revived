//! Dolby Atmos and FLAC out of a DASH manifest: an addon hands over an MPD whose
//! rendition is fetched whole and reworked into something the player decodes like any
//! other file. Atmos: the E-AC-3 rendition decoded to stereo PCM and wrapped in a WAV
//! (Windows has no E-AC-3 decoder of its own and symphonia doesn't carry one;
//! filmcraft-ac3 does the decode here). FLAC: Tidal's hi-res DASH carries the song as
//! fMP4 fragments of raw FLAC frames; with the init segment's STREAMINFO in front, the
//! fragments reassemble into one .flac byte stream the player's own decoder reads.

use std::time::Duration;

use filmcraft_ac3::{Decoder, parse_header};
use futures_util::StreamExt;

const FETCH_TIMEOUT: Duration = Duration::from_secs(20);
/// A rendition bigger than this isn't a song; don't hold it in memory.
const MAX_BYTES: usize = 96 * 1024 * 1024;
const PARALLEL: usize = 6;

pub struct Audio {
    /// Interleaved stereo 16-bit PCM, little-endian.
    pub pcm: Vec<u8>,
    pub sample_rate: u32,
    pub duration_ms: u64,
    pub bitrate_kbps: u32,
}

/// A reassembled FLAC rendition.
pub struct FlacAudio {
    /// A complete FLAC byte stream: the magic, STREAMINFO and every frame.
    pub bytes: Vec<u8>,
    pub sample_rate: u32,
    pub bit_depth: u32,
    pub channels: u8,
    pub duration_ms: u64,
    pub bitrate_kbps: u32,
}

/// Which rendition of the manifest to take.
#[derive(Clone, Copy)]
pub enum Want {
    /// E-AC-3 (JOC) first, plain AC-3 second: what an Atmos request plays.
    Dolby,
    /// The FLAC rendition: what a lossless request plays when the addon serves
    /// hi-res as DASH (Tidal's "quality first" mode does, whatever tier is asked).
    Flac,
}

fn rank_of(want: Want, codecs: &str) -> i32 {
    let c = codecs.to_ascii_lowercase();
    match want {
        Want::Dolby => {
            if c.contains("ec-3") {
                0
            } else if c.contains("ac-3") {
                1
            } else {
                i32::MAX
            }
        }
        Want::Flac => {
            if c.contains("flac") {
                0
            } else {
                i32::MAX
            }
        }
    }
}

/// Fetch the manifest's Dolby (E-)AC-3 rendition and decode it to stereo PCM.
pub async fn fetch(http: &reqwest::Client, mpd_url: &str) -> anyhow::Result<Audio> {
    let text = http.get(mpd_url).timeout(FETCH_TIMEOUT).header("User-Agent", crate::hires::USER_AGENT).send().await?.error_for_status()?.text().await?;
    let plan = Plan::parse(&text, mpd_url, Want::Dolby)?;
    let bytes = download(http, &plan).await?;
    let (pcm, sample_rate) = tokio::task::spawn_blocking(move || decode(&bytes)).await??;
    anyhow::ensure!(!pcm.is_empty(), "the Atmos rendition decoded to nothing");
    let duration_ms = pcm.len() as u64 / 2 * 1000 / sample_rate as u64;
    let mut pcm_bytes = Vec::with_capacity(pcm.len() * 2);
    for s in pcm {
        pcm_bytes.extend_from_slice(&s.to_le_bytes());
    }
    Ok(Audio { pcm: pcm_bytes, sample_rate, duration_ms, bitrate_kbps: plan.bandwidth / 1000 })
}

/// Fetch the manifest's FLAC rendition and reassemble it into one FLAC byte stream.
pub async fn fetch_flac(http: &reqwest::Client, mpd_url: &str) -> anyhow::Result<FlacAudio> {
    let text = http.get(mpd_url).timeout(FETCH_TIMEOUT).header("User-Agent", crate::hires::USER_AGENT).send().await?.error_for_status()?.text().await?;
    let plan = Plan::parse(&text, mpd_url, Want::Flac)?;
    let bytes = download(http, &plan).await?;
    let (stream, si) = reassemble(&bytes)?;
    anyhow::ensure!(!stream.is_empty(), "the FLAC rendition reassembled to nothing");
    let duration_ms = if si.sample_rate > 0 && si.samples > 0 {
        si.samples * 1000 / si.sample_rate as u64
    } else {
        plan.duration_ms
    };
    Ok(FlacAudio { bytes: stream, sample_rate: si.sample_rate, bit_depth: si.bit_depth as u32, channels: si.channels, duration_ms, bitrate_kbps: plan.bandwidth / 1000 })
}

/// The plan's init and every media segment, in order, into one buffer.
async fn download(http: &reqwest::Client, plan: &Plan) -> anyhow::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&get(http, &plan.init).await?);
    let mut stream = futures_util::stream::iter(plan.segments.iter().cloned().map(|u| {
        let http = http.clone();
        async move { get(&http, &u).await }
    })).buffered(PARALLEL);
    while let Some(part) = stream.next().await {
        let part = part?;
        anyhow::ensure!(bytes.len() + part.len() <= MAX_BYTES, "the rendition is too large");
        bytes.extend_from_slice(&part);
    }
    Ok(bytes)
}

async fn get(http: &reqwest::Client, url: &str) -> anyhow::Result<Vec<u8>> {
    let r = http.get(url).timeout(FETCH_TIMEOUT).header("User-Agent", crate::hires::USER_AGENT).send().await?.error_for_status()?;
    Ok(r.bytes().await?.to_vec())
}

/// The rendition's segment addresses, read out of the MPD.
struct Plan {
    init: String,
    segments: Vec<String>,
    bandwidth: u32,
    /// The period's duration, for when the frames themselves don't say how long they are.
    duration_ms: u64,
}

impl Plan {
    fn parse(xml: &str, mpd_url: &str, want: Want) -> anyhow::Result<Self> {
        let doc = roxmltree::Document::parse(xml)?;
        let mpd = doc.root_element();
        anyhow::ensure!(mpd.tag_name().name() == "MPD", "not a DASH manifest");
        let base = url::Url::parse(mpd_url)?;
        let period = mpd.children().find(|n| n.has_tag_name("Period")).ok_or_else(|| anyhow::anyhow!("the manifest has no period"))?;
        let period_ms = duration_attr(mpd.attribute("mediaPresentationDuration")).or_else(|| duration_attr(period.attribute("duration"))).unwrap_or(0);
        let mut reps: Vec<roxmltree::Node> = period.children().filter(|n| n.has_tag_name("AdaptationSet")).flat_map(|a| a.children().filter(|n| n.has_tag_name("Representation")).map(move |r| (a, r))).filter(|(a, r)| {
            let mime = r.attribute("mimeType").or_else(|| a.attribute("mimeType")).unwrap_or("");
            mime.is_empty() || mime.starts_with("audio/")
        }).map(|(_, r)| r).collect();
        reps.sort_by_key(|r| rank_of(want, r.attribute("codecs").unwrap_or("")));
        let rep = reps.into_iter().find(|r| rank_of(want, r.attribute("codecs").unwrap_or("")) < i32::MAX).ok_or_else(|| {
            anyhow::anyhow!("the manifest has no {} rendition", if matches!(want, Want::Dolby) { "Dolby" } else { "FLAC" })
        })?;
        let bandwidth = rep.attribute("bandwidth").and_then(|v| v.parse().ok()).unwrap_or(0);
        // SegmentTemplate and BaseURL may sit on any level from the representation up.
        fn up<'a, 'i>(n: roxmltree::Node<'a, 'i>, tag: &str) -> Option<roxmltree::Node<'a, 'i>> {
            let mut cur = Some(n);
            while let Some(c) = cur {
                if let Some(found) = c.children().find(|k| k.has_tag_name(tag)) {
                    return Some(found);
                }
                cur = c.parent();
            }
            None
        }
        let template = up(rep, "SegmentTemplate").ok_or_else(|| anyhow::anyhow!("the rendition has no segment template"))?;
        let base = {
            let mut cur = Some(rep);
            let mut found = None;
            while let Some(c) = cur {
                if let Some(b) = c.children().find(|k| k.has_tag_name("BaseURL")).and_then(|b| b.text()) {
                    found = Some(b.trim().to_owned());
                    break;
                }
                cur = c.parent();
            }
            match found {
                Some(b) => base.join(&b)?,
                None => base,
            }
        };
        let address = |pattern: &str, number: u64, time: u64| -> anyhow::Result<String> {
            let mut s = pattern.replace("$RepresentationID$", rep.attribute("id").unwrap_or(""));
            s = s.replace("$Bandwidth$", &bandwidth.to_string());
            s = s.replace("$Time$", &time.to_string());
            // $Number$ or $Number%05d$.
            while let Some(start) = s.find("$Number") {
                let end = s[start + 1..].find('$').map(|i| start + i + 2).ok_or_else(|| anyhow::anyhow!("unclosed $Number$"))?;
                let token = &s[start + 1..end - 1];
                let width = token.split('%').nth(1).and_then(|w| w.trim_start_matches('0').trim_end_matches('d').parse::<usize>().ok());
                let text = match width {
                    Some(w) => format!("{number:0w$}"),
                    None => number.to_string(),
                };
                s.replace_range(start..end, &text);
            }
            Ok(if url::Url::parse(&s).is_ok() { s } else { base.join(&s)?.into() })
        };
        let timescale: u64 = template.attribute("timescale").and_then(|v| v.parse().ok()).unwrap_or(1);
        let start: u64 = template.attribute("startNumber").and_then(|v| v.parse().ok()).unwrap_or(1);
        let init = address(template.attribute("initialization").ok_or_else(|| anyhow::anyhow!("no initialization segment"))?, start, 0)?;
        let media = template.attribute("media").ok_or_else(|| anyhow::anyhow!("no media template"))?;
        let mut segments = Vec::new();
        if let Some(timeline) = template.children().find(|n| n.has_tag_name("SegmentTimeline")) {
            let (mut time, mut number) = (0u64, start);
            for s in timeline.children().filter(|n| n.has_tag_name("S")) {
                if let Some(t) = s.attribute("t").and_then(|v| v.parse::<u64>().ok()) {
                    time = t;
                }
                let d: u64 = s.attribute("d").and_then(|v| v.parse().ok()).ok_or_else(|| anyhow::anyhow!("a segment has no duration"))?;
                let r: i64 = s.attribute("r").and_then(|v| v.parse().ok()).unwrap_or(0);
                let count = if r < 0 {
                    // Repeats until the period ends.
                    ((period_ms as u128 * timescale as u128 / 1000).saturating_sub(time as u128) / d as u128) as u64
                } else {
                    r as u64 + 1
                };
                for _ in 0..count {
                    segments.push(address(media, number, time)?);
                    time += d;
                    number += 1;
                }
            }
        } else if let Some(d) = template.attribute("duration").and_then(|v| v.parse::<u64>().ok()) {
            let total = period_ms as u128 * timescale as u128 / 1000;
            let count = total.div_ceil(d as u128) as u64;
            anyhow::ensure!(count > 0 && count < 100_000, "the rendition's length is unknown");
            for i in 0..count {
                segments.push(address(media, start + i, i * d)?);
            }
        }
        anyhow::ensure!(!segments.is_empty(), "the rendition lists no segments");
        Ok(Self { init, segments, bandwidth, duration_ms: period_ms })
    }
}

/// `PT3M23.648S` and friends, in milliseconds.
fn duration_attr(v: Option<&str>) -> Option<u64> {
    let s = v?.strip_prefix("PT")?;
    let (mut ms, mut num) = (0u64, String::new());
    for c in s.chars() {
        match c {
            '0'..='9' | '.' => num.push(c),
            'H' => { ms += (num.parse::<f64>().ok()? * 3_600_000.0) as u64; num.clear(); }
            'M' => { ms += (num.parse::<f64>().ok()? * 60_000.0) as u64; num.clear(); }
            'S' => { ms += (num.parse::<f64>().ok()? * 1000.0) as u64; num.clear(); }
            _ => return None,
        }
    }
    Some(ms)
}

/// What the init segment says about the FLAC track.
struct StreamInfo {
    /// The 34 bytes themselves, verbatim, for the reassembled stream's header.
    info: Vec<u8>,
    sample_rate: u32,
    channels: u8,
    bit_depth: u32,
    samples: u64,
}

/// fMP4 FLAC frames and the init segment's STREAMINFO, out of the fetched buffer:
/// the init is everything before the first media fragment, and the frames are every
/// mdat payload after it.
fn reassemble(bytes: &[u8]) -> anyhow::Result<(Vec<u8>, StreamInfo)> {
    let split = init_end(bytes)?;
    let si = streaminfo(&bytes[..split]).ok_or_else(|| anyhow::anyhow!("the FLAC rendition's init carries no STREAMINFO"))?;
    let mut out = Vec::with_capacity(4 + 38 + (bytes.len() - split));
    out.extend_from_slice(b"fLaC");
    // STREAMINFO's metadata-block header: last block set, block type 0, 34 bytes.
    out.extend_from_slice(&[0x80, 0x00, 0x00, 0x22]);
    out.extend_from_slice(&si.info);
    for payload in mdat_payloads(&bytes[split..]) {
        out.extend_from_slice(payload);
    }
    Ok((out, si))
}

/// Where the init segment ends and the media fragments begin: the first moof box.
fn init_end(bytes: &[u8]) -> anyhow::Result<usize> {
    let mut i = 0usize;
    while i + 8 <= bytes.len() {
        let (header, size) = match u32::from_be_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]) as usize {
            1 if i + 16 <= bytes.len() => (16, u64::from_be_bytes(bytes[i + 8..i + 16].try_into().unwrap()) as usize),
            0 => (8, bytes.len() - i),
            n => (8, n),
        };
        if size < header || i + size > bytes.len() {
            break;
        }
        if &bytes[i + header - 4..i + header] == b"moof" {
            return Ok(i);
        }
        i += size;
    }
    anyhow::bail!("the FLAC rendition has no media segments")
}

/// The 34 STREAMINFO bytes inside the init segment's `dfLa` box (Tidal's name; servers
/// also write `dfla`, and pad the payload, so the bytes are found by their
/// metadata-block header — block type 0, 24-bit length of 34 — with a last resort of
/// any 34 bytes that read back as a sane STREAMINFO).
fn streaminfo(init: &[u8]) -> Option<StreamInfo> {
    let sane = |si: &[u8]| -> Option<StreamInfo> {
        if si.len() != 34 {
            return None;
        }
        let rate = ((si[10] as u32) << 12) | ((si[11] as u32) << 4) | (si[12] >> 4) as u32;
        let channels = (((si[12] >> 1) & 7) + 1) as u8;
        let bit_depth = ((((si[12] & 1) as u32) << 4) | (si[13] >> 4) as u32) + 1;
        let samples = (((si[13] & 0x0F) as u64) << 32) | u32::from_be_bytes([si[14], si[15], si[16], si[17]]) as u64;
        // A misread window decodes to nonsense; real rates are the standard set.
        const RATES: [u32; 16] = [8000, 11025, 12000, 16000, 22050, 24000, 32000, 44100, 48000, 56000, 64000, 88200, 96000, 128000, 176400, 192000];
        (RATES.contains(&rate) && (1..=8).contains(&channels) && (4..=32).contains(&bit_depth) && samples > 0)
            .then(|| StreamInfo { info: si.to_owned(), sample_rate: rate, channels, bit_depth, samples })
    };
    // The block header: low 7 bits are the block type (0 for STREAMINFO), then a
    // 24-bit length of exactly 34.
    for i in 0..init.len().saturating_sub(38) {
        if init[i] & 0x7F == 0 && init[i + 1] == 0 && init[i + 2] == 0 && init[i + 3] == 34 {
            if let Some(si) = sane(&init[i + 4..i + 38]) {
                return Some(si);
            }
        }
    }
    for i in 0..init.len().saturating_sub(34) {
        if let Some(si) = sane(&init[i..i + 34]) {
            return Some(si);
        }
    }
    None
}

/// The E-AC-3 syncframes inside the fMP4's mdat boxes, decoded and downmixed to
/// interleaved stereo i16. Dependent substreams (the JOC objects beyond the 5.1
/// bed) carry no program of their own and are skipped.
fn decode(bytes: &[u8]) -> anyhow::Result<(Vec<i16>, u32)> {
    let mut decoder = Decoder::new();
    let mut pcm: Vec<i16> = Vec::new();
    let mut rate = 0u32;
    for payload in mdat_payloads(bytes) {
        let mut i = 0usize;
        while i + 7 <= payload.len() {
            if payload[i] != 0x0B || payload[i + 1] != 0x77 {
                i += 1;
                continue;
            }
            let Ok(h) = parse_header(&payload[i..]) else { i += 1; continue };
            if h.frame_bytes == 0 || i + h.frame_bytes > payload.len() {
                i += 1;
                continue;
            }
            let frame = &payload[i..i + h.frame_bytes];
            i += h.frame_bytes;
            if !h.is_primary() {
                continue;
            }
            match decoder.decode(frame) {
                Ok(d) => {
                    if rate == 0 {
                        rate = d.header.sample_rate;
                    }
                    downmix(&d.channels, &mut pcm);
                }
                // Enhanced coupling / AHT and the like: this copy can't play, so the
                // caller falls back to the addon's next one.
                Err(filmcraft_ac3::Error::Unsupported(why)) => anyhow::bail!("the Atmos stream uses {why}"),
                Err(_) => {}
            }
        }
    }
    anyhow::ensure!(rate > 0, "no Dolby syncframes in the rendition");
    Ok((pcm, rate))
}

/// Top-level mdat payloads of an (fragmented) MP4.
fn mdat_payloads(bytes: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 8 <= bytes.len() {
        let size = u32::from_be_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]) as usize;
        let kind = &bytes[i + 4..i + 8];
        let (header, size) = match size {
            1 if i + 16 <= bytes.len() => (16, u64::from_be_bytes(bytes[i + 8..i + 16].try_into().unwrap()) as usize),
            0 => (8, bytes.len() - i),
            _ => (8, size),
        };
        if size < header || i + size > bytes.len() {
            break;
        }
        if kind == b"mdat" {
            out.push(&bytes[i + header..i + size]);
        }
        i += size;
    }
    out
}

/// The 5.1 bed (WAV order L R C LFE Ls Rs) folded to stereo, centre and surrounds
/// at -3 dB, LFE left out; smaller layouts keep their front pair.
fn downmix(channels: &[Vec<f32>], pcm: &mut Vec<i16>) {
    let n = channels.first().map_or(0, Vec::len);
    let mut push = |l: f32, r: f32| {
        pcm.push((l.clamp(-1.0, 1.0) * 32767.0) as i16);
        pcm.push((r.clamp(-1.0, 1.0) * 32767.0) as i16);
    };
    const C: f32 = std::f32::consts::FRAC_1_SQRT_2;
    match channels.len() {
        0 => {}
        1 => for i in 0..n { push(channels[0][i], channels[0][i]); },
        2 => for i in 0..n { push(channels[0][i], channels[1][i]); },
        // L R C
        3 => for i in 0..n { push(channels[0][i] + C * channels[2][i], channels[1][i] + C * channels[2][i]); },
        // L R Ls Rs
        4 => for i in 0..n { push(channels[0][i] + C * channels[2][i], channels[1][i] + C * channels[3][i]); },
        // L R C Ls Rs
        5 => for i in 0..n { push(channels[0][i] + C * (channels[2][i] + channels[3][i]), channels[1][i] + C * (channels[2][i] + channels[4][i])); },
        // L R C LFE Ls Rs
        _ => for i in 0..n { push(channels[0][i] + C * (channels[2][i] + channels[4][i]), channels[1][i] + C * (channels[2][i] + channels[5][i])); },
    }
}

fn wav(pcm: &[i16], sample_rate: u32) -> Vec<u8> {
    let data = (pcm.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + pcm.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&2u16.to_le_bytes()); // stereo
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 4).to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for s in pcm {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_durations() {
        assert_eq!(duration_attr(Some("PT3M23.648S")), Some(203_648));
        assert_eq!(duration_attr(Some("PT1H2M3S")), Some(3_723_000));
        assert_eq!(duration_attr(Some("PT0S")), Some(0));
        assert_eq!(duration_attr(None), None);
    }

    #[test]
    fn a_tidal_style_mpd() {
        let xml = r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" mediaPresentationDuration="PT4S">
          <Period><AdaptationSet mimeType="audio/mp4">
            <Representation id="EAC3_JOC" codecs="ec-3" bandwidth="769208" audioSamplingRate="48000">
              <SegmentTemplate timescale="48000" startNumber="1" initialization="init.mp4" media="seg-$Number$.m4s">
                <SegmentTimeline><S d="192000" r="0"/></SegmentTimeline>
              </SegmentTemplate>
            </Representation>
          </AdaptationSet></Period>
        </MPD>"#;
        let plan = Plan::parse(xml, "https://cdn.example.com/album/manifest.mpd", Want::Dolby).unwrap();
        assert_eq!(plan.init, "https://cdn.example.com/album/init.mp4");
        assert_eq!(plan.segments, vec!["https://cdn.example.com/album/seg-1.m4s"]);
        assert_eq!(plan.bandwidth, 769_208);
        assert_eq!(plan.duration_ms, 4_000);
    }

    #[test]
    fn the_flac_rendition_is_taken_when_asked() {
        // Tidal's shape: two AAC copies first, the FLAC one last, each with its own
        // SegmentTemplate.
        let xml = r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" mediaPresentationDuration="PT4M4.074S">
          <Period><AdaptationSet mimeType="audio/mp4">
            <Representation id="HEAACV1" codecs="mp4a.40.5" bandwidth="97044" audioSamplingRate="44100">
              <SegmentTemplate timescale="44100" startNumber="1" initialization="https://cdn.example.com/he-init.mp4" media="https://cdn.example.com/he-$Number$.mp4">
                <SegmentTimeline><S d="176128" r="60"/></SegmentTimeline>
              </SegmentTemplate>
            </Representation>
            <Representation id="AACLC" codecs="mp4a.40.2" bandwidth="321868" audioSamplingRate="44100">
              <SegmentTemplate timescale="44100" startNumber="1" initialization="https://cdn.example.com/lc-init.mp4" media="https://cdn.example.com/lc-$Number$.mp4">
                <SegmentTimeline><S d="176128" r="60"/></SegmentTimeline>
              </SegmentTemplate>
            </Representation>
            <Representation id="FLAC,44100,16" codecs="flac" bandwidth="973421" audioSamplingRate="44100">
              <SegmentTemplate timescale="44100" startNumber="1" initialization="https://cdn.example.com/flac-init.mp4" media="https://cdn.example.com/flac-$Number$.mp4">
                <SegmentTimeline><S d="176128" r="60"/><S d="19886"/></SegmentTimeline>
              </SegmentTemplate>
            </Representation>
          </AdaptationSet></Period>
        </MPD>"#;
        let plan = Plan::parse(xml, "https://cdn.example.com/m.mpd", Want::Flac).unwrap();
        assert_eq!(plan.init, "https://cdn.example.com/flac-init.mp4");
        // 61 repeats of the first span plus the closing one.
        assert_eq!(plan.segments.len(), 62);
        assert!(plan.segments[0].contains("flac-init") || plan.segments[0].contains("flac-1"));
        assert_eq!(plan.duration_ms, 244_074);
    }

    /// A minimal fMP4 init: a `dfLa` box with Tidal's shape — four padding bytes, then
    /// the STREAMINFO block (44100 Hz, stereo, 16-bit, 10 758 636 samples).
    fn tidal_shaped_init() -> Vec<u8> {
        let mut si = vec![0u8; 34];
        si[10] = 0x0A;
        si[11] = 0xC4;
        si[12] = 0x42;
        si[13] = 0xF0;
        si[14..18].copy_from_slice(&1_075_86_36u32.to_be_bytes());
        let mut init = Vec::new();
        init.extend_from_slice(&20u32.to_be_bytes());
        init.extend_from_slice(b"ftypisom\0\0\0\0isom");
        let dfla_len = 8 + 4 + 4 + 34; // header, the padding, the block header, STREAMINFO
        init.extend_from_slice(&(dfla_len as u32).to_be_bytes());
        init.extend_from_slice(b"dfLa");
        init.extend_from_slice(&[0; 4]);
        init.extend_from_slice(&[0x80, 0x00, 0x00, 0x22]);
        init.extend_from_slice(&si);
        init
    }

    #[test]
    fn streaminfo_is_read_from_a_tidal_shaped_init() {
        let mut si = vec![0u8; 34];
        si[10] = 0x0A;
        si[11] = 0xC4;
        si[12] = 0x42;
        si[13] = 0xF0;
        si[14..18].copy_from_slice(&1_075_86_36u32.to_be_bytes());
        let init = tidal_shaped_init();
        let read = streaminfo(&init).expect("STREAMINFO not found");
        assert_eq!(read.sample_rate, 44_100);
        assert_eq!(read.channels, 2);
        assert_eq!(read.bit_depth, 16);
        assert_eq!(read.samples, 1_075_86_36);
        assert_eq!(read.info, si);
    }

    #[test]
    fn a_reassembled_flac_stream_carries_the_metadata_then_the_frames() {
        let mut bytes = tidal_shaped_init();
        // One media fragment: a bare moof, then an mdat holding two "frames".
        bytes.extend_from_slice(&12u32.to_be_bytes());
        bytes.extend_from_slice(b"moof\0\0\0\0");
        bytes.extend_from_slice(&14u32.to_be_bytes());
        bytes.extend_from_slice(b"mdat\xff\xf8\x11\x22\xff\xf9");
        let (stream, si) = reassemble(&bytes).expect("reassemble failed");
        assert!(stream.starts_with(b"fLaC\x80\x00\x00\x22"));
        assert_eq!(&stream[42..], &[0xff, 0xf8, 0x11, 0x22, 0xff, 0xf9]);
        assert_eq!(si.sample_rate, 44_100);
    }

    #[test]
    fn mdat_walk() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&16u32.to_be_bytes());
        bytes.extend_from_slice(b"moov");
        bytes.extend_from_slice(&[0; 8]);
        bytes.extend_from_slice(&12u32.to_be_bytes());
        bytes.extend_from_slice(b"mdat");
        bytes.extend_from_slice(&[1, 2, 3, 4]);
        assert_eq!(mdat_payloads(&bytes), vec![&[1u8, 2, 3, 4][..]]);
    }

    #[test]
    fn the_decoded_wav_plays_through_the_players_decoder() {
        let pcm: Vec<i16> = (0..480).flat_map(|i| [i as i16 * 10, -(i as i16) * 10]).collect();
        let data = wav(&pcm, 48_000);
        assert_eq!(&data[0..4], b"RIFF");
        assert_eq!(&data[8..12], b"WAVE");
        assert_eq!(&data[12..16], b"fmt ");
        assert_eq!(&data[36..40], b"data");
        // Symphonia probe directly.
        use symphonia::core::io::MediaSourceStream;
        use symphonia::core::probe::Hint;
        let mss = MediaSourceStream::new(Box::new(std::io::Cursor::new(data.clone())), Default::default());
        let mut hint = Hint::new();
        hint.with_extension("wav");
        let probed = symphonia::default::get_probe().format(&hint, mss, &Default::default(), &Default::default());
        assert!(probed.is_ok(), "symphonia wav probe failed: {:?}", probed.err());
    }

    #[test]
    #[ignore]
    fn live_blinding_lights_atmos_decodes_to_playable_pcm() {
        // Reads the local hires source and fetches a real Tidal Atmos stream.
        // Never committed with secrets; run manually when debugging.
        let data = std::fs::read(directories::ProjectDirs::from("", "", "ytm-rs").unwrap().data_dir().join("app.ron")).unwrap();
        let text = String::from_utf8_lossy(&data);
        let base = text.split("\"hires_source\":").nth(1).and_then(|s| s.split('"').nth(1)).unwrap().replace("\\n", "\n").lines().next().unwrap().to_owned();
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let http = reqwest::Client::new();
            // Search + stream via the same endpoints the app uses.
            let mk = |q: &str| { let mut u = url::Url::parse(&base).unwrap(); { let mut p = u.path_segments_mut().unwrap(); p.pop_if_empty(); p.push("search"); } u.query_pairs_mut().append_pair("q", q).append_pair("quality", "LOSSLESS").append_pair("atmos", "auto"); u };
            let r = http.get(mk("Blinding Lights The Weeknd")).header("User-Agent", crate::hires::USER_AGENT).send().await.unwrap();
            let j: serde_json::Value = r.json().await.unwrap();
            let items = j["tracks"].as_array().or(j["results"].as_array()).cloned().unwrap_or_default();
            let hit = items.iter().find(|t| t["artist"].as_str().unwrap_or("").to_lowercase().contains("weeknd") && t["title"].as_str().unwrap_or("").eq_ignore_ascii_case("Blinding Lights")).unwrap();
            let id = hit["id"].as_str().unwrap();
            let mut su = url::Url::parse(&base).unwrap();
            { let mut p = su.path_segments_mut().unwrap(); p.pop_if_empty(); p.push("stream"); p.push(id); }
            su.query_pairs_mut().append_pair("quality", "LOSSLESS").append_pair("atmos", "auto");
            let sr = http.get(su).header("User-Agent", crate::hires::USER_AGENT).send().await.unwrap();
            let sj: serde_json::Value = sr.json().await.unwrap();
            assert!(sj["atmos"].as_bool().unwrap_or(false), "not atmos: {:?}", sj["codec"]);
            let mpd = sj["url"].as_str().unwrap();
            let audio = fetch(&http, mpd).await.unwrap();
            assert!(!audio.pcm.is_empty());
            let shared = crate::stream::Shared::from_bytes_with_format(audio.pcm, audio.duration_ms, None, crate::stream::Format::Pcm { rate: audio.sample_rate, channels: 2 }, None);
            let mut dec = crate::stream::decoder(shared, false).unwrap();
            assert!(dec.by_ref().take(4800).count() == 4800);
        });
    }
}