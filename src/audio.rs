//! Audio engine: one thread owns the output device and a rodio `Player`.
//!
//! The UI talks to it through `AudioHandle`; status flows back through `AudioStatus`
//! (lock-free reads for the progress bar) plus a `Finished` event for auto-advance.

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, OnceLock};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, unbounded};
use rodio::cpal::traits::{DeviceTrait, HostTrait};
use rodio::mixer::{Mixer, MixerSource};
use rodio::{DeviceSinkBuilder, MixerDeviceSink, Player};

use crate::stream::{self, Shared};

pub use crate::stream::TrackDecoder;

pub mod eq;
pub mod limiter;
pub mod tempo;

pub enum AudioCmd {
    /// A decoder is ready for playback generation `generation`; stale generations are dropped.
    /// A decoder started mid-song (`at` > 0) is the seekable kind.
    Start { generation: u64, decoder: TrackDecoder, shared: Arc<Shared>, at: Duration },
    /// A decoder positioned at `at`, replacing the current one; only the latest seek's lands.
    Replace { generation: u64, seek: u64, decoder: TrackDecoder, at: Duration, seekable: bool },
    /// Seamlessly upgrade the current stream to hi-res in place: same generation, same
    /// song, a better copy of it. Sent by the backend once an addon answers after
    /// playback already started from YouTube.
    Upgrade { generation: u64, decoder: TrackDecoder, shared: Arc<Shared>, at: Duration, seekable: bool },
    Pause,
    Resume,
    Stop,
    /// Another song is on its way: with crossfade on, fade this one out instead of
    /// stopping it, and fade the next one in when it starts.
    Handoff,
    Seek(Duration),
    /// A song was asked for: open the device now, while its stream is still on the way.
    Warm,
    /// Seek to where `pos` will have moved on to by the time the seek lands (`pos` was
    /// current at `at`): following someone else's playback (listen together).
    SeekLive { pos: Duration, at: std::time::Instant },
    Volume(f32),
    /// The output device failed (unplugged, headphones disconnected): sent by the
    /// device's error callback so the audio thread reopens one.
    DeviceLost,
    /// Play through this device (cpal's id) whenever it's connected; None follows the
    /// system default.
    SetDevice(Option<String>),
}

pub enum AudioEvent {
    Finished { generation: u64 },
    /// Close enough to the end that the next song should start, to crossfade into it.
    Ending { generation: u64 },
    /// The song's download failed, so playback stopped at `at` (or a seek to `at`
    /// couldn't be served). Reopening the song there resolves a fresh stream.
    Interrupted { generation: u64, at: Duration, failure: stream::DownloadFailure },
    Error(String),
}

static EPOCH: LazyLock<Instant> = LazyLock::new(Instant::now);

fn now_ms() -> u64 {
    EPOCH.elapsed().as_millis() as u64
}

#[derive(Default)]
pub struct AudioStatus {
    pub position_ms: AtomicU64,
    /// When `position_ms` was sampled (ms since EPOCH), for smooth extrapolation.
    sampled_at: AtomicU64,
    pub duration_ms: AtomicU64,
    /// Updates after a seek lands, including jumps too small to distinguish from clock drift.
    pub seek_revision: AtomicU64,
    pub playing: AtomicBool,
    /// Generation the UI most recently asked for; the engine ignores older decoders.
    pub wanted_gen: AtomicU64,
    /// Generation the engine last started, set once its length is in.
    started_gen: AtomicU64,
    /// The length its stream gave, 0 when it gave none (`duration_ms` then keeps the UI's).
    stream_ms: AtomicU64,
    /// f32 bits
    volume: AtomicU32,
    /// The `Normalize` level songs are brought to.
    normalize: AtomicU8,
    /// The limiter's on, for songs normalization turns up.
    limit: Arc<AtomicBool>,
    /// What normalization does to the playing song, in dB (f32 bits); NaN when it does
    /// nothing (off, or YouTube gave no loudness for it).
    norm_db: AtomicU32,
    /// The volume slider follows loudness (cubic) rather than amplitude.
    log_volume: AtomicBool,
    /// Smoothed output loudness 0..1 (f32 bits), for audio-reactive visuals.
    level: AtomicU32,
    /// How far the output device runs behind the position (its buffer), in ms.
    latency_ms: AtomicU32,
    /// Crossfade length in ms; 0 = off.
    crossfade_ms: AtomicU32,
    /// Paused, or on the way there: songs fade to silence, and back up when it's cleared
    /// (see `Meter`).
    hushed: AtomicBool,
    /// How far that fade has turned the playing song down, 0..1 (f32 bits), for a decoder
    /// put in meanwhile (a seek's) to carry on from.
    faded: AtomicU32,
    /// Counts the players pausing outright: a song coming back from that fades in from
    /// silence, whatever its fade had got to (a pause cut short by a stalled download, or
    /// by headphones unplugged).
    silenced: AtomicU32,
    /// Pausing and playing again are instant, with no fade (Settings › Sound).
    instant_pause: AtomicBool,
    /// The equalizer's settings, applied to everything on its way to the device.
    eq: Arc<eq::EqParams>,
    /// Playback speed, applied to each song as it plays.
    speed: Arc<tempo::Speed>,
    /// DJ-style handoff rather than a straight-line crossfade (see `Ramp`).
    automix: AtomicBool,
    /// Tempo estimate of the playing song in BPM (f32 bits); 0 until one is heard.
    bpm: AtomicU32,
    mix_generation: AtomicU64,
    mix_duration_ms: AtomicU32,
    mix_active: AtomicBool,
    outgoing_position_ms: AtomicU64,
    outgoing_sampled_at: AtomicU64,
    outgoing_playing: AtomicBool,
}

impl AudioStatus {
    pub fn speed(&self) -> f32 {
        self.speed.get()
    }

    pub fn position(&self) -> Duration {
        Duration::from_millis(self.position_ms.load(Ordering::Relaxed))
    }

    /// Position extrapolated to now while playing, so progress moves smoothly between
    /// the engine's 100 ms updates.
    pub fn position_smooth(&self) -> Duration {
        let pos = self.position_ms.load(Ordering::Relaxed);
        if !self.playing.load(Ordering::Relaxed) {
            return Duration::from_millis(pos);
        }
        let age = now_ms().saturating_sub(self.sampled_at.load(Ordering::Relaxed)).min(250);
        Duration::from_millis(pos + (age as f32 * self.speed()) as u64)
    }

    pub fn outgoing_position(&self) -> Duration {
        let pos = self.outgoing_position_ms.load(Ordering::Relaxed);
        let age = if self.outgoing_playing() { now_ms().saturating_sub(self.outgoing_sampled_at.load(Ordering::Relaxed)).min(250) } else { 0 };
        Duration::from_millis(pos + age)
    }

    pub fn outgoing_playing(&self) -> bool {
        self.outgoing_playing.load(Ordering::Relaxed)
    }

    fn sample_outgoing(&self, pos: Duration, playing: bool) {
        self.outgoing_position_ms.store(pos.as_millis() as u64, Ordering::Relaxed);
        self.outgoing_sampled_at.store(now_ms(), Ordering::Relaxed);
        self.outgoing_playing.store(playing, Ordering::Relaxed);
    }

    pub fn duration(&self) -> Option<Duration> {
        let d = self.duration_ms.load(Ordering::Relaxed);
        (d > 0).then(|| Duration::from_millis(d))
    }

    /// Whether the song asked for as `generation` is playing, not the one before it still
    /// going until the engine gets to the new one.
    pub fn started(&self, generation: u64) -> bool {
        self.started_gen.load(Ordering::Acquire) == generation && self.playing.load(Ordering::Relaxed)
    }

    /// The length the song asked for as `generation` has by its stream, once started.
    pub fn stream_duration(&self, generation: u64) -> Option<Duration> {
        // Read after `started`, so the length is that song's, not the one before.
        if !self.started(generation) {
            return None;
        }
        let d = self.stream_ms.load(Ordering::Relaxed);
        (d > 0).then(|| Duration::from_millis(d))
    }

    /// Automix's handoff window for a song of this length, in seconds: how much of the
    /// end is given to the next song coming in. 0 when Automix is off.
    pub fn mix_secs(&self, length: Option<Duration>) -> f32 {
        if !self.automix.load(Ordering::Relaxed) {
            return 0.0;
        }
        mix_ms(self, length) as f32 / 1000.0
    }

    pub fn mix_duration(&self, generation: u64) -> Option<f32> {
        (self.mix_generation.load(Ordering::Acquire) == generation)
            .then(|| self.mix_duration_ms.load(Ordering::Relaxed) as f32 / 1000.0)
    }

    pub fn mixing(&self) -> bool {
        self.mix_active.load(Ordering::Relaxed)
    }

    pub fn latency(&self) -> Duration {
        // On the song's clock: more of the song at a faster speed.
        Duration::from_millis(self.latency_ms.load(Ordering::Relaxed) as u64).mul_f32(self.speed())
    }

    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }

    fn set_position(&self, ms: u64) {
        self.position_ms.store(ms, Ordering::Relaxed);
        self.sampled_at.store(now_ms(), Ordering::Relaxed);
    }

    /// The players are pausing outright: they come back up from silence.
    fn silence(&self) {
        self.faded.store(1f32.to_bits(), Ordering::Relaxed);
        self.silenced.fetch_add(1, Ordering::Relaxed);
    }
}

/// Sample-counted envelopes; Automix exchanges the lead at 65% of the blend.
#[derive(Clone, Copy)]
enum Ramp {
    In { at: u32, len: u32 },
    /// Down from `from` (a fade-in may have been cut short) to silence, then the source ends.
    Out { from: f32, at: u32, len: u32 },
    MixIn { at: u32, len: u32 },
    /// Same `from` as `Out`: the song may still be easing in when it's mixed out.
    MixOut { from: f32, at: u32, len: u32 },
}

