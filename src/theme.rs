//! Visual design tokens: fonts, icons, colours, motion.

use std::sync::Arc;

use egui::{Color32, FontData, FontDefinitions, FontFamily, FontId};

// ------------------------------------------------------------------ colours

/// Base surface under the animated background (seen while it fades in).
const BG: Color32 = Color32::from_rgb(12, 10, 14);
const TEXT: Color32 = Color32::from_rgb(242, 240, 245);
const TEXT_DIM: Color32 = Color32::from_rgb(178, 172, 186);
const TEXT_FAINT: Color32 = Color32::from_rgb(130, 124, 140);
/// Light mode's counterparts.
const BG_LIGHT: Color32 = Color32::from_rgb(246, 244, 248);
const TEXT_LIGHT: Color32 = Color32::from_rgb(22, 20, 26);
const TEXT_DIM_LIGHT: Color32 = Color32::from_rgb(86, 82, 94);
const TEXT_FAINT_LIGHT: Color32 = Color32::from_rgb(124, 118, 132);
/// Default accent until a cover provides one (BitChord pink).
pub const RED: Color32 = Color32::from_rgb(0xFA, 0x58, 0x6A);
/// A check's colour: source Tests and other "that worked" lines (Apple's system green).
pub const GREEN: Color32 = Color32::from_rgb(0x30, 0xD1, 0x58);

/// Light or dark, as the app picked it for what's being drawn (see `set_mode`): the
/// colours below follow it. Now playing always draws dark, over the album art.
static LIGHT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn light() -> bool {
    LIGHT.load(std::sync::atomic::Ordering::Relaxed)
}

/// Draw what follows light or dark: the colours here and egui's style (menus, fields,
/// text) change together. Returns the mode before, to go back to.
pub fn set_mode(ctx: &egui::Context, light: bool) -> bool {
    let was = LIGHT.swap(light, std::sync::atomic::Ordering::Relaxed);
    if was != light {
        ctx.global_style_mut(|s| s.visuals = visuals());
    }
    was
}

/// Main text.
pub fn text() -> Color32 {
    if light() { TEXT_LIGHT } else { TEXT }
}

/// Text and icons on a fill of the main text colour (a primary button, a selected chip).
pub fn on_text() -> Color32 {
    if light() { Color32::WHITE } else { Color32::from_gray(15) }
}

/// Relative luminance of an opaque colour, as WCAG words it.
fn luminance(c: Color32) -> f32 {
    let lin = |v: u8| {
        let v = v as f32 / 255.0;
        if v <= 0.040_45 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
}

/// WCAG contrast between two opaque colours.
fn contrast(a: Color32, b: Color32) -> f32 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// Text and icons on a fill of this colour: dark or white, whichever reads better.
pub fn on_fill(fill: Color32) -> Color32 {
    let dark = Color32::from_gray(15);
    if contrast(dark, fill) > contrast(Color32::WHITE, fill) { dark } else { Color32::WHITE }
}

/// Least important text: counts, hints, placeholders.
pub fn text_faint() -> Color32 {
    if light() { TEXT_FAINT_LIGHT } else { TEXT_FAINT }
}

/// The window's base colour, under the background.
pub fn bg() -> Color32 {
    if light() { BG_LIGHT } else { BG }
}

/// Menus and popups.
pub fn menu_fill() -> Color32 {
    if light() { Color32::WHITE } else { Color32::from_rgb(38, 34, 42) }
}

/// Dialogs and cards that float over the page.
pub fn surface_fill() -> Color32 {
    if light() { Color32::from_rgb(252, 251, 253) } else { Color32::from_rgb(28, 24, 32) }
}

/// Theme preset multiplier for every glass surface (f32 bits; 1.0 by default).
static GLASS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0x3f80_0000);

/// White at `a` opacity, for glassy surfaces over the background (black in light mode).
pub fn glass(a: f32) -> Color32 {
    let k = f32::from_bits(GLASS.load(std::sync::atomic::Ordering::Relaxed));
    let a = ((a * k).min(1.0) * 255.0) as u8;
    if light() { Color32::from_black_alpha(a) } else { Color32::from_white_alpha(a) }
}

/// A panel over the background (sidebar, player bar, queue): darker than the page at
/// `a`, or in light mode lighter.
pub fn panel(a: f32) -> Color32 {
    if light() { Color32::from_white_alpha((a.clamp(0.0, 1.0) * 255.0) as u8) } else { shade(a) }
}

/// The active theme's secondary text colour, set with its chrome (read very often).
static DIM: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(u32::from_le_bytes(TEXT_DIM.to_array()));

/// Secondary text colour (themes may tint it).
pub fn text_dim() -> Color32 {
    if light() {
        return TEXT_DIM_LIGHT;
    }
    let [r, g, b, a] = DIM.load(std::sync::atomic::Ordering::Relaxed).to_le_bytes();
    Color32::from_rgba_premultiplied(r, g, b, a)
}

/// Light, dark, or whichever the system uses (Settings › Appearance › Mode).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

impl Appearance {
    pub const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];

    pub fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    pub fn from_key(s: &str) -> Self {
        Self::ALL.into_iter().find(|a| a.key() == s).unwrap_or_default()
    }

    /// Whether this means light, given the system's mode as eframe reports it.
    pub fn is_light(self, ctx: &egui::Context) -> bool {
        match self {
            Self::Light => true,
            Self::Dark => false,
            Self::System => system_light().unwrap_or_else(|| ctx.system_theme() == Some(egui::Theme::Light)),
        }
    }
}

/// Settings › Appearance › Visual preset: a bundle of the visual settings beneath it,
/// for older computers (Barebones) or everything at once (High visual). Custom is
/// whatever the listener set by hand — also what a bundle becomes the moment one of
/// its settings is changed.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum VisualPreset {
    #[default]
    Custom,
    Barebones,
    High,
}

impl VisualPreset {
    pub const ALL: [Self; 3] = [Self::Custom, Self::Barebones, Self::High];

    pub fn label(self) -> &'static str {
        match self {
            Self::Custom => "Custom",
            Self::Barebones => "Barebones",
            Self::High => "High visual",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Custom => "custom",
            Self::Barebones => "barebones",
            Self::High => "high",
        }
    }

    pub fn from_key(s: &str) -> Self {
        Self::ALL.into_iter().find(|v| v.key() == s).unwrap_or_default()
    }
}

/// Windows' "app mode" (Settings › Personalization › Colors), read again every couple of
/// seconds so switching it there shows without a restart. `None` when it can't be told.
#[cfg(windows)]
fn system_light() -> Option<bool> {
    use std::time::{Duration, Instant};
    static MODE: std::sync::Mutex<Option<(Instant, Option<bool>)>> = std::sync::Mutex::new(None);
    let mut cached = MODE.lock().unwrap();
    match *cached {
        Some((at, m)) if at.elapsed() < Duration::from_secs(2) => m,
        _ => {
            let m = read_system_light();
            *cached = Some((Instant::now(), m));
            m
        }
    }
}

