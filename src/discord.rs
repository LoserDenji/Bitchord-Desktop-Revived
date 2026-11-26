//! Discord Rich Presence: "Listening to <song>" with cover art and a progress bar, laid
//! out as the listener sets it in Settings › Discord. A port of
//! ytm-rs's card model, with the fork's own two additions kept: the audio-quality line
//! BitChord-PC-Continued shows on the cover (BitChord's hi-res pipeline), and the
//! project button that opens this repo.
//!
//! A worker thread owns the IPC pipe so a missing or restarting Discord never blocks the
//! UI. The UI sends the wanted state whenever it changes; the worker coalesces bursts
//! (Discord rate-limits activity updates) and reconnects every 15 s while Discord is
//! closed. A listener who linked their account gets the same card through their own
//! gateway instead (`discord_link.rs`): the card is built once, here, for both.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use discord_rich_presence::activity::{Activity, ActivityType, Assets, Button, StatusDisplayType, Timestamps};
use discord_rich_presence::{DiscordIpc, DiscordIpcClient};
use serde::{Deserialize, Serialize};

use crate::discord_artwork::{ImagePrefs, Picture};

/// Application ID from the Discord Developer Portal. Its name is what Discord shows
/// as "Listening to <name>" in the profile card. This project's own, set at build
/// time (`BITCHORD_DISCORD_CLIENT_ID`): until 2.0.3 the fork rode on ytm-rs's
/// application, showing its users as ytm-rs on Discord. A build without one of its
/// own (empty here) keeps the card off rather than do that again.
const CLIENT_ID: &str = match option_env!("BITCHORD_DISCORD_CLIENT_ID") {
    Some(id) if !id.is_empty() => id,
    _ => "",
};

/// That application's name, unless the listener gives another.
pub const APP_NAME: &str = "BitChord PC";
pub const STATUS_QUALITY: &str = "{status} · {quality}";

/// The fork's own page, for the card's project button and Settings' example song.
pub const PROJECT_URL: &str = "https://github.com/LoserDenji/Bitchord-Desktop-Revived";
/// What the project button says; Discord caps button labels at 32 characters.
pub const PROJECT_BUTTON: &str = "Get BitChord PC";

/// The project's Discord server: the Join server button in Settings and the update
/// catalogue both open this.
pub const DISCORD_INVITE: &str = "https://discord.gg/bn2u89xUeH";

/// Discord fetches the public image. The app renders its own bundled copy.
pub const ICON: &str = "https://wsrv.nl/?url=raw.githubusercontent.com/kushagrasinghx/BitChord/acf219a7e4a7f52f026d4336d5c7ac5cf7bfafcc/app/src/main/res/mipmap-xxxhdpi/ic_liquid_glass_foreground.png&cx=38&cy=38&cw=356&ch=356&output=png";
pub const ICON_SOURCE: &str = "https://raw.githubusercontent.com/kushagrasinghx/BitChord/acf219a7e4a7f52f026d4336d5c7ac5cf7bfafcc/app/src/main/res/mipmap-xxxhdpi/ic_liquid_glass_foreground.png";
pub const ICON_BYTES: &[u8] = include_bytes!("../assets/icon/discord.png");

const RETRY: Duration = Duration::from_secs(15);

/// What a line of the card shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Show {
    Title,
    Artist,
    Album,
    /// The audio quality the fork plays the song at ("Hi-Res Lossless · FLAC · 1641 kbps
    /// · 24-bit · Stereo", "High"): the mark BitChord-PC-Continued put beside the cover.
    Quality,
    Nothing,
    Template,
}

impl Show {
    pub const ALL: [Self; 6] = [Self::Title, Self::Artist, Self::Album, Self::Quality, Self::Nothing, Self::Template];

    pub fn label(self) -> &'static str {
        match self {
            Self::Title => "Song title",
            Self::Artist => "Artist",
            Self::Album => "Album",
            Self::Quality => "Audio quality",
            Self::Nothing => "Nothing",
            Self::Template => "Custom template",
        }
    }
}

/// What the member list shows after "Listening to".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Name,
    Line1,
    #[default]
    Line2,
}

impl Status {
    pub const ALL: [Self; 3] = [Self::Name, Self::Line1, Self::Line2];
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    #[default]
    Listening,
    Playing,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Listening => "Listening to",
            Self::Playing => "Playing",
        }
    }
}

/// A page a button or the artwork opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Page {
    None,
    Song,
    Artist,
    Album,
    /// The Listen together room, while there is one.
    Jam,
    /// This project's page: the one button the old card always carried.
    Project,
    Spotify,
    AppleMusic,
    Tidal,
    Custom,
}

impl Page {
    pub const ALL: [Self; 10] =
        [Self::None, Self::Song, Self::Artist, Self::Album, Self::Jam, Self::Project, Self::Spotify, Self::AppleMusic, Self::Tidal, Self::Custom];

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Song => "Song",
            Self::Artist => "Artist",
            Self::Album => "Album",
            Self::Jam => "Listen together",
            Self::Project => APP_NAME,
            Self::Spotify => "Spotify",
            Self::AppleMusic => "Apple Music",
            Self::Tidal => "Tidal",
            Self::Custom => "Custom URL",
        }
    }

    fn button(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Song => "Listen on YouTube Music",
            Self::Artist => "View artist",
            Self::Album => "View album",
            Self::Jam => "Listen together",
            Self::Project => PROJECT_BUTTON,
            Self::Spotify => "Find on Spotify",
            Self::AppleMusic => "Find on Apple Music",
            Self::Tidal => "Find on Tidal",
            Self::Custom => "Open link",
        }
    }
}