/// A smoothstep from 0 to 1, slow at both ends.
fn smoothstep(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

impl Ramp {
    fn gain(self) -> f32 {
        match self {
            Ramp::In { at, len } => at as f32 / len as f32,
            Ramp::Out { from, at, len } => from * (1.0 - at as f32 / len as f32),
            // The two Automix gains are the square roots of a mirrored pair, so their
            // powers sum to one: the level holds through the handoff instead of dipping.
            Ramp::MixIn { at, len } => mix_handoff(at as f32 / len as f32).sqrt(),
            Ramp::MixOut { from, at, len } => from * (1.0 - mix_handoff(at as f32 / len as f32)).sqrt(),
        }
    }
}

/// How far through an Automix handoff the songs are: nothing for its first 30% (the
/// outgoing song keeps the floor), then a smooth swap over the rest.
fn mix_handoff(x: f32) -> f32 {
    smoothstep((x - 0.3) / 0.7)
}

const AUTOMIX_FADE: u32 = 1 << 31;
const FX_MAX_FRAMES: usize = 48_000;

struct MixFx {
    ring: Vec<f32>,
    channels: usize,
    frame: usize,
    channel: usize,
    rate: f32,
    low: [f32; 8],
    body: [f32; 8],
    low_k: f32,
    body_k: f32,
    beat: f32,
    rhythmic: bool,
    bass: f32,
    treble: f32,
    wet: f32,
    flange: f32,
    tail: Option<(f32, bool)>,
    ticks: u64,
}

impl MixFx {
    fn new(rate: u32, channels: u16) -> Option<Self> {
        if channels > 8 {
            return None;
        }
        let frames = ((rate as usize * 3) / 4).clamp(2, FX_MAX_FRAMES);
        Some(Self {
            ring: vec![0.0; frames * channels as usize],
            channels: channels as usize,
            frame: 0,
            channel: 0,
            rate: rate as f32,
            low: [0.0; 8],
            body: [0.0; 8],
            low_k: 1.0 - (-std::f32::consts::TAU * 180.0 / rate as f32).exp(),
            body_k: 1.0 - (-std::f32::consts::TAU * 4000.0 / rate as f32).exp(),
            beat: 0.5 * rate as f32,
            rhythmic: false,
            bass: 1.0,
            treble: 1.0,
            wet: 0.0,
            flange: 0.0,
            tail: None,
            ticks: 0,
        })
    }

    fn begin(&mut self, bpm: f32) {
        self.rhythmic = (70.0..=180.0).contains(&bpm);
        self.beat = self.rate * if self.rhythmic { 60.0 / bpm } else { 0.5 };
        while self.beat > (self.ring.len() / self.channels - 1) as f32 {
            self.beat *= 0.5;
        }
        self.beat = self.beat.max(1.0);
        self.ticks = 0;
    }

    fn tap(&self, delay: f32) -> f32 {
        let frames = self.ring.len() / self.channels;
        let delay = delay.clamp(1.0, (frames - 1) as f32);
        let whole = delay as usize;
        let frac = delay - whole as f32;
        let a = (self.frame + frames - whole) % frames;
        let b = (a + frames - 1) % frames;
        self.ring[a * self.channels + self.channel] * (1.0 - frac) + self.ring[b * self.channels + self.channel] * frac
    }

    fn process(&mut self, input: f32, ramp: Option<Ramp>) -> f32 {
        let c = self.channel;
        self.low[c] += self.low_k * (input - self.low[c]);
        self.body[c] += self.body_k * (input - self.body[c]);
        let blend = match ramp {
            Some(Ramp::MixIn { at, len }) => Some((at as f32 / len as f32, false)),
            Some(Ramp::MixOut { at, len, .. }) => Some((at as f32 / len as f32, true)),
            Some(Ramp::Out { .. }) => self.tail,
            _ => None,
        };
        self.tail = blend;
        let mut result = input;
        let mut history = input;
        if let Some((x, outgoing)) = blend {
            if c == 0 && self.ticks % 32 == 0 {
                let handoff = mix_handoff(x);
                self.bass = if outgoing { 1.0 - 0.85 * smoothstep((x - 0.35) / 0.3) } else { 0.15 + 0.85 * smoothstep((x - 0.58) / 0.25) };
                self.treble = if outgoing { 1.0 - 0.5 * handoff } else { 0.7 + 0.3 * handoff };
                self.wet = if outgoing { 0.4 * smoothstep((x - 0.3) / 0.35) } else { 0.0 };
                // The comb sweeps over two beats, fast enough to hear as a flanger.
                let phase = self.ticks as f32 / (self.beat * 2.0) * std::f32::consts::TAU;
                self.flange = self.rate * (0.0008 + 0.0037 * (0.5 + 0.5 * phase.sin()));
            }
            result = input + (self.bass - 1.0) * self.low[c] + (self.treble - 1.0) * (input - self.body[c]);
            if outgoing {
                // Dry plus two swept combs at an uneven ratio, so the notch family is
                // denser and no frequency sits dead. The right channel's combs sit a
                // quarter of the sweep farther back, which moves the image as they pass.
                let spread = (c as f32) * 0.25 * self.flange;
                let flanged = (input + self.tap(self.flange + spread) + self.tap(self.flange * 0.618 + spread * 0.5)) / 3.0;
                let tail = if self.rhythmic {
                    // A half-beat repeat stutters the last bar under the flanged comb.
                    0.45 * self.tap(self.beat * 0.5) + 0.55 * flanged
                } else {
                    let reverb = (self.tap(self.rate * 0.071) + self.tap(self.rate * 0.113) + self.tap(self.rate * 0.173)) / 3.0;
                    0.4 * reverb + 0.6 * flanged
                };
                history = input * 0.7 + tail * 0.3;
                result = result * (1.0 - self.wet) + tail * self.wet;
            }
        }
        self.ring[self.frame * self.channels + c] = history;
        self.channel += 1;
        if self.channel == self.channels {
            self.channel = 0;
            self.frame = (self.frame + 1) % (self.ring.len() / self.channels);
            self.ticks += 1;
        }
        result
    }

    fn reset(&mut self) {
        self.ring.fill(0.0);
        self.low.fill(0.0);
        self.body.fill(0.0);
        self.frame = 0;
        self.channel = 0;
        self.tail = None;
        self.ticks = 0;
    }
}

/// Pass-through source that tracks loudness (RMS over ~23 ms windows, smoothed) and
/// applies crossfade ramps. `fade_out` is set by the audio thread to the fade-out
/// length in ms (0 = keep playing); a new value restarts it from the current gain.
/// On top of that, it fades to silence while `hushed`, and back up from wherever it got
/// to when that's cleared, so pausing and playing again quickly never jumps.
struct Meter<S> {
    inner: S,
    status: Arc<AudioStatus>,
    fade_out: Arc<AtomicU32>,
    seen_out: u32,
    ramp: Option<Ramp>,
    /// The pause fade's gain, how far it moves each sample (with the fade on, and off),
    /// and `silenced` as last seen.
    pause: f32,
    pause_step: f32,
    instant_step: f32,
    seen_silenced: u32,
    /// Automix's handoff curves rather than the straight-line fade.
    mix: bool,
    acc: f32,
    n: u32,
    smooth: f32,
    tempo: Tempo,
    analysis: Arc<OnceLock<Option<f32>>>,
    analysis_acc: f32,
    fx: Option<MixFx>,
}

/// How many windows Automix reads for a beat before it settles for the plain length.
const TEMPO_PASSES: u8 = 4;

/// Rough tempo tracking for Automix's phrase length: the spacing that repeats most in
/// the music's loudness impulses over the last few seconds.
struct Tempo {
    prev: f32,
    hist: [f32; 300],
    count: usize,
    /// The song's tempo is settled; nothing more to analyse in it.
    done: bool,
    /// How many windows have been read without a beat in them.
    passes: u8,
    /// How many ms one loudness reading covers.
    hop_ms: f32,
}

impl Tempo {
    fn new(hop_ms: f32) -> Self {
        Self { prev: 0.0, hist: [0.0; 300], count: 0, done: false, passes: 0, hop_ms }
    }

    // Bounded: a few windows of roughly six seconds each, and it stops the moment one of
    // them answers, whether or not a reliable beat was found in it.
    fn push(&mut self, e: f32) -> Option<f32> {
        if self.done {
            return None;
        }
        let flux = (e - self.prev).max(0.0);
        self.prev = e;
        self.hist[self.count] = flux;
        self.count += 1;
        if self.count < self.hist.len() {
            return None;
        }
        if let Some(bpm) = self.estimate() {
            self.done = true;
            return Some(bpm);
        }
        // An intro that says nothing yet is not a song without a beat: try the next
        // window, and stop while the mix still has time to use the answer.
        self.passes += 1;
        if self.passes >= TEMPO_PASSES {
            self.done = true;
        } else {
            self.hist.fill(0.0);
            self.count = 0;
        }
        None
    }

    fn estimate(&self) -> Option<f32> {
        let n = self.hist.len();
        let mean = self.hist.iter().sum::<f32>() / n as f32;
        let power = self.hist.iter().map(|e| (e - mean).powi(2)).sum::<f32>() / n as f32;
        if power <= 1e-12 {
            return None;
        }
        // Beats of 0.30–1.20 s, with the middle of that range preferred so half- and
        // double-time patterns land on the tempo people would tap.
        let lo = ((0.30 * 1000.0 / self.hop_ms) as usize).max(4);
        let hi = ((1.20 * 1000.0 / self.hop_ms) as usize).min(n / 2);
        let mut best = (0.0f32, 0usize);
        for lag in lo..=hi {
            let c: f32 = (lag..n).map(|i| (self.hist[i] - mean) * (self.hist[i - lag] - mean)).sum::<f32>() / (n - lag) as f32;
            let secs = lag as f32 * self.hop_ms / 1000.0;
            let prior = (-(secs / 0.5).ln().powi(2) / (2.0 * 0.4 * 0.4)).exp();
            let score = c / power * prior;
            if score > best.0 {
                best = (score, lag);
            }
        }
        if best.0 < 0.08 {
            return None;
        }
        let mut bpm = 60_000.0 / (best.1 as f32 * self.hop_ms);
        while bpm < 70.0 {
            bpm *= 2.0;
        }
        while bpm > 180.0 {
            bpm /= 2.0;
        }
        Some(bpm)
    }
}

impl<S: rodio::Source> Meter<S> {
    fn new(inner: S, status: Arc<AudioStatus>, fade_in_ms: u32, fade_out: Arc<AtomicU32>, mix: bool, analysis: Arc<OnceLock<Option<f32>>>) -> Self {
        let hop_ms = 2048.0 / (inner.sample_rate().get() as f32 * inner.channels().get() as f32) * 1000.0;
        let mut tempo = Tempo::new(hop_ms);
        tempo.done = analysis.get().is_some();
        let mut fx = if status.automix.load(Ordering::Relaxed) { MixFx::new(inner.sample_rate().get(), inner.channels().get()) } else { None };
        if let Some(fx) = &mut fx {
            fx.begin(analysis.get().copied().flatten().unwrap_or(0.0));
        }
        // A decoder put in mid-fade (a seek's) carries on from there; while paused, from silence.
        let pause = 1.0 - f32::from_bits(status.faded.load(Ordering::Relaxed));
        let seen_silenced = status.silenced.load(Ordering::Relaxed);
        let mut m = Self { inner, status, fade_out, seen_out: 0, ramp: None, pause, pause_step: 0.0, instant_step: 0.0, seen_silenced, mix, acc: 0.0, n: 0, smooth: 0.0, tempo, analysis, analysis_acc: 0.0, fx };
        m.pause_step = 1.0 / m.samples(PAUSE_FADE_MS) as f32;
        m.instant_step = 1.0 / m.samples(INSTANT_FADE_MS) as f32;
        if fade_in_ms > 0 {
            m.ramp = Some(if mix { Ramp::MixIn { at: 0, len: m.samples(fade_in_ms) } } else { Ramp::In { at: 0, len: m.samples(fade_in_ms) } });
        }
        m
    }

    fn samples(&self, ms: u32) -> u32 {
        let per_sec = self.inner.sample_rate().get() as u64 * self.inner.channels().get() as u64;
        (per_sec * ms as u64 / 1000).max(1) as u32
    }
}

impl<S: rodio::Source> Iterator for Meter<S> {
    type Item = rodio::Sample;
    fn next(&mut self) -> Option<Self::Item> {
        let out = self.fade_out.load(Ordering::Relaxed);
        if out != self.seen_out {
            self.seen_out = out;
            let from = self.ramp.map_or(1.0, Ramp::gain);
            let len = self.samples(out & !AUTOMIX_FADE);
            self.mix = out & AUTOMIX_FADE != 0;
            if self.mix && let Some(fx) = &mut self.fx {
                fx.begin(self.analysis.get().copied().flatten().unwrap_or(0.0));
            }
            self.ramp = Some(if self.mix { Ramp::MixOut { from, at: 0, len } } else { Ramp::Out { from, at: 0, len } });
        }
        if let Some(Ramp::Out { at, len, .. } | Ramp::MixOut { at, len, .. }) = self.ramp
            && at >= len
        {
            return None;
        }
        let mut s = self.inner.next()?;
        if !self.tempo.done {
            self.analysis_acc += s * s;
        }
        if let Some(fx) = &mut self.fx {
            s = fx.process(s, self.ramp);
        }
        if let Some(ramp) = &mut self.ramp {
            s *= ramp.gain();
            match ramp {
                Ramp::In { at, len } | Ramp::MixIn { at, len } => {
                    *at += 1;
                    if *at >= *len {
                        self.ramp = None;
                    }
                }
                Ramp::Out { at, .. } | Ramp::MixOut { at, .. } => *at += 1,
            }
        }
        let silenced = self.status.silenced.load(Ordering::Relaxed);
        if silenced != self.seen_silenced {
            self.seen_silenced = silenced;
            self.pause = 0.0;
        }
        let target = if self.status.hushed.load(Ordering::Relaxed) { 0.0 } else { 1.0 };
        if self.pause != target {
            let step = if self.status.instant_pause.load(Ordering::Relaxed) { self.instant_step } else { self.pause_step };
            self.pause = if target > self.pause { (self.pause + step).min(1.0) } else { (self.pause - step).max(0.0) };
            // Not a song fading out: the playing one is what a new decoder follows.
            if self.seen_out == 0 {
                self.status.faded.store((1.0 - self.pause).to_bits(), Ordering::Relaxed);
            }
        }
        s *= self.pause;
        self.acc += s * s;
        self.n += 1;
        if self.n == 2048 {
            let rms = (self.acc / 2048.0).sqrt();
            // Fast attack, slow release, like a VU meter.
            let k = if rms > self.smooth { 0.6 } else { 0.08 };
            self.smooth += (rms - self.smooth) * k;
            // A song fading out is no longer the one the visuals follow.
            if self.seen_out == 0 {
                self.status.level.store(self.smooth.min(1.0).to_bits(), Ordering::Relaxed);
                if self.status.automix.load(Ordering::Relaxed) && !self.tempo.done {
                    let bpm = self.tempo.push((self.analysis_acc / 2048.0).sqrt());
                    if self.tempo.done {
                        let _ = self.analysis.set(bpm);
                        self.status.bpm.store(bpm.unwrap_or(0.0).to_bits(), Ordering::Relaxed);
                        log::debug!("audio: tempo analysis complete: {bpm:?}");
                    }
                }
            }
            self.acc = 0.0;
            self.analysis_acc = 0.0;
            self.n = 0;
        }
        Some(s)
    }
}

impl<S: rodio::Source> rodio::Source for Meter<S> {
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }
    fn channels(&self) -> rodio::ChannelCount {
        self.inner.channels()
    }
    fn sample_rate(&self) -> rodio::SampleRate {
        self.inner.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
    fn try_seek(&mut self, pos: Duration) -> Result<(), rodio::source::SeekError> {
        self.inner.try_seek(pos)?;
        self.ramp = None;
        self.acc = 0.0;
        self.analysis_acc = 0.0;
        self.n = 0;
        if let Some(fx) = &mut self.fx {
            fx.reset();
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct AudioHandle {
    tx: Sender<AudioCmd>,
    pub status: Arc<AudioStatus>,
}

impl AudioHandle {
    pub fn send(&self, cmd: AudioCmd) {
        let _ = self.tx.send(cmd);
    }

    pub fn volume(&self) -> f32 {
        f32::from_bits(self.status.volume.load(Ordering::Relaxed))
    }

    pub fn set_normalize(&self, level: Normalize) {
        self.status.normalize.store(level as u8, Ordering::Relaxed);
        self.status.limit.store(level != Normalize::Off, Ordering::Relaxed);
        self.send(AudioCmd::Volume(self.volume()));
    }

    /// How far normalization turns the playing song up (positive) or down, in dB; None
    /// when it leaves it as it is (off, or there's no loudness figure for it).
    pub fn normalization(&self) -> Option<f32> {
        Some(f32::from_bits(self.status.norm_db.load(Ordering::Relaxed))).filter(|d| !d.is_nan())
    }

    /// Switches the slider's curve, moving it so the music stays as loud as it was.
    pub fn set_log_volume(&self, on: bool) {
        if self.status.log_volume.swap(on, Ordering::Relaxed) != on {
            let v = self.volume();
            self.status.volume.store(if on { v.cbrt() } else { v.powi(3) }.to_bits(), Ordering::Relaxed);
        }
        self.send(AudioCmd::Volume(self.volume()));
    }

    pub fn set_crossfade(&self, secs: u32) {
        self.status.crossfade_ms.store(secs * 1000, Ordering::Relaxed);
    }

    /// Whether pausing fades out and playing again fades back in; off, both are instant.
    pub fn set_pause_fade(&self, on: bool) {
        self.status.instant_pause.store(!on, Ordering::Relaxed);
        // Wakes the audio thread: a pause fading out now finishes quickly.
        self.send(AudioCmd::Volume(self.volume()));
    }

    /// Takes effect at once, gliding there. `no_clip` turns everything down by the
    /// curve's highest boost, leaving room for it in loud songs.
    pub fn set_eq(&self, on: bool, gains: &eq::Gains, no_clip: bool) {
        self.status.eq.set(on, gains, no_clip);
    }

    /// Takes effect at once; the pitch goes with it.
    pub fn set_speed(&self, speed: f32) {
        self.status.speed.set(speed);
    }

    /// Automix's handoff shape for the crossfade (see `Ramp::MixIn`).
    pub fn set_automix(&self, on: bool) {
        self.status.automix.store(on, Ordering::Relaxed);
    }

    pub fn set_volume(&self, v: f32) {
        self.status.volume.store(v.to_bits(), Ordering::Relaxed);
        self.send(AudioCmd::Volume(v));
    }
}

/// A seek that needs a new decoder built over the stream buffer (off the audio thread).
struct SeekJob {
    shared: Arc<Shared>,
    pos: Duration,
    /// For a live seek (a jam): the host position and when it was, to catch up after the build.
    live: Option<(Duration, std::time::Instant)>,
    generation: u64,
    seek: u64,
}

/// Build a decoder for `j` (it may wait for the download) and hand it to the audio thread.
/// `pending`: the seek asked for since, if any.
fn build_seek(mut j: SeekJob, pending: &std::sync::Mutex<Option<SeekJob>>, tx: &Sender<AudioCmd>, events: &Sender<AudioEvent>) {
    let seekable = !j.pos.is_zero();
    let decoder = stream::decoder(j.shared.clone(), seekable);
    // That may have waited a long time (the download, the index). A seek in the same song
    // asked for meanwhile takes this decoder, which can go anywhere, rather than have it
    // dropped as stale and another built.
    if seekable && decoder.is_ok() {
        let mut next = pending.lock().unwrap();
        if next.as_ref().is_some_and(|n| n.generation == j.generation && !n.pos.is_zero()) {
            j = next.take().unwrap();
        }
    }
    let SeekJob { shared, pos, live, generation, seek } = j;
    let res = decoder.map_err(|e| e.to_string()).and_then(|mut d| {
        // Building it may have waited for the download: catch up.
        let pos = live.map_or(pos, |(p, at)| p + at.elapsed());
        if seekable {
            rodio::Source::try_seek(&mut d, pos).map_err(|e| e.to_string())?;
        }
        Ok((d, pos))
    });
    match res {
        Ok((decoder, pos)) => {
            let _ = tx.send(AudioCmd::Replace { generation, seek, decoder, at: pos, seekable });
        }
        Err(e) => match shared.failure() {
            Some(failure) => {
                let _ = events.send(AudioEvent::Interrupted { generation, at: pos, failure });
            }
            None => {
                let _ = events.send(AudioEvent::Error(format!("Seek failed: {e}")));
            }
        },
    }
}

/// A song fading out under the next one, kept until it's silent.
struct Fading {
    player: Player,
    /// Where the fading song is up to, on its own clock (see `Tempo`).
    song_ms: Arc<AtomicU64>,
    shared: Option<Arc<Shared>>,
    out: Arc<AtomicU32>,
    /// Still at full volume, waiting for the next song to start (as Morphe does): when
    /// that was asked for, and how much of this song was left then.
    waiting: Option<(Instant, Option<Duration>)>,
    /// The song ended on its own rather than being skipped: this handoff gets the whole
    /// musical phrase.
    natural: bool,
}

impl Fading {
    /// Start the fade-out, over what's left of the song if that's shorter. Returns its length.
    fn start(&mut self, crossfade: u32, automix: bool) -> u32 {
        let left = self.waiting.take().and_then(|(at, left)| Some(left?.saturating_sub(at.elapsed())));
        let len = crossfade.min(left.map_or(u32::MAX, |l| l.as_millis() as u32)).max(QUICK_FADE_MS);
        self.out.store(len | if automix { AUTOMIX_FADE } else { 0 }, Ordering::Relaxed);
        len
    }
}

impl Drop for Fading {
    fn drop(&mut self) {
        if let Some(s) = &self.shared {
            s.cancel();
        }
    }
}

/// Fade-outs shorter than this would click: a crossfade cut short by another skip, or
/// one too close to the end of the song.
const QUICK_FADE_MS: u32 = 40;

/// How long pausing fades out, and resuming fades back in, like foobar2000 and Spotify:
/// short enough not to hold anything up, long enough not to sound cut off.
const PAUSE_FADE_MS: u32 = 200;

/// With the fade off (Settings › Sound), pausing and playing again still ramp, this
/// briefly: too short to hear, but cutting off at full volume would click.
const INSTANT_FADE_MS: u32 = 5;

/// How long after a pause the players really pause: once the fade has gone through the
/// device's buffer.
const PAUSE_AFTER: Duration = Duration::from_millis(PAUSE_FADE_MS as u64 + 60);
/// The same with the fade off.
const INSTANT_PAUSE_AFTER: Duration = Duration::from_millis(INSTANT_FADE_MS as u64 + 60);

/// When a pause due at `pause_at` really pauses the players: with the fade turned off
/// on the way (`instant`), once the rest of it has gone the quick way.
fn pause_due(pause_at: Option<Instant>, instant: bool, now: Instant) -> Option<Instant> {
    match instant {
        true => pause_at.map(|at| at.min(now + INSTANT_PAUSE_AFTER)),
        false => pause_at,
    }
}

/// How long a song keeps playing at full volume for the next one to load before it
/// fades out anyway.
const HANDOFF_WAIT: Duration = Duration::from_secs(3);

/// Automix counts its mix in bars of four beats: eight bars normally, sixteen on a song
/// long enough to hold them, and never more than 32 — nor longer than this, or than a
/// third of the song.
const AUTOMIX_BARS: f32 = 8.0;
const AUTOMIX_BARS_LONG: f32 = 16.0;
const AUTOMIX_MAX_BARS: f32 = 32.0;
const AUTOMIX_MAX_MS: u32 = 45_000;
const AUTOMIX_MIN_MS: u32 = 4_000;

/// How long the mix runs, in ms. Automix works it out from the bars and the tempo it
/// hears in the outgoing song; a plain crossfade uses its setting.
fn mix_ms(status: &AudioStatus, length: Option<Duration>) -> u32 {
    let crossfade = status.crossfade_ms.load(Ordering::Relaxed);
    if !status.automix.load(Ordering::Relaxed) {
        return crossfade;
    }
    let bpm = f32::from_bits(status.bpm.load(Ordering::Relaxed));
    if !(40.0..=240.0).contains(&bpm) {
        // Nothing to count bars with yet: the crossfade length stands in.
        return crossfade;
    }
    let bars = if length.is_some_and(|d| d >= Duration::from_secs(240)) { AUTOMIX_BARS_LONG } else { AUTOMIX_BARS };
    let ms = (bars.min(AUTOMIX_MAX_BARS) * 4.0 * 60_000.0 / bpm) as u32;
    let ms = ms.clamp(AUTOMIX_MIN_MS, AUTOMIX_MAX_MS);
    // A short song saves room for its own mix: a third of it at most.
    length.map_or(ms, |d| ms.min((d.as_millis() as u32 / 3).max(QUICK_FADE_MS)))
}

/// Gain for a volume slider position. Ears hear amplitude roughly logarithmically,
/// so on a linear slider nearly all the change is in its bottom few pixels; the cubic
/// curve (PulseAudio's) spreads it out: half way is -18 dB.
fn slider_gain(status: &AudioStatus, v: f32) -> f32 {
    if status.log_volume.load(Ordering::Relaxed) { v.powi(3) } else { v }
}

/// The level volume normalization brings songs to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Normalize {
    Off,
    /// YouTube's (and Spotify's) -14 LUFS.
    #[default]
    Normal,
    /// -19 LUFS, like Spotify's Quiet: more songs line up without being turned up.
    Quiet,
}

impl Normalize {
    pub const ALL: [Self; 3] = [Self::Off, Self::Normal, Self::Quiet];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Normal => "Normal",
            Self::Quiet => "Quiet",
        }
    }

    /// As saved: "1" and "0" are what the on/off switch saved before.
    pub fn key(self) -> &'static str {
        match self {
            Self::Off => "0",
            Self::Normal => "1",
            Self::Quiet => "quiet",
        }
    }

    pub fn from_key(key: &str) -> Self {
        Self::ALL.into_iter().find(|l| l.key() == key).unwrap_or_default()
    }

    fn from_u8(v: u8) -> Self {
        Self::ALL.get(v as usize).copied().unwrap_or(Self::Off)
    }

    /// The level, in dB from YouTube's -14 LUFS target.
    fn offset_db(self) -> f32 {
        if self == Self::Quiet { -5.0 } else { 0.0 }
    }
}