#[cfg(windows)]
fn read_system_light() -> Option<bool> {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    use windows::core::w;
    let mut v = 0u32;
    let mut size = 4u32;
    // SAFETY: a DWORD out-buffer of the size passed; the strings are static.
    unsafe {
        RegGetValueW(HKEY_CURRENT_USER, w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"), w!("AppsUseLightTheme"), RRF_RT_REG_DWORD, None, Some(&mut v as *mut u32 as *mut _), Some(&mut size))
            .is_ok()
            .then_some(v != 0)
    }
}

/// The Linux desktop's light or dark style, as `follow_system_scheme` last heard it.
#[cfg(target_os = "linux")]
fn system_light() -> Option<bool> {
    match SCHEME.load(std::sync::atomic::Ordering::Relaxed) {
        1 => Some(false),
        2 => Some(true),
        _ => None,
    }
}

/// 0: no answer (yet), 1: dark, 2: light.
#[cfg(target_os = "linux")]
static SCHEME: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

#[cfg(target_os = "linux")]
const SETTINGS: &str = "org.freedesktop.portal.Settings";
#[cfg(target_os = "linux")]
const APPEARANCE: (&str, &str) = ("org.freedesktop.appearance", "color-scheme");

/// Follow the desktop's light or dark style, from the settings portal's `color-scheme`
/// (GNOME, KDE and others): the window can't tell on Wayland. A thread of its own hears
/// each change the moment the desktop makes it and `wake`s the UI to draw it (an idle
/// window draws nothing otherwise). Waits a moment for the first answer, so the first
/// frame already has the desktop's style.
#[cfg(target_os = "linux")]
pub fn follow_system_scheme(wake: impl Fn() + Copy + Send + 'static) {
    use dbus::arg::{RefArg, Variant};
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};
    let gnome = std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| d.split(':').any(|d| d.eq_ignore_ascii_case("GNOME")));
    let set = move |v: Option<u64>| {
        let s = scheme(v, gnome);
        if SCHEME.swap(s, Ordering::Relaxed) != s {
            wake();
        }
    };
    let (asked, answer) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new().name("color-scheme".into()).spawn(move || {
        let Ok(bus) = dbus::blocking::Connection::new_session() else { return };
        let portal = bus.with_proxy("org.freedesktop.portal.Desktop", "/org/freedesktop/portal/desktop", Duration::from_secs(2));
        // Listening before the first read, so a change between the two isn't missed.
        let rule = dbus::message::MatchRule::new_signal(SETTINGS, "SettingChanged").with_path("/org/freedesktop/portal/desktop");
        let heard = bus
            .add_match(rule, move |(namespace, key, v): (String, String, Variant<Box<dyn RefArg>>), _, _| {
                if (namespace.as_str(), key.as_str()) == APPEARANCE {
                    set(variant_number(&v));
                }
                true
            })
            .is_ok();
        let first = portal_color_scheme(&portal);
        set(first);
        let _ = asked.send(());
        // No portal, or one without the setting: nothing more to learn from it.
        if first.is_none() {
            return;
        }
        // Asked again now and then too: a portal that restarts doesn't tell what changed
        // while it was gone. Every couple of seconds if it can't be listened to.
        let every = Duration::from_secs(if heard { 30 } else { 2 });
        let mut read = Instant::now();
        loop {
            match every.checked_sub(read.elapsed()) {
                Some(left) => {
                    if bus.process(left).is_err() {
                        return;
                    }
                }
                None => {
                    if let Some(v) = portal_color_scheme(&portal) {
                        set(Some(v));
                    }
                    read = Instant::now();
                }
            }
        }
    });
    if spawned.is_ok() {
        let _ = answer.recv_timeout(Duration::from_millis(250));
    }
}

/// `SCHEME` for the portal's `color-scheme`: 0 no preference, 1 dark, 2 light. GNOME's
/// light style is its "default", which elsewhere means no preference.
#[cfg(target_os = "linux")]
fn scheme(color_scheme: Option<u64>, gnome: bool) -> u8 {
    match color_scheme {
        Some(1) => 1,
        Some(2) => 2,
        Some(0) if gnome => 2,
        _ => 0,
    }
}

/// `org.freedesktop.appearance` `color-scheme`. Older portals have only `Read`, which
/// wraps the value in a second variant.
#[cfg(target_os = "linux")]
fn portal_color_scheme(portal: &dbus::blocking::Proxy<&dbus::blocking::Connection>) -> Option<u64> {
    use dbus::arg::{RefArg, Variant};
    let (v,): (Variant<Box<dyn RefArg>>,) = portal.method_call(SETTINGS, "ReadOne", APPEARANCE).or_else(|_| portal.method_call(SETTINGS, "Read", APPEARANCE)).ok()?;
    variant_number(&v)
}

/// The number in a variant, however many variants deep.
#[cfg(target_os = "linux")]
fn variant_number(a: &dyn dbus::arg::RefArg) -> Option<u64> {
    a.as_u64().or_else(|| a.as_iter()?.next().and_then(variant_number))
}

#[cfg(not(any(windows, target_os = "linux")))]
fn system_light() -> Option<bool> {
    None
}

/// The Windows accent colour, in the lighter shade Windows itself uses on dark
/// surfaces (`AccentPalette`'s second entry), or the base accent if there's no palette.
/// Read again every couple of seconds, so a new colour picked in Windows' settings
/// shows without a restart. `None` off Windows or when the setting can't be read.
pub fn system_accent() -> Option<Color32> {
    use std::time::{Duration, Instant};
    static ACCENT: std::sync::Mutex<Option<(Instant, Option<Color32>)>> = std::sync::Mutex::new(None);
    let mut cached = ACCENT.lock().unwrap();
    match *cached {
        Some((at, c)) if at.elapsed() < Duration::from_secs(2) => c,
        _ => {
            let c = read_system_accent();
            *cached = Some((Instant::now(), c));
            c
        }
    }
}

#[cfg(windows)]
fn read_system_accent() -> Option<Color32> {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_BINARY, RRF_RT_REG_DWORD, RegGetValueW};
    use windows::core::w;
    // SAFETY: out-buffers sized to what we pass; the strings are static.
    unsafe {
        // Eight RGBA swatches, lightest first; the second is "light 2".
        let mut palette = [0u8; 32];
        let mut size = palette.len() as u32;
        let key = w!(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Accent");
        if RegGetValueW(HKEY_CURRENT_USER, key, w!("AccentPalette"), RRF_RT_REG_BINARY, None, Some(palette.as_mut_ptr() as *mut _), Some(&mut size)).is_ok() && size >= 8 {
            return Some(Color32::from_rgb(palette[4], palette[5], palette[6]));
        }
        // 0xAABBGGRR.
        let mut abgr = 0u32;
        let mut size = 4u32;
        RegGetValueW(HKEY_CURRENT_USER, w!(r"Software\Microsoft\Windows\DWM"), w!("AccentColor"), RRF_RT_REG_DWORD, None, Some(&mut abgr as *mut u32 as *mut _), Some(&mut size))
            .is_ok()
            .then(|| Color32::from_rgb(abgr as u8, (abgr >> 8) as u8, (abgr >> 16) as u8))
    }
}

#[cfg(not(windows))]
fn read_system_accent() -> Option<Color32> {
    None
}

/// Corner radius of album/playlist art in cards.
pub fn card_radius() -> f32 {
    crate::preset::active().app.card_radius.clamp(0.0, 40.0)
}

/// Corner radius of small song-row thumbnails.
pub fn thumb_radius() -> f32 {
    crate::preset::active().app.thumb_radius.clamp(0.0, 20.0)
}

/// Corner radius of panels (settings sections, menus, dialogs).
pub fn panel_radius() -> u8 {
    crate::preset::active().app.panel_radius.clamp(0.0, 32.0) as u8
}

/// Strength of card hover zoom/lift.
pub fn hover_k() -> f32 {
    crate::preset::active().app.hover.clamp(0.0, 2.5)
}

pub fn shade(a: f32) -> Color32 {
    Color32::from_black_alpha((a * 255.0) as u8)
}

/// Scale a colour's opacity by `a` (works for opaque and already-translucent colours).
pub fn with_alpha(c: Color32, a: f32) -> Color32 {
    c.gamma_multiply(a.clamp(0.0, 1.0))
}

/// Interpolate in premultiplied space (Color32's native form), so translucent glass
/// colours blend correctly with opaque ones.
pub fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_premultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
}

// ------------------------------------------------------------------ motion

pub fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