/// How the card looks in Settings › Discord. One set of wording for
/// both paths: the plain IPC card and the linked account's gateway one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub when_paused: bool,
    pub hide_details: bool,
    /// Replaces [`APP_NAME`]; empty keeps it.
    pub app_name: String,
    pub status: Status,
    pub lines: [Show; 3],
    pub art: bool,
    pub progress: bool,
    /// Lines 1 and 2 open what they show.
    pub links: [bool; 2],
    pub art_link: bool,
    /// The buttons below the card, which only others see.
    pub show_buttons: bool,
    pub buttons: [Page; 2],
    pub kind: Kind,
    pub elapsed_only: bool,
    pub templates: [String; 2],
    pub annotation: String,
    pub images: [ImagePrefs; 2],
    pub button_labels: [Option<String>; 2],
    pub button_urls: [String; 2],
    pub template_links: [Page; 2],
    pub artwork: crate::discord_artwork::Options,
    pub refresh_seconds: u8,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            when_paused: true,
            hide_details: false,
            app_name: String::new(),
            status: Status::Line2,
            lines: [Show::Title, Show::Artist, Show::Album],
            art: true,
            progress: true,
            links: [true, true],
            art_link: false,
            show_buttons: true,
            // The old card's two buttons: the song on YouTube Music, and this project.
            buttons: [Page::Song, Page::Project],
            kind: Kind::Listening,
            elapsed_only: false,
            templates: ["{song}".into(), "{artist}".into()],
            annotation: "{quality}".into(),
            images: [ImagePrefs::album(), ImagePrefs::logo()],
            button_labels: Default::default(),
            button_urls: Default::default(),
            template_links: [Page::Song, Page::Artist],
            artwork: Default::default(),
            refresh_seconds: 3,
        }
    }
}

impl Prefs {
    pub fn from_saved(json: &str) -> Option<Self> {
        let mut prefs: Self = serde_json::from_str(json).ok()?;
        if prefs.annotation == STATUS_QUALITY {
            prefs.annotation = "{quality}".into();
        }
        // Apply the new layout to an untouched save from the previous build.
        let previous = Self {
            when_paused: prefs.when_paused,
            status: Status::Line1,
            lines: [Show::Title, Show::Artist, Show::Quality],
            links: [true, false],
            templates: ["{name}".into(), "{artist}".into()],
            images: [ImagePrefs { hover: "{quality}".into(), ..ImagePrefs::album() }, ImagePrefs::default()],
            ..Self::default()
        };
        if prefs == previous {
            return Some(Self { when_paused: prefs.when_paused, ..Self::default() });
        }
        for template in std::iter::once(&mut prefs.app_name)
            .chain(prefs.templates.iter_mut())
            .chain(std::iter::once(&mut prefs.annotation))
            .chain(prefs.images.iter_mut().map(|image| &mut image.hover))
            .chain(prefs.button_labels.iter_mut().flatten())
        {
            if template.contains("{name}") {
                *template = template.replace("{name}", "{song}");
            }
        }
        Some(prefs)
    }

    /// The wording saved by the old Discord ("Discord via link" options), carried over
    /// the first time the new card runs: the name it was given, the member-list line it
    /// led with, the buttons it showed.
    pub fn migrated(old: &str) -> Self {
        // The old options' shape; a field a save never carried reads as its default.
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Saved {
            use_details: Option<bool>,
            activity_name: Option<String>,
            button1_visible: Option<bool>,
            button2_visible: Option<bool>,
        }
        // Nothing the old Discord saved (or a file that won't read): the new card's
        // defaults, not the old one's.
        let Ok(saved) = serde_json::from_str::<Saved>(old) else { return Self::default() };
        Self {
            app_name: saved.activity_name.unwrap_or_default(),
            status: if saved.use_details.unwrap_or(false) { Status::Line1 } else { Status::Line2 },
            buttons: [
                if saved.button1_visible.unwrap_or(true) { Page::Song } else { Page::None },
                if saved.button2_visible.unwrap_or(true) { Page::Project } else { Page::None },
            ],
            ..Self::default()
        }
    }
}

/// The song as Discord gets to know it.
#[derive(Clone, Debug, PartialEq)]
pub struct Presence {
    pub video_id: String,
    pub title: String,
    pub artist: String,
    pub artist_id: Option<String>,
    pub album: Option<String>,
    pub album_id: Option<String>,
    pub art: Option<String>,
    pub artist_art: Option<String>,
    pub duration_ms: u64,
    pub position_ms: u64,
    pub clock: (u64, u64, u32),
    /// The audio quality mark, as the player's track-info words it; `None` hides it.
    pub quality: Option<String>,
    /// Unix ms when the song (virtually) started, so Discord draws elapsed time.
    pub start_ms: i64,
    pub end_ms: Option<i64>,
    pub paused: bool,
    /// The share link of the Listen together room we're in.
    pub jam: Option<String>,
}

impl Presence {
    fn url(&self, page: Page) -> Option<String> {
        let query = || format!("{} {}", self.title, self.artist);
        let search = |base, param| {
            let mut u = url::Url::parse(base).ok()?;
            u.query_pairs_mut().append_pair(param, query().trim());
            Some(u.into())
        };
        let id = |base: &str, id: &str| {
            let mut u = url::Url::parse(base).ok()?;
            u.path_segments_mut().ok()?.push(id);
            Some(u.into())
        };
        match page {
            Page::None | Page::Custom => None,
            Page::Song if crate::local::is_local(&self.video_id) || self.video_id.is_empty() => search("https://music.youtube.com/search", "q"),
            Page::Song => {
                let mut u = url::Url::parse("https://music.youtube.com/watch").ok()?;
                u.query_pairs_mut().append_pair("v", &self.video_id);
                Some(u.into())
            }
            Page::Artist => match self.artist_id.as_deref() {
                Some(artist) => id("https://music.youtube.com/channel", artist),
                None if !self.artist.trim().is_empty() => {
                    let mut u = url::Url::parse("https://music.youtube.com/search").ok()?;
                    u.query_pairs_mut().append_pair("q", self.artist.trim());
                    Some(u.into())
                }
                None => None,
            },
            Page::Album => id("https://music.youtube.com/browse", self.album_id.as_deref()?),
            Page::Jam => self.jam.clone(),
            Page::Project => Some(PROJECT_URL.to_owned()),
            Page::Spotify => id("https://open.spotify.com/search", query().trim()),
            Page::AppleMusic => search("https://music.apple.com/search", "term"),
            Page::Tidal => search("https://listen.tidal.com/search", "q"),
        }
        .filter(|u| web_url(u, 512).is_some())
    }