/// The most normalization turns a quiet song up. Past this the limiter would be holding
/// it down all the time, which flattens it.
const MAX_BOOST_DB: f32 = 6.0;

/// What volume normalization does to a song, in dB: brings it from the loudness YouTube
/// measured to the level picked, turning loud songs down and quiet ones up (up to
/// `MAX_BOOST_DB`; the limiter keeps their peaks from clipping). None when it leaves it
/// as it is: off, or no loudness figure (yt-dlp's streams, podcast feeds).
fn norm_db(status: &AudioStatus, shared: Option<&Shared>) -> Option<f32> {
    let level = Normalize::from_u8(status.normalize.load(Ordering::Relaxed));
    if level == Normalize::Off {
        return None;
    }
    Some((level.offset_db() - shared?.loudness_db?).min(MAX_BOOST_DB))
}

fn norm_gain(status: &AudioStatus, shared: Option<&Shared>) -> f32 {
    norm_db(status, shared).map_or(1.0, |db| 10f32.powf(db / 20.0))
}

/// Keeps what normalization does to the playing song, for the UI to show.
fn show_norm(status: &AudioStatus, shared: Option<&Shared>) {
    status.norm_db.store(norm_db(status, shared).unwrap_or(f32::NAN).to_bits(), Ordering::Relaxed);
}