pub fn ease_in_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 { 4.0 * t * t * t } else { 1.0 - (-2.0 * t + 2.0).powi(3) / 2.0 }
}

/// Every UI transition runs at this fraction of the time it's written with: one knob
/// for how snappy the app feels. (Lyrics keep their own timing.)
pub const SPEED: f32 = 0.65;

/// Reduce motion (Settings › Appearance): 1.0 normally, 0.0 when every transition
/// should land at once. egui takes a zero animation time as "no transition".
static MOTION: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1.0f32.to_bits());

pub fn set_motion(on: bool) {
    MOTION.store((on as u32 as f32).to_bits(), std::sync::atomic::Ordering::Relaxed);
}

/// The fraction transitions run at: [`SPEED`], or nothing at all with Reduce motion on.
pub fn speed() -> f32 {
    SPEED * f32::from_bits(MOTION.load(std::sync::atomic::Ordering::Relaxed))
}

/// Animate toward `target` (eased), keyed by `id`. Requests repaints while moving.
pub fn anim(ctx: &egui::Context, id: impl std::hash::Hash + std::fmt::Debug, target: f32, secs: f32) -> f32 {
    ctx.animate_value_with_time(egui::Id::unique(id), target, secs * speed())
}

pub fn anim_bool(ctx: &egui::Context, id: impl std::hash::Hash + std::fmt::Debug, on: bool, secs: f32) -> f32 {
    ease_out(ctx.animate_bool_with_time(egui::Id::unique(id), on, secs * speed()))
}

/// [`anim_bool`] for a transition whose length the theme sets itself, so `ui_fade_secs`
/// leaves it alone (BitChord's `anim_bool` carries no such fade yet: the two agree).
pub fn anim_bool_timed(ctx: &egui::Context, id: impl std::hash::Hash + std::fmt::Debug, on: bool, secs: f32) -> f32 {
    ease_out(ctx.animate_bool_with_time(egui::Id::unique(id), on, secs * speed()))
}

/// [`anim_bool`] that eases in and out, for a fold that opens and closes (Discord's
/// settings): the change of direction should never read as a snap.
pub fn anim_bool_soft(ctx: &egui::Context, id: impl std::hash::Hash + std::fmt::Debug, on: bool, secs: f32) -> f32 {
    ease_in_out(ctx.animate_bool_with_time(egui::Id::unique(id), on, secs * speed()))
}

// ------------------------------------------------------------------ type

pub fn regular(size: f32) -> FontId {
    FontId::proportional(size)
}
// Named families, made once: these are called hundreds of times a frame, and a
// clone is a reference count instead of a new string.
static SEMIBOLD: std::sync::LazyLock<FontFamily> = std::sync::LazyLock::new(|| FontFamily::Name("semibold".into()));
static BOLD: std::sync::LazyLock<FontFamily> = std::sync::LazyLock::new(|| FontFamily::Name("bold".into()));
static ICONS: std::sync::LazyLock<FontFamily> = std::sync::LazyLock::new(|| FontFamily::Name("icons".into()));

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, SEMIBOLD.clone())
}
pub fn bold(size: f32) -> FontId {
    FontId::new(size, BOLD.clone())
}
pub fn icon_font(size: f32) -> FontId {
    FontId::new(size, ICONS.clone())
}

/// Lucide icons (ISC), subset to the glyphs used here.
pub mod icon {
    pub const HOUSE: char = '\u{e0f5}';
    pub const COMPASS: char = '\u{e09b}';
    pub const LIBRARY: char = '\u{e100}';
    pub const SEARCH: char = '\u{e151}';
    pub const CHEVRON_LEFT: char = '\u{e06e}';
    pub const CHEVRON_RIGHT: char = '\u{e06f}';
    pub const CHEVRON_UP: char = '\u{e070}';
    pub const CHEVRON_DOWN: char = '\u{e06d}';
    pub const PLAY: char = '\u{e13c}';
    pub const PAUSE: char = '\u{e12e}';
    pub const SKIP_BACK: char = '\u{e15f}';
    pub const SKIP_FORWARD: char = '\u{e160}';
    pub const SHUFFLE: char = '\u{e15e}';
    pub const REPEAT: char = '\u{e146}';
    pub const REPEAT_1: char = '\u{e1fd}';
    pub const VOLUME_2: char = '\u{e1ab}';
    pub const VOLUME_1: char = '\u{e1aa}';
    pub const VOLUME_X: char = '\u{e1ac}';
    pub const HEART: char = '\u{e0f2}';
    pub const LIST_MUSIC: char = '\u{e2e0}';
    pub const MIC_VOCAL: char = '\u{e349}';
    pub const X: char = '\u{e1b2}';
    pub const HISTORY: char = '\u{e1f5}';
    pub const TRASH_2: char = '\u{e18e}';
    pub const ELLIPSIS_VERTICAL: char = '\u{e0b7}';
    pub const LIST_PLUS: char = '\u{e23f}';
    pub const LIST_MINUS: char = '\u{e23e}';
    pub const LIST_CHECKS: char = '\u{e1d0}';
    pub const RADIO: char = '\u{e142}';
    pub const USER: char = '\u{e19f}';
    pub const SETTINGS: char = '\u{e154}';
    pub const LOG_OUT: char = '\u{e10e}';
    pub const MUSIC: char = '\u{e122}';
    pub const SPARKLES: char = '\u{e412}';
    pub const CHECK: char = '\u{e06c}';
    pub const CIRCLE_USER: char = '\u{e461}';
    pub const EXPAND: char = '\u{e21a}';
    pub const SHRINK: char = '\u{e220}';
    pub const UPLOAD: char = '\u{e19e}';
    pub const USERS: char = '\u{e1a4}';
    pub const COPY: char = '\u{e09e}';
    pub const LINK: char = '\u{e102}';
    pub const CROWN: char = '\u{e1d6}';
    pub const HEADPHONES: char = '\u{e0f1}';
    pub const DOWNLOAD: char = '\u{e0b2}';
    pub const FOLDER_OPEN: char = '\u{e247}';
    pub const PIN: char = '\u{e259}';
    pub const PIN_OFF: char = '\u{e2b6}';
    pub const MAXIMIZE_2: char = '\u{e113}';
    pub const PICTURE_IN_PICTURE: char = '\u{e3af}';
}

/// Inter (SIL OFL 1.1), the default face: regular / semibold / extra bold, subset to
/// Latin (with Vietnamese), Greek and Cyrillic, so those scripts never fall back to
/// another font letter by letter.
const INTER: [&[u8]; 3] = [
    include_bytes!("../assets/fonts/Inter-Regular.ttf"),
    include_bytes!("../assets/fonts/Inter-SemiBold.ttf"),
    include_bytes!("../assets/fonts/Inter-ExtraBold.ttf"),
];
// Noto Sans Devanagari v2.007 (SIL OFL 1.1), the upstream weight-only variable font.
const NOTO_DEVANAGARI: &[u8] = include_bytes!("../assets/fonts/NotoSansDevanagari.ttf");
const FALLBACK_WEIGHTS: [f32; 3] = [400.0, 600.0, 750.0];
const LUCIDE: &[u8] = include_bytes!("../assets/fonts/lucide.ttf");