    fn line(&self, show: Show, template: &str, link: bool, destination: Page) -> Option<Line> {
        let (text, page) = match show {
            Show::Title => (normalized(&self.title, 128), Page::Song),
            Show::Artist => (normalized(&self.artist, 128), Page::Artist),
            Show::Album => (normalized(self.album.as_deref()?, 128), Page::Album),
            Show::Quality => (normalized(self.quality.as_deref()?, 128), Page::None),
            Show::Template => (self.render(template, 128), destination),
            Show::Nothing => return None,
        };
        Some(Line { text: text?, url: link.then(|| self.url(page)).flatten() })
    }

    /// Expand once so braces inside a title or other metadata stay literal.
    fn render(&self, template: &str, limit: usize) -> Option<String> {
        let duration = (self.duration_ms > 0).then(|| {
            let secs = self.duration_ms / 1000;
            format!("{}:{:02}", secs / 60, secs % 60)
        });
        let mut out = String::new();
        let mut missing = false;
        let mut remaining = template;
        while let Some((prefix, rest)) = remaining.split_once('{') {
            out.push_str(prefix);
            let Some((key, tail)) = rest.split_once('}') else {
                remaining = "";
                break;
            };
            let value = match key {
                "song" | "name" => &self.title,
                "artist" => &self.artist,
                "album" => self.album.as_deref().unwrap_or_default(),
                "app" => APP_NAME,
                "quality" => self.quality.as_deref().unwrap_or_default(),
                "apple" => "\u{f8ff}",
                "duration" => duration.as_deref().unwrap_or_default(),
                "status" if self.paused => "Paused",
                "status" => "Playing",
                _ => "",
            };
            missing |= value.is_empty();
            out.push_str(value);
            remaining = tail;
        }
        out.push_str(remaining);
        let text = if missing { out.trim_matches(|c: char| c.is_whitespace() || matches!(c, '·' | '•' | '|')) } else { &out };
        normalized(text, limit)
    }
}

/// The quality line Discord carries: the tier, the bitrate and the sample rate —
/// "Lossless · 1411 kbps · 44.1 kHz" — each part dropped when the stream didn't
/// say it.
pub(crate) fn quality_summary(tier: &str, bitrate_kbps: Option<u32>, sample_rate: Option<u32>) -> String {
    let rate = sample_rate.filter(|hz| *hz > 0).map(|hz| {
        if hz % 1000 == 0 {
            format!("{} kHz", hz / 1000)
        } else {
            let fraction = format!("{:03}", hz % 1000);
            format!("{}.{} kHz", hz / 1000, fraction.trim_end_matches('0'))
        }
    });
    let bitrate = bitrate_kbps.filter(|k| *k > 0).map(|k| format!("{k} kbps"));
    [Some(tier.to_owned()), bitrate, rate].into_iter().flatten().collect::<Vec<_>>().join(" · ")
}

pub(crate) fn web_url(value: &str, limit: usize) -> Option<String> {
    let value = value.trim();
    if value.len() > limit || value.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    let u = url::Url::parse(value).ok()?;
    (matches!(u.scheme(), "https" | "http") && u.host().is_some() && u.username().is_empty() && u.password().is_none()).then(|| value.to_owned())
}

fn normalized(text: &str, limit: usize) -> Option<String> {
    // Discord's client counts UTF-16 units. Leave room for the ellipsis and never split a character.
    let clean = text.split(|c: char| c.is_whitespace() || c.is_control()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ");
    (!clean.is_empty()).then(|| truncate(clean, limit))
}

fn truncate(text: String, limit: usize) -> String {
    if text.encode_utf16().count() > limit {
        let mut units = 0;
        let short: String = text
            .chars()
            .take_while(|c| {
                units += c.len_utf16();
                units < limit
            })
            .collect();
        return format!("{}…", short.trim_end());
    }
    text
}

/// A card line, and the page it opens when it's a link.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub text: String,
    pub url: Option<String>,
}

/// The card as Discord draws it, from a song and the listener's settings. Settings'
/// preview draws the same thing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Card {
    pub name: Option<String>,
    pub status: Status,
    pub kind: Kind,
    /// The third is the large artwork's hover text.
    pub lines: [Option<Line>; 3],
    pub annotation: Option<String>,
    pub art: Option<Picture>,
    pub art_url: Option<String>,
    pub small_art: Option<Picture>,
    pub small_text: Option<String>,
    /// Start and end, Unix ms.
    pub times: Option<(i64, Option<i64>)>,
    clock: Option<(u64, u64, u32)>,
    pub buttons: Vec<(String, String)>,
}