/// The output device. Players play into a mixer of our own that the device pulls
/// from, so when the device goes away (Bluetooth headphones disconnecting) or the
/// default changes (them reconnecting), a new one carries on from the same samples
/// without touching the players or the songs in them.
struct Output {
    mixer: Mixer,
    /// Our mixer's output while no device has it; the device's `Bridge` holds it.
    slot: Arc<std::sync::Mutex<Option<MixerSource>>>,
    sink: Option<MixerDeviceSink>,
    /// The device in use (cpal's id), to notice the default changing.
    device: Option<String>,
    /// The device the user picked; the default stands in while it's not connected.
    preferred: Option<String>,
    /// Set by the device's error callback.
    lost: Arc<AtomicBool>,
    checked: Instant,
}

impl Output {
    /// Doesn't open the device: the first song does. Also whether there's one to open.
    fn new(preferred: Option<String>) -> (Self, bool) {
        // Mix at the device's own format, so the usual case needs no conversion.
        let device = target(preferred.as_deref());
        let found = device.is_some();
        let config = device.and_then(|d| d.default_output_config().ok());
        let channels = config.as_ref().and_then(|c| std::num::NonZero::new(c.channels())).unwrap_or(std::num::NonZero::new(2).unwrap());
        let rate = config.as_ref().and_then(|c| std::num::NonZero::new(c.sample_rate())).unwrap_or(std::num::NonZero::new(48_000).unwrap());
        let (mixer, source) = rodio::mixer::mixer(channels, rate);
        let lost = Arc::new(AtomicBool::new(false));
        (Self { mixer, slot: Arc::new(std::sync::Mutex::new(Some(source))), sink: None, device: None, preferred, lost, checked: Instant::now() }, found)
    }

    /// Opens the picked device, or the default one. False if there's none to open.
    fn open(&mut self, tx: Sender<AudioCmd>, status: &AudioStatus) -> bool {
        // The old device's stream gives our mixer back when it's dropped.
        self.sink = None;
        self.device = None;
        self.checked = Instant::now();
        let Some(device) = target(self.preferred.as_deref()) else {
            return false;
        };
        let id = device.id().map(|i| i.to_string()).ok();
        let lost = self.lost.clone();
        lost.store(false, Ordering::Relaxed);
        let on_error = move |e: rodio::cpal::StreamError| {
            log::warn!("audio: output device error: {e}");
            if !lost.swap(true, Ordering::Relaxed) {
                let _ = tx.send(AudioCmd::DeviceLost);
            }
        };
        let sink = match DeviceSinkBuilder::from_device(device).and_then(|b| b.with_error_callback(on_error).open_sink_or_fallback()) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("audio: couldn't open the output device: {e}");
                return false;
            }
        };
        // A stream that failed on its own hands the mixer back as its thread exits,
        // which can be just after it reported the error.
        let waited = Instant::now();
        let source = loop {
            if let Some(source) = self.slot.lock().unwrap().take() {
                break source;
            }
            if waited.elapsed() > Duration::from_secs(1) {
                log::warn!("audio: mixer still held by the old device");
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        let mut sink = sink;
        sink.log_on_drop(false);
        let channels = rodio::Source::channels(&source).get();
        let rate = rodio::Source::sample_rate(&source).get();
        // Here, after the mixer, so one equalizer covers both songs in a crossfade and
        // a new song or a seek doesn't restart it.
        let eq = eq::Eq::new(status.eq.clone(), rate, channels);
        let limiter = limiter::Limiter::new(status.limit.clone(), rate, channels);
        sink.mixer().add(Bridge { inner: Some(source), slot: self.slot.clone(), channels, n: 0, eq, limiter });
        // The position counts samples handed to the device; it plays them a buffer later
        // (rodio asks for ~50 ms), plus the shared-mode device period (~10 ms) and the
        // limiter's lookahead.
        let rate = sink.config().sample_rate().get().max(1);
        let buffer_ms = match sink.config().buffer_size() {
            rodio::cpal::BufferSize::Fixed(n) => *n as u64 * 1000 / rate as u64,
            rodio::cpal::BufferSize::Default => 0,
        };
        status.latency_ms.store((buffer_ms + 10) as u32 + limiter::LOOKAHEAD_MS, Ordering::Relaxed);
        log::info!("audio: output device {:?} at {} Hz", id, rate);
        self.sink = Some(sink);
        self.device = id;
        true
    }

    /// Whether the device to play through (the picked one if it's connected, else the
    /// default) differs from the one in use.
    fn default_changed(&mut self) -> bool {
        self.checked = Instant::now();
        let want = target(self.preferred.as_deref()).and_then(|d| d.id().ok()).map(|i| i.to_string());
        want.is_some() && want != self.device
    }
}

/// Empties the player without waiting on the device. rodio's `clear` waits for the device
/// to skip what's in it, which never happens while the device is let go (a song paused that
/// long), nor while a device that's gone still looks open (until `DeviceLost` arrives): a new
/// player instead, at the same volume and paused, as `clear` leaves it. The old one's song
/// ends the next time the mixer is pulled.
fn clear(player: &mut Player, output: &Output) {
    let fresh = Player::connect_new(&output.mixer);
    fresh.set_volume(player.volume());
    fresh.pause();
    *player = fresh;
}

/// The picked device if it's connected, else the system default.
fn target(preferred: Option<&str>) -> Option<rodio::cpal::Device> {
    let host = rodio::cpal::default_host();
    if let Some(want) = preferred
        && let Some(d) = host.output_devices().ok().and_then(|mut all| all.find(|d| d.id().is_ok_and(|i| i.to_string() == want)))
    {
        return Some(d);
    }
    host.default_output_device()
}

/// An output device to offer in settings.
#[derive(Clone, PartialEq)]
pub struct OutputDevice {
    /// cpal's id: stable across reconnects.
    pub id: String,
    pub name: String,
}

/// The connected output devices, by the names Windows' sound settings show.
pub fn output_devices() -> Vec<OutputDevice> {
    let Ok(all) = rodio::cpal::default_host().output_devices() else { return Vec::new() };
    let mut out: Vec<OutputDevice> = all
        .filter_map(|d| {
            let id = d.id().ok()?.to_string();
            let desc = d.description().ok()?;
            // "Speakers (Realtek(R) Audio)" rather than just "Speakers".
            let name = desc.extended().first().cloned().unwrap_or_else(|| desc.name().to_owned());
            Some(OutputDevice { id, name })
        })
        .collect();
    out.sort_by_key(|a| a.name.to_lowercase());
    out
}

/// Our mixer's output, as a source in the device's mixer. Never ends, so the device
/// keeps it; gives the mixer back when the device's stream drops it.
struct Bridge {
    inner: Option<MixerSource>,
    slot: Arc<std::sync::Mutex<Option<MixerSource>>>,
    channels: u16,
    /// Samples handed out, to give the mixer back on a frame boundary (else the next
    /// device would get the channels swapped).
    n: u64,
    /// The equalizer, shaping every sample on its way to the device.
    eq: eq::Eq,
    /// Last, after the volume, so it only acts on what would really clip.
    limiter: limiter::Limiter,
}

impl Iterator for Bridge {
    type Item = rodio::Sample;
    fn next(&mut self) -> Option<Self::Item> {
        let channel = (self.n % self.channels.max(1) as u64) as usize;
        self.n += 1;
        let s = self.inner.as_mut().and_then(Iterator::next).unwrap_or(0.0);
        Some(self.limiter.process(self.eq.process(s, channel), channel))
    }
}

impl rodio::Source for Bridge {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> rodio::ChannelCount {
        self.inner.as_ref().map_or(std::num::NonZero::new(self.channels.max(1)).unwrap(), |s| s.channels())
    }
    fn sample_rate(&self) -> rodio::SampleRate {
        self.inner.as_ref().map_or(std::num::NonZero::new(48_000).unwrap(), |s| s.sample_rate())
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        if let Some(mut source) = self.inner.take() {
            while !self.n.is_multiple_of(self.channels.max(1) as u64) {
                source.next();
                self.n += 1;
            }
            *self.slot.lock().unwrap() = Some(source);
        }
    }
}