/// Lyric typefaces used by theme presets, as static regular / semibold / bold
/// instances. They cover Latin only (their upstream has no Vietnamese, Greek or
/// Cyrillic); `lyric_font_for` sets such lines in Inter instead.
/// Instrument Sans is only ever used at semibold (one preset), so its bold slot reuses
/// that instead of shipping a weight nothing draws.
const LYRIC_FONTS: &[(&str, [&[u8]; 3])] = &[
    ("sfpro", [
        include_bytes!("../assets/fonts/SFProDisplay-Regular.otf"),
        include_bytes!("../assets/fonts/SFProDisplay-SemiBold.otf"),
        include_bytes!("../assets/fonts/SFProDisplay-Bold.otf"),
    ]),
    ("dmsans", [
        include_bytes!("../assets/fonts/DMSans-Regular.ttf"),
        include_bytes!("../assets/fonts/DMSans-SemiBold.ttf"),
        include_bytes!("../assets/fonts/DMSans-Bold.ttf"),
    ]),
    ("googlesansflex", [
        include_bytes!("../assets/fonts/GoogleSansFlex-Regular.ttf"),
        include_bytes!("../assets/fonts/GoogleSansFlex-SemiBold.ttf"),
        include_bytes!("../assets/fonts/GoogleSansFlex-Bold.ttf"),
    ]),
    ("figtree", [
        include_bytes!("../assets/fonts/Figtree-Regular.ttf"),
        include_bytes!("../assets/fonts/Figtree-SemiBold.ttf"),
        include_bytes!("../assets/fonts/Figtree-ExtraBold.ttf"),
    ]),
    ("instrumentsans", [
        include_bytes!("../assets/fonts/InstrumentSans-Regular.ttf"),
        include_bytes!("../assets/fonts/InstrumentSans-SemiBold.ttf"),
        include_bytes!("../assets/fonts/InstrumentSans-SemiBold.ttf"),
    ]),
    ("jetbrainsnerd", [
        include_bytes!("../assets/fonts/JetBrainsMonoNerdFont-Regular.ttf"),
        include_bytes!("../assets/fonts/JetBrainsMonoNerdFont-SemiBold.ttf"),
        include_bytes!("../assets/fonts/JetBrainsMonoNerdFont-Bold.ttf"),
    ]),
];

/// The bundled faces, by the names the font setting shows and stores.
pub const BUNDLED_FONTS: &[&str] = &["SF Pro Display", "Inter", "DM Sans", "Figtree", "Google Sans Flex", "Instrument Sans", "JetBrains Mono Nerd"];

/// The bundled font a preset font name refers to. Fonts that can't be bundled
/// (Spotify's) map to the closest open one; unknown names to Inter.
fn lyric_key(name: &str) -> Option<&'static str> {
    Some(match name.to_ascii_lowercase().replace(' ', "").as_str() {
        "sfprodisplay" | "sfpro" => "sfpro",
        "dmsans" => "dmsans",
        "googlesansflex" | "googlesans" => "googlesansflex",
        "figtree" | "spotifymixuititle" | "spotifymix" | "circular" => "figtree",
        "switzer" | "instrumentsans" => "instrumentsans",
        "jetbrainsmononerd" | "jetbrainsmononerdfont" | "jetbrainsnerd" | "jetbrainsnerdfont" => "jetbrainsnerd",
        _ => return None,
    })
}

// ------------------------------------------------------------------ font setting

/// The listener's font (Settings › Appearance), used instead of the theme's for the
/// interface and the lyrics. Empty: the theme's own.
static FONT_CHOICE: std::sync::RwLock<String> = std::sync::RwLock::new(String::new());

/// An installed font the listener (or the theme) picked, once its faces are loaded. Drawn through the
/// "choice-<weight>" families, which always exist (Inter until something is loaded), so
/// a frame laid out before egui swaps fonts in never names a missing family.
static CHOICE: std::sync::RwLock<Option<Chosen>> = std::sync::RwLock::new(None);

struct Chosen {
    name: String,
    faces: [Face; 3],
}

/// One face of a primary font: the bytes, the face in the file, and the weight to set
/// on a variable font.
#[derive(Clone, Copy)]
struct Face {
    bytes: &'static [u8],
    index: u32,
    wght: Option<f32>,
}

impl Face {
    const fn of(bytes: &'static [u8]) -> Self {
        Self { bytes, index: 0, wght: None }
    }
}

pub fn set_font_choice(name: &str) {
    *FONT_CHOICE.write().unwrap() = name.to_owned();
}

pub fn font_choice() -> String {
    FONT_CHOICE.read().unwrap().clone()
}

/// Whether the listener's font is `name`, without copying it.
pub fn font_choice_is(name: &str) -> bool {
    *FONT_CHOICE.read().unwrap() == name
}

/// The font name to draw with where a theme asks for `theme_font`.
pub fn effective_font(theme_font: &str) -> String {
    let choice = FONT_CHOICE.read().unwrap();
    if choice.is_empty() { theme_font.to_owned() } else { choice.clone() }
}

/// The family key for a font name: a bundled face, "choice" for the installed font the
/// listener picked (once loaded), else Inter.
fn font_key(name: &str) -> &'static str {
    let name = effective_font(name);
    if let Some(k) = lyric_key(&name) {
        return k;
    }
    if CHOICE.read().unwrap().as_ref().is_some_and(|c| c.name.eq_ignore_ascii_case(&name)) { "choice" } else { "inter" }
}

fn family(key: &str, weight: crate::preset::Weight, size: f32) -> FontId {
    use crate::preset::Weight;
    let w = match weight {
        Weight::Regular => 0,
        Weight::Semibold => 1,
        Weight::Bold => 2,
    };
    FontId::new(size, FontFamily::Name(format!("{key}-{w}").into()))
}

/// Lyric font for a preset's font name and weight.
pub fn lyric_font(name: &str, weight: crate::preset::Weight, size: f32) -> FontId {
    family(font_key(name), weight, size)
}

/// Lyric font for one piece of text. When the font lacks letters the text uses and
/// Inter has them (Vietnamese, Greek or Cyrillic in DM Sans, Figtree, …), the whole
/// text is set in Inter rather than mixing two typefaces letter by letter.
pub fn lyric_font_for(name: &str, weight: crate::preset::Weight, size: f32, text: &str) -> FontId {
    let key = font_key(name);
    let key = if key != "inter" && lacks_letters(key, text) { "inter" } else { key };
    family(key, weight, size)
}

fn lacks_letters(key: &str, text: &str) -> bool {
    use skrifa::MetadataProvider;
    let face = match key {
        "choice" => match CHOICE.read().unwrap().as_ref() {
            Some(c) => c.faces[0],
            None => return false,
        },
        k => match LYRIC_FONTS.iter().find(|(n, _)| *n == k) {
            Some((_, w)) => Face::of(w[0]),
            None => return false,
        },
    };
    let (Ok(font), Ok(inter)) = (skrifa::FontRef::from_index(face.bytes, face.index), skrifa::FontRef::new(INTER[0])) else { return false };
    let (has, inter) = (font.charmap(), inter.charmap());
    text.chars().any(|c| c.is_alphabetic() && has.map(c).is_none() && inter.map(c).is_some())
}

// ------------------------------------------------------------------ installed fonts

/// A font family installed on this PC, for the font setting.
pub struct SysFamily {
    pub name: String,
    faces: Vec<SysFace>,
}

struct SysFace {
    path: std::path::PathBuf,
    index: u32,
    weight: f32,
    /// The weight axis range, for a variable font.
    wght: Option<(f32, f32)>,
}

static INSTALLED_FONTS: std::sync::OnceLock<Vec<SysFamily>> = std::sync::OnceLock::new();
static SCANNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Text fonts installed on this PC, sorted by name, without the bundled ones. `None`
/// while a background scan (started by the first call) runs; `done` runs after it.
pub fn installed_fonts(done: impl FnOnce() + Send + 'static) -> Option<&'static [SysFamily]> {
    if let Some(v) = INSTALLED_FONTS.get() {
        return Some(v);
    }
    if !SCANNING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        std::thread::spawn(move || {
            let _ = INSTALLED_FONTS.set(scan_fonts());
            done();
        });
    }
    None
}