impl Card {
    pub fn new(p: &Presence, prefs: &Prefs, [art, small_art]: [Option<Picture>; 2]) -> Self {
        let name = if prefs.hide_details && prefs.app_name.contains('{') {
            // A template can contain the title or other private listening details.
            None
        } else {
            p.render(&prefs.app_name, 128)
        };
        if prefs.hide_details {
            return Self { name, kind: prefs.kind, status: Status::Name, ..Default::default() };
        }
        let art = art.filter(|_| prefs.art);
        let small_art = small_art.filter(|_| art.is_some());
        let mut lines = [
            p.line(prefs.lines[0], &prefs.templates[0], prefs.links[0], prefs.template_links[0]),
            p.line(prefs.lines[1], &prefs.templates[1], prefs.links[1], prefs.template_links[1]),
            art.as_ref().and_then(|_| p.line(prefs.lines[2], &prefs.images[0].hover, false, Page::None)),
        ];
        // Discord has no paused activity state. Keep the pause label on the
        // default artist line and let an explicit state template render itself.
        let pause_label = p.paused && prefs.lines[1] == Show::Artist;
        if pause_label {
            lines[1] = Some(Line { text: "Paused".to_owned(), url: None });
        }
        // A status line with nothing on it falls back to the app's name, as Discord does.
        let status = match prefs.status {
            Status::Line1 if lines[0].is_none() => Status::Name,
            Status::Line2 if pause_label && lines[0].is_some() => Status::Line1,
            Status::Line2 if lines[1].is_none() => Status::Name,
            s => s,
        };
        let art_url = art.as_ref().filter(|_| prefs.art_link).and_then(|_| p.url(Page::Album).or_else(|| p.url(Page::Song)));
        let hover = prefs.images[1].hover.trim();
        let hover = if hover.is_empty() { STATUS_QUALITY } else { hover };
        let small_text = small_art.as_ref().and_then(|_| p.render(hover, 128));
        let annotation = art.as_ref().and_then(|_| p.render(&prefs.annotation, 128));
        let mut buttons = Vec::new();
        if prefs.show_buttons {
            for (i, page) in prefs.buttons.into_iter().enumerate() {
                let url = if page == Page::Custom { web_url(&prefs.button_urls[i], 512) } else { p.url(page) };
                let Some(url) = url.filter(|u| buttons.iter().all(|(_, b)| b != u)) else { continue };
                let label = match &prefs.button_labels[i] {
                    Some(template) => p.render(template, 32),
                    None => Some(page.button().to_owned()),
                };
                if let Some(label) = label {
                    buttons.push((label, url));
                }
            }
        }
        let end = (prefs.kind == Kind::Listening && !prefs.elapsed_only).then_some(p.end_ms).flatten();
        let times = (prefs.progress && !p.paused).then_some((p.start_ms, end));
        Self { name, kind: prefs.kind, status, lines, annotation, art, art_url, small_art, small_text, times, clock: times.map(|_| p.clock), buttons }
    }

    pub fn name(&self) -> &str {
        self.name.as_deref().unwrap_or(APP_NAME)
    }

    /// Discord's profile card can draw the large artwork text below the artist.
    pub fn large_text(&self) -> Option<&str> {
        self.annotation.as_deref().or_else(|| self.lines[2].as_ref().map(|line| line.text.as_str()))
    }

    /// Allow the clock to drift between audio samples. An explicit seek or speed
    /// change carries its own clock revision.
    pub fn same(&self, other: &Self, ignore_badge_progress: bool) -> bool {
        let Self { name, kind, status, lines, annotation, art, art_url, small_art, small_text, times, clock, buttons } = self;
        let same_picture = |a: &Option<Picture>, b: &Option<Picture>| {
            if ignore_badge_progress && let Some(((_, paused_a), (_, paused_b))) = a.as_ref().and_then(|p| p.badge).zip(b.as_ref().and_then(|p| p.badge)) {
                return paused_a == paused_b;
            }
            a.as_ref().map(Picture::addresses) == b.as_ref().map(Picture::addresses)
        };
        let near = |a: i64, b: i64| a.abs_diff(b) < 2000;
        let near_end = |a: Option<i64>, b: Option<i64>| match (a, b) {
            (Some(a), Some(b)) => near(a, b),
            (a, b) => a.is_none() && b.is_none(),
        };
        let times = match (*times, other.times) {
            (Some((a, x)), Some((b, y))) => near(a, b) && near_end(x, y),
            (a, b) => a.is_none() && b.is_none(),
        };
        times
            && *name == other.name
            && *kind == other.kind
            && *clock == other.clock
            && *status == other.status
            && *lines == other.lines
            && *annotation == other.annotation
            && same_picture(art, &other.art)
            && same_picture(small_art, &other.small_art)
            && *small_text == other.small_text
            && *art_url == other.art_url
            && *buttons == other.buttons
    }

    /// The member list's "Listening to …".
    pub fn status_text(&self) -> &str {
        let line = |i: usize| self.lines[i].as_ref().map(|l| l.text.as_str());
        match self.status {
            Status::Name => None,
            Status::Line1 => line(0),
            Status::Line2 => line(1),
        }
        .unwrap_or(self.name())
    }

    /// What the member list shows beside its music note: the status line, then " · " and
    /// the other of lines 1 and 2, as in "Believer · Imagine Dragons".
    pub fn member_text(&self) -> String {
        let other = match self.status {
            Status::Name => None,
            Status::Line1 => self.lines[1].as_ref(),
            Status::Line2 => self.lines[0].as_ref(),
        };
        match other {
            Some(l) => format!("{} · {}", self.status_text(), l.text),
            None => self.status_text().to_owned(),
        }
    }
}

/// Keep bursts within Discord's five updates per twenty seconds. A clear is immediate
/// so stopped playback cannot leave a stale song on the profile.
#[derive(Default)]
pub struct Updates(VecDeque<Instant>);

impl Updates {
    pub fn delay(&mut self, now: Instant, seconds: u8, badge_progress: bool) -> Duration {
        let window = Duration::from_secs(20);
        self.0.retain(|at| now.duration_since(*at) < window);
        let interval = self.0.back().map_or(Duration::ZERO, |at| Duration::from_secs(seconds.clamp(1, 10).into()).saturating_sub(now.duration_since(*at)));
        // Reserve two places for playback changes while a progress badge is moving.
        let limit = if badge_progress { 3 } else { 5 };
        let burst = self.0.len().checked_sub(limit).map_or(Duration::ZERO, |i| window.saturating_sub(now.duration_since(self.0[i])));
        interval.max(burst)
    }

    pub fn record(&mut self, now: Instant) {
        self.0.push_back(now);
    }
}