/// How often to look for a new default device while playing.
const DEVICE_CHECK: Duration = Duration::from_secs(1);

pub fn spawn(volume: f32, log_volume: bool, device: Option<String>, events: Sender<AudioEvent>, wake: impl Fn() + Send + 'static) -> AudioHandle {
    let (tx, rx) = unbounded();
    let status = Arc::new(AudioStatus::default());
    status.volume.store(volume.to_bits(), Ordering::Relaxed);
    status.log_volume.store(log_volume, Ordering::Relaxed);
    let st = status.clone();
    std::thread::Builder::new()
        .name("audio".into())
        .spawn({
            let tx = tx.clone();
            move || run(rx, tx, st, volume, device, events, wake)
        })
        .expect("spawn audio thread");
    AudioHandle { tx, status }
}

fn run(
    rx: Receiver<AudioCmd>,
    tx: Sender<AudioCmd>,
    status: Arc<AudioStatus>,
    volume: f32,
    device: Option<String>,
    events: Sender<AudioEvent>,
    wake: impl Fn(),
) {
    // An open device costs CPU even while silent, and keeps Windows awake: it waits for
    // the first song.
    let (mut output, found) = Output::new(device);
    if !found {
        let _ = events.send(AudioEvent::Error("No audio output device".into()));
        wake();
    }
    let mut player = Player::connect_new(&output.mixer);
    player.set_volume(slider_gain(&status, volume));
    // The playing source's fade-out control (see `Meter`).
    let mut fade_out = Arc::new(AtomicU32::new(0));
    // Songs fading out under the current one (more than one only for a moment).
    let mut fading: Vec<Fading> = Vec::new();
    // Whether `Ending` went out for this generation.
    let mut ending_sent = false;
    // The playing song's length. Not `status.duration()` at a handoff: the UI has
    // already put the next song's there.
    let mut length: Option<Duration> = None;

    // Generation of the source currently in the player (0 = nothing loaded).
    let mut current: u64 = 0;
    let mut shared: Option<Arc<Shared>> = None;
    // Where the playing song is up to, on its own clock (see `Tempo`).
    let mut song_ms = Arc::new(AtomicU64::new(0));
    let song_pos = |ms: &AtomicU64| Duration::from_millis(ms.load(Ordering::Relaxed));
    let tempo = |decoder, from, ms: &Arc<AtomicU64>| tempo::Tempo::new(decoder, status.speed.clone(), from, ms.clone());
    // Whether the playing decoder can seek in place (it indexed the whole file).
    let mut seekable = false;
    // Latest seek; a slower, older seek's decoder must not land after a newer one.
    let mut seek_seq: u64 = 0;
    let seek_job: Arc<std::sync::Mutex<Option<SeekJob>>> = Default::default();
    let seek_busy = Arc::new(std::sync::atomic::AtomicBool::new(false));
    // Dev hook: YTM_DEVICE_LOST=<secs> acts as if the output device disconnected that
    // long into the first song (the error callback firing for a device that's gone).
    let mut dev_lost = std::env::var("YTM_DEVICE_LOST").ok().and_then(|v| v.parse::<f32>().ok()).map(Duration::from_secs_f32);
    // When a pause fading out really pauses the players.
    let mut pause_at: Option<Instant> = None;
    loop {
        // Poll while playing so the position stays fresh and the end is noticed; idle otherwise.
        let timeout = if (current != 0 || !fading.is_empty()) && !player.is_paused() { Duration::from_millis(100) } else { Duration::from_secs(3600) };
        let timeout = pause_at.map_or(timeout, |at| timeout.min(at.saturating_duration_since(Instant::now())));
        match rx.recv_timeout(timeout) {
            Ok(AudioCmd::Start { generation, decoder, shared: s, at }) => {
                if generation != status.wanted_gen.load(Ordering::Relaxed) {
                    log::debug!("audio: dropping stale generation {generation}");
                    continue;
                }
                log::debug!("audio: start generation {generation}");
                if (output.sink.is_none() || output.default_changed()) && !output.open(tx.clone(), &status) {
                    let _ = events.send(AudioEvent::Error("No audio output device".into()));
                    wake();
                }
                clear(&mut player, &output);
                // A song asked for while paused plays. What was fading out under the paused
                // one goes, rather than come back up.
                pause_at = None;
                if status.hushed.swap(false, Ordering::Relaxed) {
                    fading.clear();
                }
                let gain = norm_gain(&status, Some(&s));
                show_norm(&status, Some(&s));
                log::info!("audio: loudness {:?} dB, normalization gain {gain:.3}", s.loudness_db);
                player.set_volume(slider_gain(&status, f32::from_bits(status.volume.load(Ordering::Relaxed))) * gain);
                // Fade in only over a song still playing, both over the same length.
                fading.retain(|f| {
                    if f.player.empty() {
                        if let Some(s) = &f.shared {
                            s.cancel();
                        }
                        false
                    } else {
                        true
                    }
                });
                let skip_len = status.crossfade_ms.load(Ordering::Relaxed);
                let natural_len = mix_ms(&status, length);
                let mut fade_in = 0;
                for f in fading.iter_mut().filter(|f| f.waiting.is_some()) {
                    // A song that ended on its own gets the full phrase; a skip stays quick.
                    fade_in = f.start(if f.natural { natural_len } else { skip_len }, status.automix.load(Ordering::Relaxed));
                    log::debug!("audio: crossfade into generation {generation} over {fade_in} ms");
                }
                fade_out = Arc::new(AtomicU32::new(0));
                let mix = fade_in > 0 && status.automix.load(Ordering::Relaxed);
                status.bpm.store(s.tempo.get().copied().flatten().unwrap_or(0.0).to_bits(), Ordering::Relaxed);
                status.mix_duration_ms.store(fade_in, Ordering::Relaxed);
                status.mix_active.store(mix, Ordering::Relaxed);
                status.mix_generation.store(generation, Ordering::Release);
                song_ms = Arc::new(AtomicU64::new(0));
                player.append(Meter::new(tempo(decoder, at, &song_ms), status.clone(), fade_in, fade_out.clone(), mix, s.tempo.clone()));
                player.play();
                // 0 means the stream didn't say; keep the length the UI already knows.
                if s.duration_ms > 0 {
                    status.duration_ms.store(s.duration_ms, Ordering::Relaxed);
                }
                length = status.duration();
                if let Some(old) = shared.replace(s.clone())
                    && !Arc::ptr_eq(&old, &s)
                {
                    old.cancel();
                }
                current = generation;
                seekable = !at.is_zero();
                ending_sent = false;
                status.stream_ms.store(s.duration_ms, Ordering::Relaxed);
                status.started_gen.store(generation, Ordering::Release);
                // The new song's needle, claimed before `playing`: the first sample from a
                // fresh decoder is a moment away, and until then the reader — the progress
                // bar, the party's announce — would take the song that just ended for this one.
                status.set_position(at.as_millis() as u64);
                status.playing.store(true, Ordering::Relaxed);
            }
            Ok(AudioCmd::Replace { generation, seek, decoder, at, seekable: s }) => {
                if generation != current || seek != seek_seq {
                    continue;
                }
                log::debug!("audio: seek to {at:?} via new decoder (seekable: {s})");
                seekable = s;
                let paused = player.is_paused();
                clear(&mut player, &output);
                fade_out = Arc::new(AtomicU32::new(0));
                let analysis = shared.as_ref().expect("a current generation has a stream").tempo.clone();
                song_ms = Arc::new(AtomicU64::new(0));
                player.append(Meter::new(tempo(decoder, at, &song_ms), status.clone(), 0, fade_out.clone(), false, analysis));
                if !paused {
                    player.play();
                }
                // Paused, nothing else tells the UI (and the system media controls) where it is now.
                status.set_position(at.as_millis() as u64);
                status.seek_revision.fetch_add(1, Ordering::Release);
                wake();
            }
            Ok(AudioCmd::Upgrade { generation, decoder, shared: s, at, seekable: s_seekable }) => {
                if generation != current || generation != status.wanted_gen.load(Ordering::Relaxed) {
                    s.cancel();
                    continue;
                }
                log::debug!("audio: upgrade generation {generation} to hi-res stream at {at:?}");
                if (output.sink.is_none() || output.default_changed()) && !output.open(tx.clone(), &status) {
                    let _ = events.send(AudioEvent::Error("No audio output device".into()));
                    wake();
                }
                let paused = player.is_paused();
                if !paused && !player.empty() {
                    // The YouTube copy fades out under the hi-res one, like a short crossfade.
                    fade_out.store(QUICK_FADE_MS, Ordering::Relaxed);
                    let old = std::mem::replace(&mut player, Player::connect_new(&output.mixer));
                    fading.push(Fading { player: old, song_ms: song_ms.clone(), shared: shared.take(), out: fade_out.clone(), waiting: None, natural: false });
                } else {
                    clear(&mut player, &output);
                    if let Some(old) = shared.take() {
                        old.cancel();
                    }
                }
                let gain = norm_gain(&status, Some(&s));
                show_norm(&status, Some(&s));
                log::info!("audio: hi-res loudness {:?} dB, normalization gain {gain:.3}", s.loudness_db);
                player.set_volume(slider_gain(&status, f32::from_bits(status.volume.load(Ordering::Relaxed))) * gain);
                fade_out = Arc::new(AtomicU32::new(0));
                let fade_in = if !paused { QUICK_FADE_MS } else { 0 };
                let analysis = s.tempo.clone();
                status.bpm.store(s.tempo.get().copied().flatten().unwrap_or(0.0).to_bits(), Ordering::Relaxed);
                song_ms = Arc::new(AtomicU64::new(0));
                player.append(Meter::new(tempo(decoder, at, &song_ms), status.clone(), fade_in, fade_out.clone(), false, analysis));
                if !paused {
                    player.play();
                }
                if s.duration_ms > 0 {
                    status.duration_ms.store(s.duration_ms, Ordering::Relaxed);
                }
                length = status.duration();
                if let Some(old) = shared.replace(s.clone())
                    && !Arc::ptr_eq(&old, &s)
                {
                    old.cancel();
                }
                seekable = s_seekable;
                ending_sent = false;
                status.stream_ms.store(s.duration_ms, Ordering::Relaxed);
                // The upgraded stream picks up at the same needle, not at the old one's
                // last reading.
                status.set_position(at.as_millis() as u64);
            }
            Ok(AudioCmd::Pause) => {
                log::debug!("audio: pause at {:?}", song_pos(&song_ms));
                // Fade out first; the players pause once it's done (below). Another pause
                // meanwhile (the media controls' and the window's) doesn't put that off.
                // With the fade off, it's over in a few ms.
                status.hushed.store(true, Ordering::Relaxed);
                if !player.is_paused() && pause_at.is_none() {
                    let after = if status.instant_pause.load(Ordering::Relaxed) { INSTANT_PAUSE_AFTER } else { PAUSE_AFTER };
                    pause_at = Some(Instant::now() + after);
                }
                status.playing.store(false, Ordering::Relaxed);
                // So the UI (and the system media controls) see it even when idle.
                wake();
            }
            Ok(AudioCmd::Resume) => {
                log::debug!("audio: resume (generation {current})");
                if current != 0 {
                    // Headphones may have come back (or gone) while paused.
                    if output.sink.is_none() || output.default_changed() {
                        output.open(tx.clone(), &status);
                    }
                    // Fades back in from where the pause got to (in a few ms with the fade off).
                    status.hushed.store(false, Ordering::Relaxed);
                    pause_at = None;
                    player.play();
                    fading.iter().for_each(|f| f.player.play());
                    status.playing.store(true, Ordering::Relaxed);
                    wake();
                }
            }
            // One crossfade at a time: skipping again mid-fade cuts over (below) instead
            // of piling a third song on.
            Ok(AudioCmd::Handoff)
                if mix_ms(&status, length) > 0
                    && current != 0
                    && !player.is_paused()
                    && !player.empty()
                    && fading.iter().all(|f| f.player.empty()) =>
            {
                // Keeps playing at full volume until the next song starts (see `Start`).
                // In real time, at the speed it's playing.
                let left = length.map(|d| d.saturating_sub(song_pos(&song_ms)).div_f32(status.speed()));
                log::debug!("audio: handing off from generation {current}, {left:?} left");
                let old = std::mem::replace(&mut player, Player::connect_new(&output.mixer));
                fading.push(Fading { player: old, song_ms: song_ms.clone(), shared: shared.take(), out: fade_out.clone(), waiting: Some((Instant::now(), left)), natural: ending_sent });
                current = 0;
                status.playing.store(false, Ordering::Relaxed);
                status.set_position(0);
            }
            Ok(cmd @ (AudioCmd::Stop | AudioCmd::Handoff)) => {
                if matches!(cmd, AudioCmd::Handoff) {
                    // Skipping again mid-crossfade: everything playing goes, quickly enough
                    // to sound like a cut but without a click, and the next song comes in
                    // at full volume.
                    for f in fading.iter_mut() {
                        f.waiting = None;
                        f.out.store(QUICK_FADE_MS, Ordering::Relaxed);
                    }
                    log::debug!("audio: skipped mid-crossfade, cutting over");
                    if current != 0 && !player.is_paused() && !player.empty() {
                        fade_out.store(QUICK_FADE_MS, Ordering::Relaxed);
                        let old = std::mem::replace(&mut player, Player::connect_new(&output.mixer));
                        fading.push(Fading { player: old, song_ms: song_ms.clone(), shared: shared.take(), out: fade_out.clone(), waiting: None, natural: false });
                    } else {
                        clear(&mut player, &output);
                    }
                } else {
                    clear(&mut player, &output);
                    fading.clear();
                }
                current = 0;
                status.mix_active.store(false, Ordering::Relaxed);
                status.mix_generation.store(0, Ordering::Release);
                // Also stops a seek still waiting on this download.
                if let Some(old) = shared.take() {
                    old.cancel();
                }
                status.playing.store(false, Ordering::Relaxed);
                status.set_position(0);
            }
            Ok(cmd @ (AudioCmd::Seek(_) | AudioCmd::SeekLive { .. })) => {
                let (pos, live) = match cmd {
                    AudioCmd::SeekLive { pos, at } => (pos + at.elapsed(), Some((pos, at))),
                    AudioCmd::Seek(pos) => (pos, None),
                    _ => unreachable!(),
                };
                seek_seq += 1;
                if current == 0 {
                    continue;
                }
                // Seeking ends a crossfade: the new song alone from here.
                for f in &fading {
                    f.out.store(QUICK_FADE_MS, Ordering::Relaxed);
                }
                // A seekable decoder already indexed the (fully downloaded) file. Without a
                // device to take the seek, a new decoder (below) instead. Likewise for a
                // podcast feed's file not all in memory: its seek reads at the new spot,
                // which would wait on the download inside the device's callback. A windowed
                // file's seek reads may hit blocks already dropped behind the reader, which
                // waits the same way: those rebuild off-thread too.
                let in_place = seekable && shared.as_ref().is_none_or(|s| (!s.feed && !s.windowed()) || s.is_complete());
                if in_place && player.try_seek(pos).is_ok() {
                    log::debug!("audio: seek to {pos:?} in place");
                    status.set_position(pos.as_millis() as u64);
                    status.seek_revision.fetch_add(1, Ordering::Release);
                    wake();
                    continue;
                }
                // The playing decoder was opened non-seekable for a fast start. Build a
                // seekable one over the same buffer off-thread (it needs every fragment
                // header, so it may wait for the download) and swap it in. Back to the
                // start, a fresh non-seekable decoder is enough and opens at once.
                if let Some(s) = shared.clone() {
                    // One builder at a time, always for the latest seek: a burst of seeks
                    // (a jam catching up) mustn't pile up threads waiting on the download.
                    *seek_job.lock().unwrap() = Some(SeekJob { shared: s, pos, live, generation: current, seek: seek_seq });
                    if !seek_busy.swap(true, Ordering::AcqRel) {
                        let (job, busy, tx, events) = (seek_job.clone(), seek_busy.clone(), tx.clone(), events.clone());
                        std::thread::spawn(move || {
                            loop {
                                let next = job.lock().unwrap().take();
                                match next {
                                    Some(j) => build_seek(j, &job, &tx, &events),
                                    None => {
                                        busy.store(false, Ordering::Release);
                                        // A seek that came in just now finds `busy` already false, or is taken here.
                                        if job.lock().unwrap().is_none() || busy.swap(true, Ordering::AcqRel) {
                                            break;
                                        }
                                    }
                                }
                            }
                        });
                    }
                }
            }
            // Failing here is `Start`'s to report.
            Ok(AudioCmd::Warm) => {
                if output.sink.is_none() {
                    output.open(tx.clone(), &status);
                }
            }
            Ok(AudioCmd::DeviceLost) => {
                let was = output.device.clone();
                output.open(tx.clone(), &status);
                // The device itself went away, not a hiccup on the same one: pause, as
                // unplugging headphones does, rather than carry on out of the speakers.
                if output.device != was && !player.is_paused() && (current != 0 || !fading.is_empty()) {
                    log::info!("audio: output device gone, pausing");
                    status.hushed.store(true, Ordering::Relaxed);
                    status.silence();
                    pause_at = None;
                    player.pause();
                    fading.iter().for_each(|f| f.player.pause());
                    status.playing.store(false, Ordering::Relaxed);
                    wake();
                }
            }
            Ok(AudioCmd::SetDevice(id)) => {
                log::info!("audio: output device set to {id:?}");
                output.preferred = id;
                if output.default_changed() || output.sink.is_none() {
                    output.open(tx.clone(), &status);
                }
            }
            Ok(AudioCmd::Volume(v)) => {
                let v = slider_gain(&status, v);
                player.set_volume(v * norm_gain(&status, shared.as_deref()));
                // The level may be what changed.
                if shared.is_some() {
                    show_norm(&status, shared.as_deref());
                }
                for f in &fading {
                    f.player.set_volume(v * norm_gain(&status, f.shared.as_deref()));
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return,
        }

        if current != 0 && dev_lost.is_some_and(|lost| song_pos(&song_ms) >= lost) {
            dev_lost = None;
            log::info!("audio: dev hook: output device lost");
            output.sink = None;
            output.device = Some("dev-hook-gone".into());
            output.lost.store(true, Ordering::Relaxed);
            let _ = tx.send(AudioCmd::DeviceLost);
        }
        // A pause has faded out (the quick way, if the fade was turned off on the way).
        pause_at = pause_due(pause_at, status.instant_pause.load(Ordering::Relaxed), Instant::now());
        if pause_at.is_some_and(|at| at <= Instant::now()) {
            pause_at = None;
            status.silence();
            player.pause();
            fading.iter().for_each(|f| f.player.pause());
        }
        // Follow the default device while playing (headphones connecting take over).
        // A failed device is `DeviceLost`'s to handle (it decides whether to pause).
        if (current != 0 || !fading.is_empty())
            && !player.is_paused()
            && output.checked.elapsed() >= DEVICE_CHECK
            && !output.lost.load(Ordering::Relaxed)
            && (output.sink.is_none() || output.default_changed())
        {
            output.open(tx.clone(), &status);
        }
        if !fading.is_empty() {
            if let Some(f) = fading.iter().find(|f| f.natural) {
                status.sample_outgoing(song_pos(&f.song_ms), !f.player.is_paused() && !f.player.empty());
            }
            // The next song is taking too long: don't hold this one up any longer.
            for f in fading.iter_mut().filter(|f| !status.automix.load(Ordering::Relaxed) && f.waiting.is_some_and(|(at, _)| at.elapsed() > HANDOFF_WAIT)) {
                let len = if f.natural { mix_ms(&status, length) } else { status.crossfade_ms.load(Ordering::Relaxed) };
                log::debug!("audio: next song not ready, fading out over {} ms", f.start(len, status.automix.load(Ordering::Relaxed)));
            }
            fading.retain(|f| {
                if f.player.empty() {
                    if let Some(s) = &f.shared {
                        s.cancel();
                    }
                    false
                } else {
                    true
                }
            });
            if fading.is_empty() {
                status.mix_active.store(false, Ordering::Relaxed);
                wake();
                log::debug!("audio: crossfade done");
            }
        }
        if current != 0 {
            let pos = song_pos(&song_ms);
            status.set_position(pos.as_millis() as u64);
            // Start the next song early, to crossfade into it. Not for songs barely
            // longer than the fade, nor while a pause fades out (it'd start the next one
            // playing). (The fade's in real time: more of the song when faster.)
            let crossfade = Duration::from_millis(mix_ms(&status, length) as u64).mul_f32(status.speed());
            if !ending_sent
                && !crossfade.is_zero()
                && !status.hushed.load(Ordering::Relaxed)
                && let Some(d) = length
                && d > crossfade * 2
                && pos + crossfade >= d
                // Not a podcast feed's file: the ads its host puts in make it run longer
                // than YouTube Music's length, so it plays to its real end.
                && shared.as_ref().is_some_and(|s| s.error().is_none() && !s.feed)
            {
                ending_sent = true;
                status.sample_outgoing(pos, !player.is_paused());
                let _ = events.send(AudioEvent::Ending { generation: current });
                wake();
            }
            // Paused just before the end, the song's over only once it's played again:
            // the next one mustn't start playing.
            if player.empty() && !status.hushed.load(Ordering::Relaxed) {
                // A failed download ends the decoder early: that's not the song ending.
                let event = match shared.as_ref().and_then(|s| s.failure()) {
                    Some(failure) => AudioEvent::Interrupted { generation: current, at: song_pos(&song_ms), failure },
                    None => AudioEvent::Finished { generation: current },
                };
                let _ = events.send(event);
                current = 0;
                shared = None;
                status.playing.store(false, Ordering::Relaxed);
                wake();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pausing_fades_out_and_playing_again_fades_back_from_there() {
        use std::num::NonZero;
        // 1 kHz mono of full-scale samples: 1 ms = 1 sample.
        let ones = || rodio::buffer::SamplesBuffer::new(NonZero::new(1).unwrap(), NonZero::new(1000).unwrap(), vec![1.0; 1000]);
        let fade = PAUSE_FADE_MS as usize;
        let status = Arc::new(AudioStatus::default());
        let mut m = Meter::new(ones(), status.clone(), 0, Arc::new(AtomicU32::new(0)), false, Default::default());
        assert_eq!(m.next(), Some(1.0));
        status.hushed.store(true, Ordering::Relaxed);
        let down: Vec<f32> = m.by_ref().take(fade / 2).collect();
        assert!(down.windows(2).all(|w| w[1] < w[0]));
        assert!((down[fade / 2 - 1] - 0.5).abs() < 0.01);
        // A seek's new decoder put in now carries on from there.
        assert!((Meter::new(ones(), status.clone(), 0, Arc::new(AtomicU32::new(0)), false, Default::default()).pause - 0.5).abs() < 0.01);
        // Played again half way down: back up from there, not from silence or full.
        status.hushed.store(false, Ordering::Relaxed);
        let up: Vec<f32> = m.by_ref().take(fade / 2).collect();
        assert!((up[0] - 0.5).abs() < 0.01);
        assert!((up[fade / 2 - 1] - 1.0).abs() < 1e-4);
        status.hushed.store(true, Ordering::Relaxed);
        // Silent once the fade is through, and it stays there.
        assert!(m.by_ref().skip(fade).take(10).all(|s| s == 0.0));
        // A new decoder put in while paused (a seek) waits silent, then fades in.
        let mut seeked = Meter::new(ones(), status.clone(), 0, Arc::new(AtomicU32::new(0)), false, Default::default());
        assert_eq!(seeked.next(), Some(0.0));
        status.hushed.store(false, Ordering::Relaxed);
    }

    #[test]
    fn with_the_fade_off_pausing_and_playing_again_take_a_few_ms() {
        use std::num::NonZero;
        let ones = || rodio::buffer::SamplesBuffer::new(NonZero::new(1).unwrap(), NonZero::new(1000).unwrap(), vec![1.0; 1000]);
        let quick = INSTANT_FADE_MS as usize;
        let status = Arc::new(AudioStatus::default());
        status.instant_pause.store(true, Ordering::Relaxed);
        let mut m = Meter::new(ones(), status.clone(), 0, Arc::new(AtomicU32::new(0)), false, Default::default());
        assert_eq!(m.next(), Some(1.0));
        // Down to silence in a few samples, not cut off at full (that would click).
        status.hushed.store(true, Ordering::Relaxed);
        let down: Vec<f32> = m.by_ref().take(quick).collect();
        assert!(down[0] < 1.0 && down[0] > 0.5);
        assert!(down.windows(2).all(|w| w[1] < w[0]));
        assert!(down[quick - 1] < 1e-4);
        // The players pause once that's through.
        status.silence();
        assert_eq!(m.next(), Some(0.0));
        // Played again: back up as quickly, as is a seek's decoder put in while paused.
        let mut seeked = Meter::new(ones(), status.clone(), 0, Arc::new(AtomicU32::new(0)), false, Default::default());
        status.hushed.store(false, Ordering::Relaxed);
        let up: Vec<f32> = m.by_ref().take(quick).collect();
        assert!(up[0] > 0.0 && up[0] < 0.5);
        assert!(up.windows(2).all(|w| w[1] > w[0]));
        assert!((up[quick - 1] - 1.0).abs() < 1e-4);
        assert!(seeked.next().unwrap() < 0.5);
        assert!((seeked.nth(quick - 2).unwrap() - 1.0).abs() < 1e-4);
    }

    #[test]
    fn turning_the_fade_off_mid_pause_finishes_it_the_quick_way() {
        use std::num::NonZero;
        let ones = || rodio::buffer::SamplesBuffer::new(NonZero::new(1).unwrap(), NonZero::new(1000).unwrap(), vec![1.0; 1000]);
        let status = Arc::new(AudioStatus::default());
        let mut m = Meter::new(ones(), status.clone(), 0, Arc::new(AtomicU32::new(0)), false, Default::default());
        status.hushed.store(true, Ordering::Relaxed);
        let half = m.by_ref().take(PAUSE_FADE_MS as usize / 2).last().unwrap();
        assert!((half - 0.5).abs() < 0.01);
        // Turned off half way down: the rest goes in a few samples, from where it got to.
        status.instant_pause.store(true, Ordering::Relaxed);
        let rest: Vec<f32> = m.by_ref().take(INSTANT_FADE_MS as usize).collect();
        assert!(rest[0] < half && rest[0] > 0.0);
        assert!(rest.last().unwrap() < &1e-4);

        // And the audio thread pauses the players once that's through, not the long fade.
        let now = Instant::now();
        let long = Some(now + PAUSE_AFTER);
        assert_eq!(pause_due(long, false, now), long);
        assert_eq!(pause_due(long, true, now), Some(now + INSTANT_PAUSE_AFTER));
        // One due sooner stays as it is; none stays none.
        let soon = Some(now + Duration::from_millis(1));
        assert_eq!(pause_due(soon, true, now), soon);
        assert_eq!(pause_due(None, true, now), None);
    }

    #[test]
    fn emptying_the_player_doesnt_wait_on_a_device_let_go() {
        use std::num::NonZero;
        let (mixer, source) = rodio::mixer::mixer(NonZero::new(2).unwrap(), NonZero::new(48_000).unwrap());
        // Let go of while paused: nothing pulls the mixer.
        let output = Output { mixer, slot: Arc::new(std::sync::Mutex::new(Some(source))), sink: None, device: None, preferred: None, lost: Default::default(), checked: Instant::now() };
        let mut player = Player::connect_new(&output.mixer);
        player.set_volume(0.5);
        player.append(rodio::buffer::SamplesBuffer::new(NonZero::new(2).unwrap(), NonZero::new(48_000).unwrap(), vec![0.0; 96_000]));
        player.pause();
        let (done, finished) = crossbeam_channel::bounded(1);
        std::thread::spawn(move || {
            clear(&mut player, &output);
            let _ = done.send((player.empty(), player.is_paused(), player.volume()));
        });
        assert_eq!(finished.recv_timeout(Duration::from_secs(2)).ok(), Some((true, true, 0.5)));
    }

    #[test]
    fn emptying_the_player_doesnt_wait_on_a_stalled_device() {
        use std::num::NonZero;
        let (mixer, source) = rodio::mixer::mixer(NonZero::new(2).unwrap(), NonZero::new(48_000).unwrap());
        // Playing, through a device that stopped pulling before it reported itself gone.
        let output = Output { mixer, slot: Arc::new(std::sync::Mutex::new(None)), sink: None, device: None, preferred: None, lost: Default::default(), checked: Instant::now() };
        let mut player = Player::connect_new(&output.mixer);
        player.set_volume(0.25);
        player.append(rodio::buffer::SamplesBuffer::new(NonZero::new(2).unwrap(), NonZero::new(48_000).unwrap(), vec![0.5; 96_000]));
        player.play();
        let (done, finished) = crossbeam_channel::bounded(1);
        std::thread::spawn(move || {
            clear(&mut player, &output);
            let _ = done.send((player.empty(), player.is_paused(), player.volume()));
        });
        assert_eq!(finished.recv_timeout(Duration::from_secs(2)).ok(), Some((true, true, 0.25)));
        // Once the device pulls again, the old song is gone within rodio's 5 ms check.
        let mut source = source;
        let tail: Vec<f32> = source.by_ref().take(9_600).skip(4_800).collect();
        assert!(tail.iter().all(|s| *s == 0.0));
    }

    #[test]
    fn a_song_counts_as_started_only_once_the_engine_starts_it() {
        let status = AudioStatus::default();
        status.started_gen.store(1, Ordering::Release);
        status.playing.store(true, Ordering::Relaxed);
        // The UI asked for 2, but the engine hasn't stopped 1 yet.
        assert!(!status.started(2));
        status.started_gen.store(2, Ordering::Release);
        assert!(status.started(2));
        status.playing.store(false, Ordering::Relaxed);
        assert!(!status.started(2));
    }

    #[test]
    fn only_a_length_the_stream_gave_counts_as_its_own() {
        let status = AudioStatus::default();
        // A music video's 5:00 the UI put in, for a song whose stream gives no length.
        status.duration_ms.store(300_000, Ordering::Relaxed);
        status.started_gen.store(2, Ordering::Release);
        status.playing.store(true, Ordering::Relaxed);
        assert_eq!(status.stream_duration(2), None);
        status.stream_ms.store(181_000, Ordering::Relaxed);
        assert_eq!(status.stream_duration(2), Some(Duration::from_secs(181)));
        // Not for the next song, until the engine starts it, with its own (here none).
        assert_eq!(status.stream_duration(3), None);
        status.stream_ms.store(0, Ordering::Relaxed);
        status.started_gen.store(3, Ordering::Release);
        assert_eq!(status.stream_duration(3), None);
    }

    #[test]
    fn outgoing_clock_survives_incoming_position_reset() {
        let status = AudioStatus::default();
        status.sample_outgoing(Duration::from_secs(180), false);
        status.set_position(0);
        assert_eq!(status.outgoing_position(), Duration::from_secs(180));
        status.set_position(2500);
        assert_eq!(status.outgoing_position(), Duration::from_secs(180));
    }

    #[test]
    fn paused_outgoing_clock_does_not_extrapolate() {
        let status = AudioStatus::default();
        status.sample_outgoing(Duration::from_secs(42), false);
        status.outgoing_sampled_at.store(0, Ordering::Relaxed);
        assert!(!status.outgoing_playing());
        assert_eq!(status.outgoing_position(), Duration::from_secs(42));
    }

    #[test]
    fn crossfade_ramps_mirror_each_other_and_end_the_fading_song() {
        use std::num::NonZero;
        // 1 kHz mono of full-scale samples: 1 ms = 1 sample.
        let ones = || rodio::buffer::SamplesBuffer::new(NonZero::new(1).unwrap(), NonZero::new(1000).unwrap(), vec![1.0; 1000]);
        let status = Arc::new(AudioStatus::default());
        let out = Arc::new(AtomicU32::new(0));
        let mut incoming = Meter::new(ones(), status.clone(), 100, Arc::new(AtomicU32::new(0)), false, Default::default());
        let mut outgoing = Meter::new(ones(), status, 0, out.clone(), false, Default::default());
        assert_eq!(outgoing.next(), Some(1.0));
        out.store(100, Ordering::Relaxed);
        let (a, b): (Vec<f32>, Vec<f32>) = (0..100).map(|_| (incoming.next().unwrap(), outgoing.next().unwrap())).unzip();
        assert_eq!(a[0], 0.0);
        assert_eq!(b[0], 1.0);
        // One comes up exactly as the other goes down.
        for (x, y) in a.iter().zip(&b) {
            assert!((x + y - 1.0).abs() < 1e-4);
        }
        assert!((b[50] - 0.5).abs() < 1e-4);
        assert_eq!(incoming.next(), Some(1.0));
        assert_eq!(outgoing.next(), None);
    }

    #[test]
    fn automix_envelopes_hold_then_exchange_at_sixty_five_percent() {
        use std::num::NonZero;
        let ones = || rodio::buffer::SamplesBuffer::new(NonZero::new(1).unwrap(), NonZero::new(1000).unwrap(), vec![1.0; 1000]);
        let status = Arc::new(AudioStatus::default());
        let out = Arc::new(AtomicU32::new(0));
        let mut incoming = Meter::new(ones(), status.clone(), 100, Arc::new(AtomicU32::new(0)), true, Default::default());
        let mut outgoing = Meter::new(ones(), status, 0, out.clone(), false, Default::default());
        assert_eq!(outgoing.next(), Some(1.0));
        out.store(100 | AUTOMIX_FADE, Ordering::Relaxed);
        let (a, b): (Vec<f32>, Vec<f32>) = (0..100).map(|_| (incoming.next().unwrap(), outgoing.next().unwrap())).unzip();
        // The outgoing song keeps the floor through the first third of the overlap.
        assert_eq!(a[25], 0.0);
        assert_eq!(b[25], 1.0);
        // Then an equal-power swap: no gap and no step at either end, and the combined
        // loudness of the two songs holds through the whole handoff.
        for (x, y) in a.iter().zip(&b) {
            assert!((x * x + y * y - 1.0).abs() < 1e-4);
        }
        assert!((a[65] - 0.5_f32.sqrt()).abs() < 1e-4);
        assert!((b[65] - 0.5_f32.sqrt()).abs() < 1e-4);
        assert_eq!(incoming.next(), Some(1.0));
        assert_eq!(outgoing.next(), None);
    }

    #[test]
    fn transition_fx_are_dry_outside_the_blend_and_keep_channels_separate() {
        let mut fx = MixFx::new(48_000, 2).unwrap();
        for i in 0..48_000 {
            let input = (i as f32 * 0.03).sin() * 0.5;
            assert_eq!(fx.process(input, None), input);
            assert_eq!(fx.process(0.0, None), 0.0);
        }
        fx.begin(120.0);
        let mut difference = 0.0;
        for frame in 0..48_000 {
            let input = (frame as f32 * 0.03).sin() * 0.5;
            let ramp = Some(Ramp::MixOut { from: 1.0, at: frame * 2, len: 96_000 });
            let left = fx.process(input, ramp);
            let right = fx.process(0.0, ramp);
            assert!(left.is_finite() && left.abs() < 1.0);
            assert_eq!(right, 0.0);
            difference += (left - input).abs();
        }
        assert!(difference > 10.0);
        assert_eq!(fx.process(0.25, None), 0.25);
        assert_eq!(fx.process(-0.25, None), -0.25);
    }

    #[test]
    fn automix_repeat_and_flanger_are_audible() {
        let mut fx = MixFx::new(48_000, 1).unwrap();
        fx.begin(120.0);
        // Two thirds through the blend: the wet path is up.
        let ramp = Some(Ramp::MixOut { from: 1.0, at: 60_000, len: 100_000 });
        let out: Vec<f32> = (0..30_000).map(|i| fx.process(if i == 0 { 1.0 } else { 0.0 }, ramp)).collect();
        // The flanger's short comb copy sits a few dozen frames behind the impulse.
        let comb: f32 = out[20..200].iter().map(|x| x.abs()).sum();
        assert!(comb > 0.01, "no flanged comb after the impulse: {comb}");
        // The beat repeat brings the impulse back a half beat later (12,000 frames).
        let echo: f32 = out[11_950..12_060].iter().map(|x| x.abs()).sum();
        assert!(echo > 0.01, "no half-beat repeat: {echo}");
    }

    #[test]
    fn enabled_effects_finish_without_changing_the_incoming_position() {
        use std::num::NonZero;
        let rate = 48_000;
        let source = |step: f32| rodio::buffer::SamplesBuffer::new(
            NonZero::new(1).unwrap(),
            NonZero::new(rate).unwrap(),
            (0..rate * 2).map(|i| (i as f32 * step).sin() * 0.3).collect::<Vec<_>>(),
        );
        let status = Arc::new(AudioStatus::default());
        status.automix.store(true, Ordering::Relaxed);
        let out = Arc::new(AtomicU32::new(0));
        let mut incoming = Meter::new(source(0.04), status.clone(), 1000, Default::default(), true, Default::default());
        let mut outgoing = Meter::new(source(0.03), status, 0, out.clone(), false, Arc::new(OnceLock::from(Some(120.0))));
        assert!(incoming.fx.is_some() && outgoing.fx.is_some());
        assert_eq!(outgoing.by_ref().take(12_000).count(), 12_000);
        out.store(1000 | AUTOMIX_FADE, Ordering::Relaxed);
        for _ in 0..rate {
            let mixed = incoming.next().unwrap() + outgoing.next().unwrap();
            assert!(mixed.is_finite() && mixed.abs() < 1.0);
        }
        assert_eq!(outgoing.next(), None);
        assert_eq!(incoming.next(), Some((rate as f32 * 0.04).sin() * 0.3));
        assert!(incoming.ramp.is_none());
    }

    #[test]
    fn a_quick_fade_retains_the_effect_instead_of_jumping_to_dry() {
        let mut fx = MixFx::new(48_000, 1).unwrap();
        fx.begin(120.0);
        let mut before = 0.0;
        for at in 0..36_000 {
            before = fx.process(0.4, Some(Ramp::MixOut { from: 1.0, at, len: 48_000 }));
        }
        let after = fx.process(0.4, Some(Ramp::Out { from: 0.3, at: 0, len: 1920 }));
        assert!((after - before).abs() < 0.01);
        assert!((after - 0.4).abs() > 0.05);
    }

    #[test]
    fn transition_buffers_are_bounded_and_do_not_grow() {
        for rate in [8_000, 44_100, 48_000, 96_000, 192_000] {
            let mut fx = MixFx::new(rate, 2).unwrap();
            let bytes = fx.ring.capacity() * std::mem::size_of::<f32>();
            assert!(bytes <= FX_MAX_FRAMES * 2 * 4);
            let address = fx.ring.as_ptr();
            fx.begin(70.0);
            assert!(fx.beat <= (fx.ring.len() / fx.channels - 1) as f32);
            for at in 0..rate * 2 {
                fx.process(0.1, Some(Ramp::MixOut { from: 1.0, at, len: rate * 2 }));
            }
            assert_eq!(fx.ring.as_ptr(), address);
            assert_eq!(fx.ring.capacity() * 4, bytes);
        }
        assert!(MixFx::new(48_000, 9).is_none());
    }

    #[test]
    fn unknown_tempo_uses_a_finite_tail_and_seek_clears_history() {
        let mut fx = MixFx::new(48_000, 1).unwrap();
        fx.begin(0.0);
        assert!(!fx.rhythmic);
        let mut tail = 0.0;
        for at in 0..48_000 {
            let input = if at == 24_000 { 0.5 } else { 0.0 };
            let sample = fx.process(input, Some(Ramp::MixOut { from: 1.0, at, len: 48_000 }));
            assert!(sample.is_finite() && sample.abs() <= 0.5);
            if at > 24_001 {
                tail += sample.abs();
            }
        }
        assert!(tail > 0.001);
        fx.reset();
        assert!(fx.ring.iter().all(|v| *v == 0.0));
        assert_eq!(fx.process(0.0, None), 0.0);
    }

    #[test]
    fn tempo_analysis_stops_after_its_windows_quiet_or_not() {
        for silence in [false, true] {
            let mut tempo = Tempo::new(20.0);
            let mut result = None;
            let mut windows = 0;
            while !tempo.done {
                for i in 0..300 {
                    result = tempo.push(if !silence && (i + windows * 300) % 25 == 0 { 1.0 } else { 0.0 });
                }
                windows += 1;
                assert!(windows <= TEMPO_PASSES as usize, "the analysis never stops");
            }
            // A beat answers in the first window; nothing but silence takes all four.
            assert_eq!(windows, if silence { TEMPO_PASSES as usize } else { 1 });
            if silence {
                assert!(result.is_none());
            } else {
                assert!((result.unwrap() - 120.0).abs() < 1.0);
            }
            let history = tempo.hist;
            for _ in 0..1000 {
                assert!(tempo.push(0.8).is_none());
            }
            assert_eq!(tempo.hist, history);
            assert_eq!(tempo.count, 300);
        }
    }

    #[test]
    fn replacing_a_decoder_reuses_its_completed_analysis() {
        use std::num::NonZero;
        for result in [Some(120.0), None] {
            let analysis = Arc::new(OnceLock::from(result));
            let source = rodio::buffer::SamplesBuffer::new(NonZero::new(2).unwrap(), NonZero::new(48_000).unwrap(), vec![0.0; 4096]);
            let status = Arc::new(AudioStatus::default());
            status.automix.store(true, Ordering::Relaxed);
            let mut meter = Meter::new(source, status, 0, Default::default(), false, analysis.clone());
            assert!(meter.tempo.done);
            assert_eq!(meter.by_ref().count(), 4096);
            assert_eq!(meter.tempo.count, 0);
            assert_eq!(analysis.get(), Some(&result));
        }
    }

    #[test]
    fn mix_status_is_bound_to_the_started_generation() {
        let status = AudioStatus::default();
        status.mix_duration_ms.store(12_345, Ordering::Relaxed);
        status.mix_active.store(true, Ordering::Relaxed);
        status.mix_generation.store(7, Ordering::Release);
        assert_eq!(status.mix_duration(6), None);
        assert_eq!(status.mix_duration(7), Some(12.345));
        assert!(status.mixing());
        status.mix_active.store(false, Ordering::Relaxed);
        assert!(!status.mixing());
    }

    #[test]
    fn normalization_brings_loud_and_quiet_songs_to_the_level() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let audio = AudioHandle { tx, status: Arc::new(AudioStatus::default()) };
        let status = &audio.status;
        let loud = Shared::new(1, 0, Some(7.29), crate::stream::Format::Aac, None, false);
        let quiet = Shared::new(1, 0, Some(-4.0), crate::stream::Format::Aac, None, false);
        let very_quiet = Shared::new(1, 0, Some(-15.0), crate::stream::Format::Aac, None, false);
        let unknown = Shared::new(1, 0, None, crate::stream::Format::Aac, None, false);
        audio.set_normalize(Normalize::Normal);
        assert!(status.limit.load(Ordering::Relaxed));
        assert!((norm_gain(status, Some(&loud)) - 0.432).abs() < 0.001);
        assert!((norm_gain(status, Some(&quiet)) - 1.585).abs() < 0.001);
        // Up by no more than 6 dB.
        assert_eq!(norm_db(status, Some(&very_quiet)), Some(MAX_BOOST_DB));
        assert_eq!(norm_gain(status, Some(&unknown)), 1.0);
        assert_eq!(norm_gain(status, None), 1.0);
        // Quiet: 5 dB lower, so a song 4 dB under the target comes down a little.
        audio.set_normalize(Normalize::Quiet);
        assert_eq!(norm_db(status, Some(&quiet)), Some(-1.0));
        assert!((norm_db(status, Some(&loud)).unwrap() + 12.29).abs() < 1e-4);
        audio.set_normalize(Normalize::Off);
        assert!(!status.limit.load(Ordering::Relaxed));
        assert_eq!(norm_gain(status, Some(&loud)), 1.0);
        assert_eq!(norm_gain(status, Some(&quiet)), 1.0);
        show_norm(status, Some(&loud));
        assert_eq!(audio.normalization(), None);
    }

    #[test]
    fn the_old_switch_keeps_its_meaning() {
        assert_eq!(Normalize::from_key("1"), Normalize::Normal);
        assert_eq!(Normalize::from_key("0"), Normalize::Off);
        assert_eq!(Normalize::from_key("quiet"), Normalize::Quiet);
        assert_eq!(Normalize::from_key(""), Normalize::Normal);
        for l in Normalize::ALL {
            assert_eq!(Normalize::from_u8(l as u8), l);
        }
    }

    #[test]
    fn log_volume_keeps_loudness_when_switched() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let audio = AudioHandle { tx, status: Arc::new(AudioStatus::default()) };
        audio.set_volume(0.125);
        audio.set_log_volume(true);
        assert!((audio.volume() - 0.5).abs() < 1e-6);
        assert!((slider_gain(&audio.status, audio.volume()) - 0.125).abs() < 1e-6);
        audio.set_log_volume(true);
        assert!((audio.volume() - 0.5).abs() < 1e-6);
        audio.set_log_volume(false);
        assert!((audio.volume() - 0.125).abs() < 1e-6);
    }
}