fn font_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs = Vec::new();
    #[cfg(windows)]
    {
        let windir = std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into());
        dirs.push(std::path::PathBuf::from(windir).join("Fonts"));
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            dirs.push(std::path::PathBuf::from(local).join("Microsoft\\Windows\\Fonts"));
        }
    }
    #[cfg(target_os = "macos")]
    dirs.extend(["/System/Library/Fonts", "/Library/Fonts"].map(Into::into));
    #[cfg(all(unix, not(target_os = "macos")))]
    dirs.extend(["/usr/share/fonts", "/usr/local/share/fonts"].map(Into::into));
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(std::path::PathBuf::from(home).join(".local/share/fonts"));
    }
    dirs
}

/// Every upright, normal-width face of a text font (one that maps Latin letters), by
/// family. Only names and attributes are read; nothing stays mapped.
fn scan_fonts() -> Vec<SysFamily> {
    use skrifa::{MetadataProvider, attribute::Style, string::StringId};
    let mut files = Vec::new();
    let mut dirs = font_dirs();
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let path = e.path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().and_then(|x| x.to_str()).is_some_and(|x| matches!(x.to_ascii_lowercase().as_str(), "ttf" | "otf" | "ttc" | "otc")) {
                files.push(path);
            }
        }
    }
    let mut families: std::collections::BTreeMap<String, SysFamily> = std::collections::BTreeMap::new();
    for path in files {
        let Ok(file) = std::fs::File::open(&path) else { continue };
        // SAFETY: read-only and dropped before the next file; font files don't change
        // under a running app.
        let Ok(data) = (unsafe { memmap2::Mmap::map(&file) }) else { continue };
        let Ok(fonts) = skrifa::raw::FileRef::new(&data) else { continue };
        for (index, font) in fonts.fonts().enumerate() {
            let Ok(font) = font else { continue };
            let cmap = font.charmap();
            if cmap.is_symbol() || cmap.map('a').is_none() || cmap.map('Z').is_none() {
                continue;
            }
            let attrs = font.attributes();
            if attrs.style != Style::Normal || (attrs.stretch.ratio() - 1.0).abs() > 0.01 {
                continue;
            }
            let name = [StringId::TYPOGRAPHIC_FAMILY_NAME, StringId::FAMILY_NAME]
                .into_iter()
                .find_map(|id| font.localized_strings(id).english_or_first())
                .map(|s| s.to_string().trim().to_owned());
            // "@" names are vertical CJK variants.
            let Some(name) = name.filter(|n| !n.is_empty() && !n.starts_with('@') && lyric_key(n).is_none() && !n.eq_ignore_ascii_case("inter")) else { continue };
            let wght = font.axes().iter().find(|a| a.tag() == skrifa::Tag::new(b"wght")).map(|a| (a.min_value(), a.max_value()));
            let face = SysFace { path: path.clone(), index: index as u32, weight: attrs.weight.value(), wght };
            families.entry(name.to_lowercase()).or_insert_with(|| SysFamily { name, faces: Vec::new() }).faces.push(face);
        }
    }
    families.into_values().collect()
}

/// The closest face to each of the three weights the app draws: regular, semibold, and
/// bold, leaning to 700 over 900 where a family has both.
fn choose_faces(family: &SysFamily) -> Option<[Face; 3]> {
    let mut faces = Vec::new();
    for target in [400.0, 600.0, 750.0] {
        let best = family.faces.iter().min_by_key(|f| match f.wght {
            Some((lo, hi)) if (lo..=hi).contains(&target) => 0,
            Some((lo, hi)) => (target - target.clamp(lo, hi)).abs() as i32,
            None => (f.weight - target).abs() as i32,
        })?;
        let bytes = map(&best.path)?;
        faces.push(Face { bytes, index: best.index, wght: best.wght.map(|(lo, hi)| target.clamp(lo, hi)) });
    }
    faces.try_into().ok()
}

/// Linux dynamic font discovery helper. 
fn fc_match_face(query: &str, probe: char) -> Option<(std::path::PathBuf, u32)>{
    use skrifa::MetadataProvider;
    let output = std::process::Command::new("fc-match")
        .args(["-f", "%{file}:%{index}\n", query])
        .output()
        .ok()?;
    if !output.status.success() {
        return None
    }
    let s = std::str::from_utf8(&output.stdout).ok()?;
    let (p, idx_str) = s.lines().next()?.trim().rsplit_once(':')?;
    let idx = idx_str.parse::<u32>().unwrap_or(0) & 0xFFFF;
    let path = std::path::PathBuf::from(p);
    let font = skrifa::FontRef::from_index(map(&path)?, idx).ok()?;
    font.charmap().map(probe).is_some().then_some((path, idx))
}

fn face_covers(path: &std::path::Path, index: u32, probe: char) -> bool {
    use skrifa::MetadataProvider;
    map(path)
        .and_then(|bytes| skrifa::FontRef::from_index(bytes, index).ok())
        .is_some_and(|font| font.charmap().map(probe).is_some())
} 

/// Load the faces of the installed font in use, if that's what it is: the listener's
/// pick, else one the theme names for its interface (Fluent's Segoe UI Variable). A
/// name can list fallbacks, "Segoe UI Variable, Segoe UI", tried in order.
fn load_choice(ui_font: &str) {
    let name = effective_font(ui_font);
    if name.is_empty() || lyric_key(&name).is_some() || name.eq_ignore_ascii_case("inter") {
        *CHOICE.write().unwrap() = None;
        return;
    }
    if CHOICE.read().unwrap().as_ref().is_some_and(|c| c.name == name) {
        return;
    }
    // Only when an installed font is set: a one-off scan, the first time.
    let all = INSTALLED_FONTS.get_or_init(scan_fonts);
    let faces = name.split(',').map(str::trim).find_map(|n| all.iter().find(|f| f.name.eq_ignore_ascii_case(n)).and_then(choose_faces));
    if faces.is_none() {
        log::warn!("font {name:?} isn't installed; using Inter");
    }
    *CHOICE.write().unwrap() = faces.map(|faces| Chosen { name, faces });
}

// System fallbacks: egui's fonts cover Latin only, so CJK, Arabic, Indic, Thai,
// symbols and emoji in titles would render as boxes. Memory-mapped, not read, so a
// 20 MB CJK collection costs only the glyph pages actually drawn.
#[cfg(windows)]
const CANDIDATES: &[(&str, u32)] = &[
    ("C:\\Windows\\Fonts\\segoeui.ttf", 0),   // Latin ext., Greek, Cyrillic, Arabic, Hebrew
    ("C:\\Windows\\Fonts\\seguisym.ttf", 0),  // symbols, arrows, dingbats
    ("C:\\Windows\\Fonts\\YuGothM.ttc", 0),   // Japanese
    ("C:\\Windows\\Fonts\\msyh.ttc", 0),      // Simplified Chinese
    ("C:\\Windows\\Fonts\\msjh.ttc", 0),      // Traditional Chinese
    ("C:\\Windows\\Fonts\\malgun.ttf", 0),    // Korean
    ("C:\\Windows\\Fonts\\Nirmala.ttc", 0),   // Indic scripts
    ("C:\\Windows\\Fonts\\Nirmala.ttf", 0),
    ("C:\\Windows\\Fonts\\LeelawUI.ttf", 0),  // Thai, Lao, Khmer
    ("C:\\Windows\\Fonts\\ebrima.ttf", 0),    // Ethiopic, N'Ko, etc.
    ("C:\\Windows\\Fonts\\seguiemj.ttf", 0),  // emoji (drawn monochrome)
];