pub struct Discord {
    tx: Sender<Option<Card>>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl Discord {
    pub fn spawn() -> Option<Self> {
        let id = std::env::var("YTM_DISCORD_ID").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| CLIENT_ID.to_owned());
        if id.is_empty() {
            log::warn!("discord: no application of this project's own in this build; the card stays off");
            return None;
        }
        let (tx, rx) = crossbeam_channel::unbounded();
        let join = std::thread::Builder::new().name("discord".into()).spawn(move || run(&id, rx)).ok()?;
        Some(Self { tx, join: Some(join) })
    }

    /// `None` clears the status (paused / stopped).
    pub fn set(&self, card: Option<Card>) {
        let _ = self.tx.send(card);
    }

    /// The app is quitting: take the status down, then close the pipe. Discord keeps
    /// a presence up for a while after the app behind it goes away, so the clear has
    /// to actually be sent — the worker finishes it before this returns — and a
    /// Discord that's closed anyway was never showing anything to keep.
    pub fn quit(mut self) {
        let _ = self.tx.send(None);
        drop(self.tx);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn run(id: &str, rx: Receiver<Option<Card>>) {
    let mut client: Option<DiscordIpcClient> = None;
    let mut want: Option<Card> = None;
    let mut dirty = false;
    let mut next_try = Instant::now();
    loop {
        match rx.recv_timeout(RETRY) {
            Ok(p) => {
                // Let a burst (skip, skip, seek) settle, then send only the last state.
                std::thread::sleep(Duration::from_millis(400));
                want = rx.try_iter().last().unwrap_or(p);
                dirty = true;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if !dirty {
            continue;
        }
        if client.is_none() {
            if Instant::now() < next_try {
                continue;
            }
            let mut c = DiscordIpcClient::new(id);
            match c.connect() {
                Ok(()) => {
                    log::info!("discord: connected");
                    client = Some(c);
                }
                Err(_) => {
                    next_try = Instant::now() + RETRY;
                    continue;
                }
            }
        }
        let c = client.as_mut().expect("connected above");
        let res = match &want {
            Some(card) => c.set_activity(activity(card, false)),
            None => c.clear_activity(),
        };
        // Read Discord's reply: it reports rejected payloads there, and leaving
        // replies unread would fill the pipe over a long session.
        let mut res = res.and_then(|()| c.recv());
        if res.as_ref().is_ok_and(|(_, reply)| reply["evt"] == "ERROR")
            && let Some(card) = want.as_ref()
            && card.art.iter().chain(card.small_art.iter()).any(|picture| picture.fallback.is_some())
        {
            // Keep the text and timestamps if the desktop rejects a hosted image.
            res = c.set_activity(activity(card, true)).and_then(|()| c.recv());
        }
        match res {
            Ok((_, reply)) => {
                if reply["evt"] == "ERROR" {
                    log::warn!("discord rejected activity: {}", reply["data"]);
                } else {
                    log::debug!("discord: activity set: {}", reply["data"]);
                }
                dirty = false;
            }
            Err(e) => {
                log::warn!("discord: {e}; reconnecting");
                let _ = c.close();
                client = None;
                next_try = Instant::now() + RETRY;
            }
        }
    }
    if let Some(mut c) = client {
        let _ = c.close();
    }
}

fn activity(c: &Card, still: bool) -> Activity<'_> {
    let mut a = Activity::new().activity_type(match c.kind {
        Kind::Listening => ActivityType::Listening,
        Kind::Playing => ActivityType::Playing,
    }).status_display_type(match c.status {
        Status::Name => StatusDisplayType::Name,
        Status::Line1 => StatusDisplayType::Details,
        Status::Line2 => StatusDisplayType::State,
    });
    a = a.name(field(c.name()));
    if let Some(l) = &c.lines[0] {
        a = a.details(field(&l.text));
        if let Some(u) = &l.url {
            a = a.details_url(u.as_str());
        }
    }
    if let Some(l) = &c.lines[1] {
        a = a.state(field(&l.text));
        if let Some(u) = l.url.as_deref() {
            a = a.state_url(u);
        }
    }
    if let Some(art) = c.art.as_ref().and_then(|p| p.ipc_url(still)) {
        let mut assets = Assets::new().large_image(art);
        if let Some(text) = c.large_text() {
            assets = assets.large_text(field(text));
        }
        if let Some(u) = &c.art_url {
            assets = assets.large_url(u.as_str());
        }
        if let Some(small) = c.small_art.as_ref().and_then(|p| p.ipc_url(still)) {
            assets = assets.small_image(small);
            if let Some(text) = &c.small_text {
                assets = assets.small_text(field(text));
            }
        }
        a = a.assets(assets);
    }
    if let Some((start, end)) = c.times {
        let mut ts = Timestamps::new().start(start);
        if let Some(end) = end {
            ts = ts.end(end);
        }
        a = a.timestamps(ts);
    }
    a.buttons(c.buttons.iter().map(|(label, url)| Button::new(label.as_str(), url.as_str())).collect())
}

/// Discord rejects text fields shorter than 2 or longer than 128 characters. The
/// gateway payload carries the same fields, so the linked card clamps with it too.
pub(crate) fn field(s: &str) -> String {
    let mut out = s.to_owned();
    if out.encode_utf16().count() == 1 {
        out.push('\u{2800}');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song() -> Presence {
        Presence {
            video_id: "v1".into(),
            title: "Believer".into(),
            artist: "Imagine Dragons".into(),
            artist_id: Some("UC1".into()),
            album: Some("Evolve".into()),
            album_id: None,
            art: Some("https://img/1".into()),
            artist_art: None,
            quality: Some("Hi-Res Lossless · FLAC · 1641 kbps · 24-bit · Stereo".into()),
            duration_ms: 199_000,
            position_ms: 0,
            clock: (0, 0, 0),
            start_ms: 1000,
            end_ms: Some(200_000),
            paused: false,
            jam: None,
        }
    }

    fn card(song: &Presence, prefs: &Prefs) -> Card {
        Card::new(song, prefs, [song.art.as_deref().and_then(Picture::web), None])
    }

    const SONG_URL: &str = "https://music.youtube.com/watch?v=v1";
    const ARTIST_URL: &str = "https://music.youtube.com/channel/UC1";

    #[test]
    fn default_card_links_song_and_artist_and_shows_album_and_quality() {
        let prefs = Prefs::default();
        let p = song();
        let c = Card::new(&p, &prefs, [p.art.as_deref().and_then(Picture::web), Picture::web(ICON)]);
        assert_eq!(c.lines[0], Some(Line { text: "Believer".into(), url: Some(SONG_URL.into()) }));
        assert_eq!(c.lines[1], Some(Line { text: "Imagine Dragons".into(), url: Some(ARTIST_URL.into()) }));
        assert_eq!(c.lines[2].as_ref().map(|l| l.text.as_str()), Some("Evolve"));
        assert_eq!(c.annotation.as_deref(), p.quality.as_deref());
        assert_eq!(c.small_text.as_deref(), p.quality.as_deref());
        assert_eq!(prefs.images[1].mode, crate::discord_artwork::Mode::Logo);
        let wire = serde_json::to_value(activity(&c, false)).unwrap();
        assert_eq!(wire["name"], APP_NAME);
        assert_eq!(wire["state_url"], ARTIST_URL);
        assert_eq!(wire["state"], "Imagine Dragons");
        assert_eq!(wire["assets"]["large_text"], p.quality.as_deref().unwrap());
        assert_eq!(c.times, Some((1000, Some(200_000))));
        assert_eq!(c.buttons, vec![("Listen on YouTube Music".into(), SONG_URL.to_owned()), (PROJECT_BUTTON.into(), PROJECT_URL.to_owned())]);
        assert_eq!(c.status_text(), "Imagine Dragons");
        assert_eq!(c.member_text(), "Imagine Dragons · Believer");
        assert_eq!(c.name(), APP_NAME);
    }

    #[test]
    fn missing_quality_leaves_the_large_image_text() {
        let prefs = Prefs::default();
        let c = card(&Presence { quality: None, ..song() }, &prefs);
        assert!(c.annotation.is_none());
        assert_eq!(c.large_text(), Some("Evolve"));
        // A quality change (a hi-res upgrade landing mid-song) is a new card.
        let upgraded = card(&Presence { quality: Some("Dolby Atmos · E-AC-3 JOC · 768 kbps".into()), ..song() }, &prefs);
        assert!(!c.same(&upgraded, false));
    }

    #[test]
    fn hidden_details_leave_only_the_name() {
        let prefs = Prefs { hide_details: true, app_name: " Music ".into(), ..Prefs::default() };
        let c = card(&song(), &prefs);
        assert_eq!(c, Card { name: Some("Music".into()), status: Status::Name, ..Default::default() });
        assert_eq!(c.status_text(), "Music");
    }

    #[test]
    fn the_large_image_owns_the_third_line_and_hover() {
        let prefs = Prefs { art: false, ..Prefs::default() };
        let c = card(&song(), &prefs);
        assert!(c.lines[2].is_none() && c.art.is_none());
        assert!(c.annotation.is_none());
    }

    #[test]
    fn button_goes_out_within_discords_limits() {
        let a = serde_json::to_value(activity(&card(&song(), &Prefs::default()), false)).unwrap();
        assert_eq!(a["buttons"], serde_json::json!([{ "label": "Listen on YouTube Music", "url": SONG_URL }, { "label": PROJECT_BUTTON, "url": PROJECT_URL }]));
        for page in Page::ALL {
            assert!(page.button().chars().count() <= 32, "{page:?}");
        }
        // Switched off, none are sent: Discord turns down an empty list.
        let c = card(&song(), &Prefs { show_buttons: false, ..Prefs::default() });
        assert!(c.buttons.is_empty());
        assert!(serde_json::to_value(activity(&c, false)).unwrap()["buttons"].is_null());
    }

    #[test]
    fn same_card_allows_clock_drift_not_seeks_or_edits() {
        let prefs = Prefs::default();
        let c = card(&song(), &prefs);
        let at = |start_ms: i64| card(&Presence { start_ms, end_ms: song().end_ms.map(|e| e + start_ms - song().start_ms), ..song() }, &prefs);
        assert!(c.same(&at(song().start_ms + 150), false));
        assert!(!c.same(&at(song().start_ms + 5_000), false));
        assert!(!c.same(&card(&song(), &Prefs { app_name: "Mine".into(), ..Prefs::default() }), false));
        // Paused, the clock doesn't show.
        let paused = |start_ms: i64| card(&Presence { paused: true, start_ms, ..song() }, &prefs);
        assert!(paused(0).same(&paused(60_000), false));
    }

    #[test]
    fn paused_drops_the_progress_bar() {
        let c = card(&Presence { paused: true, ..song() }, &Prefs::default());
        assert_eq!(c.times, None);
        assert!(card(&song(), &Prefs::default()).times.is_some());
    }

    #[test]
    fn pause_says_so_on_the_second_line() {
        let p = Presence { paused: true, ..song() };
        let prefs = Prefs::default();
        let c = card(&p, &prefs);
        assert_eq!(c.lines[1].as_ref().map(|l| l.text.as_str()), Some("Paused"));
        assert_eq!(c.lines[1].clone().and_then(|l| l.url), None);
        assert_eq!(c.member_text(), "Believer · Paused");
        // The song and the quality mark stay where they were.
        assert_eq!(c.lines[0].as_ref().map(|l| l.text.as_str()), Some("Believer"));
        assert!(c.lines[2].is_some());
        assert_eq!(c.annotation, p.quality);
        assert!(c.times.is_none());
        let custom = Prefs { lines: [Show::Title, Show::Template, Show::Album], templates: ["{song}".into(), "{status} · {artist}".into()], ..prefs };
        assert_eq!(card(&p, &custom).lines[1].as_ref().map(|l| l.text.as_str()), Some("Paused · Imagine Dragons"));
    }

    #[test]
    fn jam_button_only_in_a_room() {
        let prefs = Prefs { buttons: [Page::Jam, Page::None], ..Prefs::default() };
        assert!(card(&song(), &prefs).buttons.is_empty());
        let link = "https://bitchord.example/invite/ABC234";
        let c = card(&Presence { jam: Some(link.into()), ..song() }, &prefs);
        assert_eq!(c.buttons, vec![("Listen together".into(), link.to_owned())]);
    }

    #[test]
    fn links_and_buttons_skip_missing_pages() {
        let prefs = Prefs {
            lines: [Show::Album, Show::Artist, Show::Title],
            links: [true, true],
            art_link: true,
            buttons: [Page::Album, Page::Artist],
            ..Prefs::default()
        };
        let c = card(&song(), &prefs);
        // No album id: the album line isn't a link, and the artwork opens the song.
        assert_eq!(c.lines[0].as_ref().and_then(|l| l.url.clone()), None);
        assert_eq!(c.lines[1].as_ref().and_then(|l| l.url.as_deref()), Some(ARTIST_URL));
        assert_eq!(c.art_url.as_deref(), Some(SONG_URL));
        assert_eq!(c.buttons, vec![("View artist".into(), ARTIST_URL.to_owned())]);
    }

    #[test]
    fn same_button_twice_shows_once() {
        let prefs = Prefs { buttons: [Page::Song, Page::Song], ..Prefs::default() };
        assert_eq!(card(&song(), &prefs).buttons.len(), 1);
    }

    #[test]
    fn empty_status_line_falls_back_to_the_name() {
        let prefs = Prefs { status: Status::Line1, lines: [Show::Nothing, Show::Artist, Show::Album], ..Prefs::default() };
        let c = card(&song(), &prefs);
        assert_eq!((c.status, c.status_text()), (Status::Name, APP_NAME));
    }

    #[test]
    fn prefs_saved_before_a_field_existed_still_load() {
        let p: Prefs = serde_json::from_str(r#"{"when_paused":true}"#).unwrap();
        assert_eq!(p, Prefs { when_paused: true, ..Prefs::default() });
    }

    #[test]
    fn old_default_third_line_migrates_to_quality_only() {
        let prefs = Prefs::from_saved(r#"{"annotation":"{status} · {quality}"}"#).unwrap();
        assert_eq!(prefs.annotation, "{quality}");
        let custom = Prefs::from_saved(r#"{"annotation":"{album}"}"#).unwrap();
        assert_eq!(custom.annotation, "{album}");
    }

    #[test]
    fn quality_summary_says_the_tier_the_bitrate_and_the_rate() {
        assert_eq!(quality_summary("Lossless", Some(1411), Some(44_100)), "Lossless · 1411 kbps · 44.1 kHz");
        assert_eq!(quality_summary("High", Some(133), Some(48_000)), "High · 133 kbps · 48 kHz");
        assert_eq!(quality_summary("Hi-Res Lossless", Some(2304), Some(96_000)), "Hi-Res Lossless · 2304 kbps · 96 kHz");
        assert_eq!(quality_summary("High", None, Some(22_050)), "High · 22.05 kHz");
        assert_eq!(quality_summary("High", Some(256), None), "High · 256 kbps");
    }

    #[test]
    fn saved_templates_migrate_to_song_and_keep_overrides() {
        let json = r#"{"app_name":"{name} on {app}","templates":["{name}","{artist}"],"links":[false,false],"button_labels":["Find {name}",null]}"#;
        let prefs = Prefs::from_saved(json).unwrap();
        assert_eq!(prefs.app_name, "{song} on {app}");
        assert_eq!(prefs.templates[0], "{song}");
        assert_eq!(prefs.button_labels[0].as_deref(), Some("Find {song}"));
        assert_eq!(prefs.links, [false, false]);
    }

    #[test]
    fn blank_small_hover_uses_status_and_quality() {
        let p = song();
        let mut prefs = Prefs::default();
        prefs.images[1].hover = " ".into();
        let c = Card::new(&p, &prefs, [Picture::web(ICON), Some(Picture::default())]);
        assert_eq!(c.small_text.as_deref(), Some("Playing · Hi-Res Lossless · FLAC · 1641 kbps · 24-bit · Stereo"));
        let local = serde_json::to_value(activity(&c, false)).unwrap();
        assert!(local["assets"]["small_image"].is_null());
        assert!(local["assets"]["small_text"].is_null());
    }

    #[test]
    fn the_old_link_options_carry_over() {
        // A settings file saved by the old Discord: what the linked card's wording
        // becomes, and that a missing field reads as the default it had.
        let old = r#"{ "activity_type": "listening", "use_details": true, "activity_name": "My app", "button1_visible": true, "button2_visible": false }"#;
        let p = Prefs::migrated(old);
        assert_eq!(p, Prefs {
            app_name: "My app".into(),
            status: Status::Line1,
            buttons: [Page::Song, Page::None],
            ..Prefs::default()
        });
        // The old defaults: artist-first, both buttons.
        let p = Prefs::migrated(r#"{}"#);
        assert_eq!((p.status, p.buttons), (Status::Line2, [Page::Song, Page::Project]));
        assert_eq!(Prefs::migrated("not json"), Prefs::default());
    }

    #[test]
    fn templates_keep_metadata_literal_and_drop_unknown_values() {
        let p = Presence { title: "{artist} is a song title".into(), album: None, quality: None, paused: true, ..song() };
        assert_eq!(
            p.render(" {song} \n {album} {unrecognized} {quality} {duration} {status} {app} ", 128).as_deref(),
            Some("{artist} is a song title 3:19 Paused BitChord PC")
        );
        assert_eq!(p.render("{album} {nope}", 128), None);
        assert_eq!(p.render("{name}", 128), p.render("{song}", 128));
        assert_eq!(field(&p.render(" X ", 128).unwrap()), "X\u{2800}");
        let text = p.render(&"🎵".repeat(100), 128).unwrap();
        assert!(text.ends_with('…'));
        assert!(text.encode_utf16().count() <= 128);
    }

    #[test]
    fn customized_settings_round_trip() {
        let mut p = Prefs {
            kind: Kind::Playing,
            app_name: "{artist} on {app}".into(),
            buttons: [Page::Spotify, Page::Custom],
            button_labels: [Some("Find {name}".into()), Some(String::new())],
            button_urls: [String::new(), "https://example.org".into()],
            ..Prefs::default()
        };
        p.images[0].mode = crate::discord_artwork::Mode::Either;
        p.images[1].mode = crate::discord_artwork::Mode::Status;
        p.artwork.host = true;
        assert_eq!(serde_json::from_str::<Prefs>(&serde_json::to_string(&p).unwrap()).unwrap(), p);
        let c = card(&song(), &p);
        assert_eq!(c.buttons, vec![("Find Believer".into(), "https://open.spotify.com/search/Believer%20Imagine%20Dragons".into())]);
        assert_eq!(c.times, Some((1000, None)));
    }

    #[test]
    fn ipc_activity_carries_both_assets_and_falls_back_to_stills() {
        let mut p = Prefs {
            kind: Kind::Playing,
            app_name: "{artist} on {app}".into(),
            status: Status::Line2,
            lines: [Show::Template, Show::Template, Show::Template],
            templates: ["{song} • {duration}".into(), "{status}".into()],
            annotation: String::new(),
            art_link: true,
            buttons: [Page::Custom, Page::None],
            button_labels: [Some("Listen to {name}".into()), None],
            button_urls: ["https://example.com/listen".into(), String::new()],
            ..Prefs::default()
        };
        p.images[0].hover = "{album}".into();
        p.images[1].hover = "{status}".into();
        let mut large = Picture::web("https://example.com/cover.gif").unwrap();
        large.fallback = song().art;
        let small = Picture::web(ICON).unwrap();
        let pictures = [Some(large.clone()), Some(small.clone())];
        let c = Card::new(&song(), &p, pictures);
        let json = serde_json::to_value(activity(&c, false)).unwrap();
        assert_eq!(json["type"], 0);
        assert_eq!(json["name"], "Imagine Dragons on BitChord PC");
        assert_eq!(json["details"], "Believer • 3:19");
        assert_eq!(json["state"], "Playing");
        assert_eq!(json["assets"]["large_text"], "Evolve");
        assert_eq!(json["assets"]["large_url"], SONG_URL);
        assert_eq!(json["assets"]["small_image"], ICON);
        assert_eq!(json["assets"]["small_text"], "Playing");
        assert_eq!(json["buttons"][0]["url"], "https://example.com/listen");
        assert_eq!(json["timestamps"], serde_json::json!({ "start": 1000 }));
        let fallback = serde_json::to_value(activity(&c, true)).unwrap();
        assert_eq!(fallback["assets"]["large_image"], "https://img/1");
        assert_eq!(fallback["assets"]["small_image"], ICON);
        assert!(Card::new(&song(), &p, [None, Some(small)]).small_art.is_none());
        p.hide_details = true;
        let hidden = Card::new(&song(), &p, [Some(large), None]);
        assert_eq!(hidden.name(), APP_NAME);
        assert!(hidden.art.is_none());
    }

    #[test]
    fn explicit_seek_and_speed_changes_update_even_with_nearly_identical_times() {
        let p = song();
        let prefs = Prefs::default();
        let initial = card(&p, &prefs);
        let sought = card(&Presence { clock: (0, 1, 0), start_ms: p.start_ms - 200, ..p.clone() }, &prefs);
        let faster = card(&Presence { clock: (0, 1, 1.25f32.to_bits()), ..p.clone() }, &prefs);
        let next = card(&Presence { clock: (1, 0, 0), ..p.clone() }, &prefs);
        let paused = card(&Presence { paused: true, ..p }, &prefs);
        assert!(!initial.same(&sought, false));
        assert!(!sought.same(&faster, false));
        assert!(!initial.same(&next, false));
        assert!(!initial.same(&paused, false));
    }

    #[test]
    fn custom_links_reject_invalid_or_credentialed_urls() {
        for url in ["javascript:example", "file:///tmp/song", "https://a@site.example", "https://site.example/a b"] {
            assert!(web_url(url, 512).is_none());
        }
        assert_eq!(web_url(" https://site.example/a ", 32).as_deref(), Some("https://site.example/a"));
    }

    #[test]
    fn updates_obey_interval_and_burst_budget() {
        let now = Instant::now();
        let mut updates = Updates::default();
        assert!(updates.delay(now, 3, false).is_zero());
        for offset in 0..5 {
            updates.record(now + Duration::from_secs(offset));
        }
        assert_eq!(updates.delay(now + Duration::from_secs(5), 3, false), Duration::from_secs(15));
        assert!(updates.delay(now + Duration::from_secs(20), 3, false).is_zero());
    }

    #[test]
    fn badge_progress_leaves_room_for_pause_and_resume() {
        let large = song().art.as_deref().and_then(Picture::web);
        let with_badge = |step, paused| {
            let mut small = Picture::web(&format!("https://img/{step}-{paused}.png")).unwrap();
            small.badge = Some((step, paused));
            Card::new(&song(), &Prefs::default(), [large.clone(), Some(small)])
        };
        let c = with_badge(1, false);
        assert!(!c.same(&with_badge(2, false), false));
        assert!(c.same(&with_badge(2, false), true));
        assert!(!c.same(&with_badge(2, true), true));

        let now = Instant::now();
        let mut updates = Updates::default();
        for offset in [0, 2, 4] {
            updates.record(now + Duration::from_secs(offset));
        }
        let next = now + Duration::from_secs(6);
        assert!(updates.delay(next, 1, false).is_zero());
        assert_eq!(updates.delay(next, 1, true), Duration::from_secs(14));
    }
}