/// Semibold and bold faces of the fallbacks, so text in a script the primary font
/// lacks comes out at the weight the line asks for, not regular inside a bold title
/// (Better Lyrics uses Noto Sans CJK at the lyric weight).
#[cfg(windows)]
const HEAVIER: &[(&str, [(&str, u32); 2])] = &[
    ("C:\\Windows\\Fonts\\segoeui.ttf", [("C:\\Windows\\Fonts\\seguisb.ttf", 0), ("C:\\Windows\\Fonts\\segoeuib.ttf", 0)]),
    ("C:\\Windows\\Fonts\\YuGothM.ttc", [("C:\\Windows\\Fonts\\YuGothB.ttc", 0), ("C:\\Windows\\Fonts\\YuGothB.ttc", 0)]),
    ("C:\\Windows\\Fonts\\msyh.ttc", [("C:\\Windows\\Fonts\\msyhbd.ttc", 0), ("C:\\Windows\\Fonts\\msyhbd.ttc", 0)]),
    ("C:\\Windows\\Fonts\\msjh.ttc", [("C:\\Windows\\Fonts\\msjhbd.ttc", 0), ("C:\\Windows\\Fonts\\msjhbd.ttc", 0)]),
    ("C:\\Windows\\Fonts\\malgun.ttf", [("C:\\Windows\\Fonts\\malgunbd.ttf", 0), ("C:\\Windows\\Fonts\\malgunbd.ttf", 0)]),
    ("C:\\Windows\\Fonts\\Nirmala.ttc", [("C:\\Windows\\Fonts\\Nirmala.ttc", 1), ("C:\\Windows\\Fonts\\Nirmala.ttc", 1)]),
    ("C:\\Windows\\Fonts\\LeelawUI.ttf", [("C:\\Windows\\Fonts\\LeelaUIb.ttf", 0), ("C:\\Windows\\Fonts\\LeelaUIb.ttf", 0)]),
    ("C:\\Windows\\Fonts\\ebrima.ttf", [("C:\\Windows\\Fonts\\ebrimabd.ttf", 0), ("C:\\Windows\\Fonts\\ebrimabd.ttf", 0)]),
];
#[cfg(not(windows))]
const HEAVIER: &[(&str, [(&str, u32); 2])] = &[];

#[cfg(target_os = "macos")]
const CANDIDATES: &[(&str, u32)] = &[
    ("/System/Library/Fonts/Supplemental/Arial Unicode.ttf", 0),
    ("/System/Library/Fonts/Hiragino Sans GB.ttc", 0),
    ("/System/Library/Fonts/AppleSDGothicNeo.ttc", 0),
    ("/System/Library/Fonts/Apple Symbols.ttf", 0),
];

#[cfg(windows)]
fn fallback_candidates() -> Vec<(std::path::PathBuf, u32)> {
    CANDIDATES.iter().map(|&(p, i)| (std::path::PathBuf::from(p), i)).collect()
}

#[cfg(target_os = "macos")]
fn fallback_candidates() -> Vec<(std::path::PathBuf, u32)> {
    CANDIDATES.iter().map(|&(p, i)| (std::path::PathBuf::from(p), i)).collect()
}

#[cfg(all(unix, not(target_os = "macos")))]
fn fallback_candidates() -> Vec<(std::path::PathBuf, u32)> {
    linux_fallback_candidates()
}

#[cfg(all(unix, not(target_os = "macos")))]
fn linux_fallback_candidates() -> Vec<(std::path::PathBuf, u32)> {
    let mut list: Vec<(std::path::PathBuf, u32)> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    // Static well-known paths covering major Linux distributions
    // (Fedora, Arch, Debian, Ubuntu, openSUSE, Alpine, etc.)
    const KNOWN: &[(&str, u32)] = &[
        ("/usr/share/fonts/google-noto-sans-cjk-vf-fonts/NotoSansCJK-VF.ttc", 0),
        ("/usr/share/fonts/google-noto-sans-cjk-vf-fonts/NotoSansCJK-VF.ttc", 2),
        ("/usr/share/fonts/google-noto-cjk-fonts/NotoSansCJK-Regular.ttc", 0),
        ("/usr/share/fonts/google-noto-cjk-fonts/NotoSansCJK-Regular.ttc", 2),
        ("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc", 0),
        ("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc", 2),
        ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", 0),
        ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", 2),
        ("/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc", 0),
        ("/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc", 2),
        ("/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc", 0),
        ("/usr/share/fonts/wenquanyi/wqy-microhei/wqy-microhei.ttc", 0),
        ("/usr/share/fonts/truetype/wqy/wqy-microhei.ttc", 0),
        ("/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf", 0),

        ("/usr/share/fonts/google-noto-vf/NotoSansArabic[wght].ttf", 0),
        ("/usr/share/fonts/google-noto-fonts/NotoSansArabic-Regular.ttf", 0),
        ("/usr/share/fonts/noto/NotoSansArabic-Regular.ttf", 0),
        ("/usr/share/fonts/truetype/noto/NotoSansArabic-Regular.ttf", 0),
        ("/usr/share/fonts/opentype/noto/NotoSansArabic-Regular.otf", 0),

        ("/usr/share/fonts/google-noto-vf/NotoSansHebrew[wght].ttf", 0),
        ("/usr/share/fonts/google-noto-fonts/NotoSansHebrew-Regular.ttf", 0),
        ("/usr/share/fonts/noto/NotoSansHebrew-Regular.ttf", 0),
        ("/usr/share/fonts/truetype/noto/NotoSansHebrew-Regular.ttf", 0),

        ("/usr/share/fonts/google-noto-vf/NotoSansThai[wght].ttf", 0),
        ("/usr/share/fonts/google-noto-fonts/NotoSansThai-Regular.ttf", 0),
        ("/usr/share/fonts/noto/NotoSansThai-Regular.ttf", 0),
        ("/usr/share/fonts/truetype/noto/NotoSansThai-Regular.ttf", 0),

        ("/usr/share/fonts/google-noto-vf/NotoSansDevanagari[wght].ttf", 0),
        ("/usr/share/fonts/google-noto-fonts/NotoSansDevanagari-Regular.ttf", 0),
        ("/usr/share/fonts/noto/NotoSansDevanagari-Regular.ttf", 0),
        ("/usr/share/fonts/truetype/noto/NotoSansDevanagari-Regular.ttf", 0),

        ("/usr/share/fonts/google-noto-vf/NotoSansBengali[wght].ttf", 0),
        ("/usr/share/fonts/google-noto-vf/NotoSansTamil[wght].ttf", 0),
        ("/usr/share/fonts/google-noto-vf/NotoSansTelugu[wght].ttf", 0),
        ("/usr/share/fonts/google-noto-vf/NotoSansKhmer[wght].ttf", 0),

        ("/usr/share/fonts/google-noto/NotoSansSymbols2-Regular.ttf", 0),
        ("/usr/share/fonts/google-noto-fonts/NotoSansSymbols2-Regular.ttf", 0),
        ("/usr/share/fonts/noto/NotoSansSymbols2-Regular.ttf", 0),
        ("/usr/share/fonts/truetype/noto/NotoSansSymbols2-Regular.ttf", 0),
        ("/usr/share/fonts/google-noto/NotoSansSymbols-Regular.ttf", 0),
        ("/usr/share/fonts/noto/NotoSansSymbols-Regular.ttf", 0),
        ("/usr/share/fonts/truetype/noto/NotoSansSymbols-Regular.ttf", 0),

        ("/usr/share/fonts/google-noto/NotoSans-Regular.ttf", 0),
        ("/usr/share/fonts/noto/NotoSans-Regular.ttf", 0),
        ("/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf", 0),
        ("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", 0),
        ("/usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf", 0),
        ("/usr/share/fonts/dejavu/DejaVuSans.ttf", 0),
    ];

    for &(path, idx) in KNOWN {
        let p = std::path::PathBuf::from(path);
        if p.exists() && seen.insert((p.clone(), idx)) {
            list.push((p, idx));
        }
    }

    // Dynamic discovery via `fc-match` for scripts that were not found
    // in well-known system paths (e.g. NixOS, Guix, custom prefixes, ~/.local/share/fonts).
    // Default using Noto Sans, but will use other font family if there's none
    let queries: &[(&str, char)] = &[
        ("Noto Sans CJK SC:charset=4e00", '\u{4e00}'),
        ("Noto Sans CJK JP:charset=3042", '\u{3042}'),
        ("Noto Sans CJK KR:charset=ac00", '\u{ac00}'),
        ("Noto Sans Arabic:charset=0627", '\u{0627}'),
        ("Noto Sans Hebrew:charset=05d0", '\u{05d0}'),
        ("Noto Sans Thai:charset=0e01", '\u{0e01}'),
        ("Noto Sans Devanagari:charset=0915", '\u{0915}'),
        ("Noto Sans Symbols 2:charset=2192", '\u{2192}'),
    ];


    for &(query, probe) in queries {
        if list.iter().any(|(p, idx)| face_covers(p, *idx, probe)) {
            continue;
        }
        if let Some(face) = fc_match_face(query, probe) {
            if seen.insert(face.clone()) {
                list.push(face);
            }
        }
    }

    list
}

/// Ascent and line height in em, as egui lays text out (skrifa's metrics: OS/2 typo
/// values when the font asks for them, else hhea).
fn em_metrics(bytes: &[u8], index: u32) -> Option<(f32, f32)> {
    use skrifa::MetadataProvider;
    let font = skrifa::FontRef::from_index(bytes, index).ok()?;
    let m = font.metrics(skrifa::instance::Size::unscaled(), skrifa::instance::LocationRef::default());
    let upem = m.units_per_em.max(1) as f32;
    Some((m.ascent / upem, (m.ascent - m.descent + m.leading) / upem))
}

fn font_data(face: Face, y_offset_factor: f32) -> FontData {
    let mut tweak = egui::FontTweak { y_offset_factor, ..Default::default() };
    if let Some(w) = face.wght {
        tweak.coords.push(b"wght", w);
    }
    FontData { index: face.index, ..FontData::from_static(face.bytes) }.tweak(tweak)
}

type Fallbacks = [Vec<(String, Face)>; 3];

/// Install fonts. `ui_font` is the theme's font name for all interface text ("inter"
/// or one of the bundled lyric fonts); the listener's font setting wins over it.
///
/// Each primary font is followed by Inter at the same weight (for the scripts the
/// theme fonts lack), the system fallbacks at the closest weight, then bundled
/// Noto Sans Devanagari for systems without that script.
///
/// egui places a fallback font's glyphs by centring its line height on the primary
/// font's, not by sharing a baseline, so CJK glyphs (Yu Gothic, Microsoft YaHei, …)
/// sit visibly higher than Latin ones in the same line. Each fallback therefore gets a
/// y offset computed from both fonts' metrics, once per primary font it backs.
pub fn install_fonts(ctx: &egui::Context, ui_font: &str) {
    load_choice(ui_font);
    let ui_key = font_key(ui_font);
    // Installing rebuilds the font atlas; most themes share a UI font, so skip repeats.
    static INSTALLED: std::sync::Mutex<Option<(&'static str, String)>> = std::sync::Mutex::new(None);
    let loaded = CHOICE.read().unwrap().as_ref().map(|c| c.name.clone()).unwrap_or_default();
    let state = Some((ui_key, loaded));
    {
        let mut installed = INSTALLED.lock().unwrap();
        if *installed == state {
            return;
        }
        *installed = state;
    }
    // Mapped once for the life of the process, however often fonts are installed:
    // the fallback list at each weight.
    static SYSTEM: std::sync::OnceLock<Fallbacks> = std::sync::OnceLock::new();
    let system = SYSTEM.get_or_init(|| {
        let candidates = fallback_candidates();
        std::array::from_fn(|w| {
            candidates
                .iter()
                .filter_map(|(path, index)| {
                    let heavy = (w > 0)
                        .then(|| {
                            let p_str = path.to_str()?;
                            HEAVIER.iter().find(|(base, _)| *base == p_str).map(|(_, v)| v[w - 1])
                        })
                        .flatten();
                    if let Some((p, i)) = heavy {
                        if let Some(bytes) = map(p) {
                            return Some((format!("sys:{p}#{i}"), Face { bytes, index: i, wght: None }));
                        }
                    }
                    let bytes = map(path)?;
                    let target = FALLBACK_WEIGHTS[w];
                    let wght = skrifa::FontRef::from_index(bytes, *index)
                        .ok()
                        .and_then(|f| {
                            use skrifa::MetadataProvider;
                            f.axes()
                                .iter()
                                .find(|a| a.tag() == skrifa::Tag::new(b"wght"))
                                .map(|a| target.clamp(a.min_value(), a.max_value()))
                        });
                    Some((format!("sys:{}#{index}", path.display()), Face { bytes, index: *index, wght }))
                })
                .collect()
        })
    });
    let choice = CHOICE.read().unwrap().as_ref().map(|c| c.faces);
    ctx.set_fonts(font_definitions(ui_key, choice, system));
}

fn font_definitions(ui_key: &str, choice: Option<[Face; 3]>, system: &Fallbacks) -> FontDefinitions {
    let mut defs = FontDefinitions::default();
    let egui_default = defs.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    defs.font_data.insert("lucide".into(), Arc::new(FontData::from_static(LUCIDE).tweak(egui::FontTweak { y_offset_factor: 0.0, ..Default::default() })));
    defs.families.insert(FontFamily::Name("icons".into()), vec!["lucide".to_owned()]);

    // Primary fonts: Inter, each lyric font, and the installed font picked in Settings.
    let mut groups: Vec<(&str, [Face; 3])> = vec![("inter", INTER.map(Face::of))];
    groups.extend(LYRIC_FONTS.iter().map(|(k, w)| (*k, w.map(Face::of))));
    groups.extend(choice.map(|faces| ("choice", faces)));
    for (key, faces) in &groups {
        let (asc_p, h_p) = em_metrics(faces[0].bytes, faces[0].index).unwrap_or((0.97, 1.21));
        for (w, face) in faces.iter().enumerate() {
            let name = format!("{key}-{w}");
            defs.font_data.insert(name.clone(), Arc::new(font_data(*face, 0.0)));
            let mut chain = vec![name.clone()];
            let inter = (*key != "inter").then(|| ("inter".to_owned(), Face::of(INTER[w])));
            let devanagari = ("noto-devanagari".to_owned(), Face { wght: Some(FALLBACK_WEIGHTS[w]), ..Face::of(NOTO_DEVANAGARI) });
            for (fallback, f) in inter.iter().chain(&system[w]).chain(std::iter::once(&devanagari)) {
                let Some((asc_f, h_f)) = em_metrics(f.bytes, f.index) else { continue };
                let shift = asc_p - (asc_f + 0.5 * (h_p - h_f));
                let id = format!("{fallback}-{w}@{key}");
                if !defs.font_data.contains_key(&id) {
                    defs.font_data.insert(id.clone(), Arc::new(font_data(*f, shift)));
                }
                chain.push(id);
            }
            if *key == "inter" && w == 0
                && let Some(mono) = defs.families.get_mut(&FontFamily::Monospace)
            {
                mono.extend(chain[1..].iter().cloned());
            }
            // egui's bundled emoji/icon font stays as a last resort.
            chain.extend(egui_default.iter().filter(|n| n.as_str() != "Ubuntu-Light").cloned());
            defs.families.insert(FontFamily::Name(name.clone().into()), chain.clone());
            if *key == ui_key {
                let family = match w {
                    0 => FontFamily::Proportional,
                    1 => FontFamily::Name("semibold".into()),
                    _ => FontFamily::Name("bold".into()),
                };
                defs.families.insert(family, chain);
            }
        }
    }
    // With no installed font picked, the "choice" families are Inter's, so they always
    // exist without a second copy of Inter and its fallbacks.
    if choice.is_none() {
        for w in 0..3 {
            let inter = defs.families[&FontFamily::Name(format!("inter-{w}").into())].clone();
            defs.families.insert(FontFamily::Name(format!("choice-{w}").into()), inter);
        }
    }
    // The same bytes get the same `Arc` on every install, so comparing these definitions
    // with the installed ones is a pointer check per font instead of reading every file.
    for data in defs.font_data.values_mut() {
        let font = shared_bytes(&data.font);
        if !Arc::ptr_eq(&font, &data.font) {
            *data = Arc::new(FontData { font, ..(**data).clone() });
        }
    }
    defs
}

/// The first `Blob` seen for these bytes. Font bytes here are all `'static` (bundled, or
/// mapped for the rest of the process), so the address identifies them for good.
fn shared_bytes(blob: &egui::epaint::text::Blob) -> egui::epaint::text::Blob {
    static SEEN: std::sync::Mutex<Vec<egui::epaint::text::Blob>> = std::sync::Mutex::new(Vec::new());
    let bytes = (**blob).as_ref();
    let mut seen = SEEN.lock().unwrap();
    if let Some(b) = seen.iter().find(|b| std::ptr::eq((***b).as_ref(), bytes)) {
        return b.clone();
    }
    seen.push(blob.clone());
    blob.clone()
}

/// Map a font file for the rest of the process. Kept by canonical path, so a file is
/// mapped once however many weights, faces or installed-font picks use it, and whichever
/// spelling names it (the installed-font scan's C:\WINDOWS, the fallback list's C:\Windows).
fn map(path: impl AsRef<std::path::Path>) -> Option<&'static [u8]> {
    static MAPPED: std::sync::Mutex<Vec<(std::path::PathBuf, &'static [u8])>> = std::sync::Mutex::new(Vec::new());
    let path = std::fs::canonicalize(path).ok()?;
    let mut mapped = MAPPED.lock().unwrap();
    if let Some((_, bytes)) = mapped.iter().find(|(p, _)| *p == path) {
        return Some(bytes);
    }
    let file = std::fs::File::open(&path).ok()?;
    // SAFETY: system font files aren't modified while in use; the mapping is
    // read-only and intentionally lives for the rest of the process.
    let mmap = unsafe { memmap2::Mmap::map(&file) }.ok()?;
    let bytes: &'static [u8] = Box::leak(Box::new(mmap));
    mapped.push((path, bytes));
    Some(bytes)
}

/// Apply a theme preset's chrome: how strong glass surfaces are and how round
/// buttons, pills and fields are.
pub fn apply_chrome(ctx: &egui::Context, p: &crate::preset::Preset) {
    GLASS.store(p.chrome.glass.clamp(0.0, 2.5).to_bits(), std::sync::atomic::Ordering::Relaxed);
    let dim = crate::preset::hex(&p.app.text_dim).unwrap_or(TEXT_DIM);
    DIM.store(u32::from_le_bytes(dim.to_array()), std::sync::atomic::Ordering::Relaxed);
    install_style(ctx);
    install_fonts(ctx, &p.app.font);
}

pub fn install_style(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(8.0, 6.0);
        s.spacing.button_padding = egui::vec2(14.0, 7.0);
        s.spacing.scroll = egui::style::ScrollStyle::floating();
        s.text_styles.insert(egui::TextStyle::Body, regular(14.0));
        s.text_styles.insert(egui::TextStyle::Button, semibold(14.0));
        s.text_styles.insert(egui::TextStyle::Heading, bold(22.0));
        s.text_styles.insert(egui::TextStyle::Small, regular(12.0));
        s.visuals = visuals();
        s.interaction.selectable_labels = false;
        s.animation_time = 0.18 * speed();
    });
}

/// egui's colours and shapes for the current mode and the active preset's chrome.
fn visuals() -> egui::Visuals {
    let p = crate::preset::active();
    let mut v = if light() { egui::Visuals::light() } else { egui::Visuals::dark() };
    v.panel_fill = bg();
    v.window_fill = surface_fill();
    v.extreme_bg_color = glass(0.08);
    v.override_text_color = Some(text());
    v.selection.bg_fill = with_alpha(RED, if light() { 0.3 } else { 0.5 });
    v.hyperlink_color = text();
    v.interact_cursor = Some(egui::CursorIcon::PointingHand);
    let pr = egui::CornerRadius::same(p.app.panel_radius.clamp(0.0, 32.0) as u8);
    v.window_corner_radius = pr;
    v.menu_corner_radius = pr;
    v.window_stroke = egui::Stroke::new(1.0, glass(0.08));
    v.popup_shadow = egui::epaint::Shadow { offset: [0, 8], blur: 28, spread: 0, color: shade(if light() { 0.16 } else { 0.5 }) };
    let r = egui::CornerRadius::same(p.chrome.radius.clamp(2.0, 28.0) as u8);
    for (w, fill) in [
        (&mut v.widgets.inactive, glass(0.08)),
        (&mut v.widgets.hovered, glass(0.14)),
        (&mut v.widgets.active, glass(0.2)),
        (&mut v.widgets.open, glass(0.14)),
    ] {
        w.bg_fill = fill;
        w.weak_bg_fill = fill;
        w.bg_stroke = egui::Stroke::NONE;
        w.corner_radius = r;
        w.fg_stroke.color = text();
    }
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, glass(0.08));
    v
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn devanagari_renders_without_system_fonts() {
        use egui::epaint::text::{Fonts, MissingGlyphPolicy, TextOptions};
        use crate::preset::Weight;

        let defs = font_definitions("jetbrainsnerd", None, &Fallbacks::default());
        let mut fonts = Fonts::new(TextOptions::default(), defs).with_missing_glyph_policy(MissingGlyphPolicy::Panic);
        let mut fonts = fonts.with_pixels_per_point(1.0);
        let text = "Hindi · काफ़ी है ना";
        for id in [
            regular(32.0), semibold(32.0), bold(32.0), FontId::monospace(32.0),
            family("jetbrainsnerd", Weight::Bold, 32.0),
            family("inter", Weight::Regular, 32.0),
            family("choice", Weight::Semibold, 32.0),
        ] {
            assert!(fonts.has_glyphs(&id, text), "missing Hindi or Latin glyphs in {id:?}");
            fonts.layout_no_wrap(text.to_owned(), id, Color32::WHITE);
        }

        // The consonant conjunct is one visible glyph after shaping.
        let conjunct = fonts.layout_no_wrap("क्ष".to_owned(), family("jetbrainsnerd", Weight::Bold, 32.0), Color32::WHITE);
        assert_eq!(conjunct.rows[0].visuals.mesh.vertices.len(), 4);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_portals_color_scheme_reads_from_one_variant_or_two() {
        use dbus::arg::{RefArg, Variant};
        let once: Box<dyn RefArg> = Box::new(2u32);
        let twice: Box<dyn RefArg> = Box::new(Variant(Box::new(1u32) as Box<dyn RefArg>));
        assert_eq!(variant_number(&Variant(once)), Some(2));
        assert_eq!(variant_number(&Variant(twice)), Some(1));
        assert_eq!(variant_number(&Variant(Box::new("dark".to_owned()) as Box<dyn RefArg>)), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn gnomes_default_style_is_light_and_no_preference_elsewhere() {
        assert_eq!(scheme(Some(1), true), 1);
        assert_eq!(scheme(Some(2), false), 2);
        assert_eq!(scheme(Some(0), true), 2);
        assert_eq!(scheme(Some(0), false), 0);
        assert_eq!(scheme(None, true), 0);
    }
}
