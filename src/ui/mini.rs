//! The mini player: the same window, shrunk to a strip with the song, the transport and a
//! seek line, kept above other windows unless unpinned. Ctrl+M or the player bar's
//! button switches; the strip's expand button (or a double-click on it) switches back.
//!
//! Each mode keeps its own place: the full window goes back to exactly where it was
//! (maximized again if it was), and the strip reopens where it was last moved. Places
//! are kept in physical pixels with the monitor's scale, so they survive moving between
//! monitors with different scaling. Launching always opens the full window: closed in
//! the mini player, the app saves the full window's place for next time.
//!
//! On Windows the app draws the strip's window buttons itself (see `titlebar`). Elsewhere
//! the system's frame stays, with its own close button, so the strip leaves its out.
//! Keeping on top is up to the window manager (Wayland has no way to ask).

use std::time::Duration;

use egui::{Color32, Pos2, Rect, Sense, Ui, Vec2, ViewportCommand, WindowLevel, pos2, vec2};
use serde::{Deserialize, Serialize};

use super::widgets::{self, icon_at, named, paint_cover, text_at};
use super::{Action, App, fmt_time, glyphs, titlebar};
use crate::theme::{self, icon};

// Decisions that are easy to change.

/// The strip when first opened. It's as tall as its contents; the width goes from
/// `MIN_W` to `MAX_W` by its sides.
pub const SIZE: Vec2 = vec2(400.0, 112.0);
const MIN_W: f32 = 330.0;
const MAX_W: f32 = 720.0;
/// The seek line's hit band along the bottom, taller than the line it shows: a press
/// anywhere near the line seeks, instead of dragging the strip. The window-drag area
/// keeps out of it (see `strip`), and it clears the resize border a frameless window
/// can reserve along its bottom edge.
const SEEK_BAND: f32 = 14.0;
/// Kept above other windows until unpinned.
const PINNED_BY_DEFAULT: bool = true;
/// From the work area's corner, the first time.
const MARGIN: f32 = 16.0;
/// Picture in picture: the mini player for a video, when first opened, and as small and
/// as big as it goes. In between it takes the video's shape (#104).
const VIDEO_SIZE: Vec2 = vec2(480.0, 270.0);
const VIDEO_MIN: Vec2 = vec2(240.0, 135.0);
const VIDEO_MAX: Vec2 = vec2(1280.0, 720.0);

/// How long the window keeps one size before it's put in the video's shape: a resize
/// the app can't follow (Wayland's, done by the desktop) is over by then. Windows keeps
/// the shape as it's dragged instead, as mpv does.
const SNAP_AFTER: f64 = 0.3;

/// The size in the video's shape (`ratio`, width over height) and within `min` and `max`,
/// keeping the width, or with `by_height` the height.
fn in_shape(ratio: f32, size: Vec2, by_height: bool, min: Vec2, max: Vec2) -> Vec2 {
    let lo = min.x.max(min.y * ratio);
    let hi = max.x.min(max.y * ratio).max(lo);
    let w = if by_height { size.y * ratio } else { size.x }.clamp(lo, hi);
    vec2(w, w / ratio)
}

/// Whether a resize from `from` to `to` was mostly of the height, so that's the side to keep.
fn height_led(from: Vec2, to: Vec2) -> bool {
    (to.y - from.y).abs() / from.y.max(1.0) > (to.x - from.x).abs() / from.x.max(1.0)
}

/// The window is being dragged or resized (Windows' own loop for it): not the moment
/// to change its size. Only Windows says.
static MOVING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Picture in picture's shape for Windows, which keeps it while the edges are dragged:
/// the video's width over height, and the size limits, in physical pixels.
#[cfg(windows)]
static SHAPE: std::sync::Mutex<Option<(f32, Vec2, Vec2)>> = std::sync::Mutex::new(None);

/// Keeps picture in picture in the video's shape while it's resized (Windows; elsewhere it
/// takes the shape once the resize is over).
#[cfg_attr(not(windows), allow(unused_variables))]
pub fn set_hwnd(hwnd: *mut std::ffi::c_void) {
    #[cfg(windows)]
    shape::init(hwnd);
}

#[cfg(windows)]
mod shape {
    use std::sync::atomic::Ordering;

    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
    use windows::Win32::UI::WindowsAndMessaging::{GetClientRect, GetWindowRect, WM_ENTERSIZEMOVE, WM_EXITSIZEMOVE, WM_SIZING, WMSZ_BOTTOMLEFT, WMSZ_BOTTOM, WMSZ_LEFT, WMSZ_TOP, WMSZ_TOPLEFT, WMSZ_TOPRIGHT};

    pub fn init(hwnd: *mut std::ffi::c_void) {
        // SAFETY: the app's own window, on the thread that made it; the subclass lives as
        // long as the window.
        if !unsafe { SetWindowSubclass(HWND(hwnd), Some(proc), 3, 0) }.as_bool() {
            log::warn!("picture in picture: couldn't subclass the window");
        }
    }

    unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _: usize, _: usize) -> LRESULT {
        match msg {
            WM_ENTERSIZEMOVE => super::MOVING.store(true, Ordering::Relaxed),
            WM_EXITSIZEMOVE => super::MOVING.store(false, Ordering::Relaxed),
            _ => {}
        }
        // SAFETY: the window's own message, passed on as it came.
        let result = unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };
        let shape = *super::SHAPE.lock().unwrap();
        let Some((ratio, min, max)) = shape.filter(|_| msg == WM_SIZING) else { return result };
        let (mut outer, mut inner) = (RECT::default(), RECT::default());
        // SAFETY: the app's own window; the rectangles are ours.
        if unsafe { GetWindowRect(hwnd, &mut outer).is_err() || GetClientRect(hwnd, &mut inner).is_err() } {
            return result;
        }
        // The frame around the inside, which the dragged rectangle includes.
        let fx = (outer.right - outer.left) - (inner.right - inner.left);
        let fy = (outer.bottom - outer.top) - (inner.bottom - inner.top);
        // SAFETY: for WM_SIZING, lparam points at the window's proposed rectangle.
        let r = unsafe { &mut *(lparam.0 as *mut RECT) };
        let edge = wparam.0 as u32;
        let size = egui::vec2((r.right - r.left - fx) as f32, (r.bottom - r.top - fy) as f32);
        // Like mpv: the top or bottom edge sets the height, any other the width.
        let fit = super::in_shape(ratio, size, edge == WMSZ_TOP || edge == WMSZ_BOTTOM, min, max);
        let (w, h) = (fit.x.round() as i32 + fx, fit.y.round() as i32 + fy);
        // The edges across from the one dragged stay put.
        if matches!(edge, WMSZ_LEFT | WMSZ_TOPLEFT | WMSZ_BOTTOMLEFT) {
            r.left = r.right - w;
        } else {
            r.right = r.left + w;
        }
        if matches!(edge, WMSZ_TOP | WMSZ_TOPLEFT | WMSZ_TOPRIGHT) {
            r.top = r.bottom - h;
        } else {
            r.bottom = r.top + h;
        }
        LRESULT(1)
    }
}

/// The mini player is the video (picture in picture): a video was on show when it opened.
static VIDEO: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The mini player up, or last up, is the video.
pub fn video_active() -> bool {
    VIDEO.load(std::sync::atomic::Ordering::Relaxed)
}

const VIDEO_KEY: &str = "mini_video_window";

/// The mini player's size when first opened.
fn mini_size() -> Vec2 {
    if video_active() { VIDEO_SIZE } else { SIZE }
}

fn width_limits() -> (f32, f32) {
    if video_active() { (VIDEO_MIN.x, VIDEO_MAX.x) } else { (MIN_W, MAX_W) }
}

/// Where a window was: its outer corner in physical pixels, its inner size in points,
/// and the monitor's scale and the app's zoom then (points are that big).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Place {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub scale: f32,
    #[serde(default = "unzoomed")]
    pub zoom: f32,
}

fn unzoomed() -> f32 {
    1.0
}

impl Place {
    /// The window's place now, once the platform has said. Wayland never tells apps
    /// where their windows are (nor lets them move them): there only the size counts.
    fn current(ctx: &egui::Context) -> Option<Place> {
        let ppp = ctx.pixels_per_point();
        let size = ctx.content_rect().size();
        let zoom = ctx.zoom_factor();
        ctx.input(|i| {
            let vp = i.viewport();
            let pos = vp.outer_rect.map_or(Pos2::ZERO, |r| r.min * ppp);
            Some(Place { x: pos.x, y: pos.y, w: size.x, h: size.y, scale: vp.native_pixels_per_point?, zoom })
        })
    }

    fn size(&self) -> Vec2 {
        vec2(self.w, self.h)
    }

    /// Its size in points at the app's zoom now (it may have changed since).
    fn size_at(&self, zoom: f32) -> Vec2 {
        self.size() * (self.zoom / zoom)
    }

    /// The outer rectangle in physical pixels, given the frame around the inside (points).
    fn rect_px(&self, frame: Vec2) -> Rect {
        Rect::from_min_size(pos2(self.x, self.y), (self.size() + frame) * self.scale * self.zoom)
    }

    /// eframe's saved-window format, so a launch after closing in the mini player opens
    /// the full window where it was.
    fn eframe_window(&self, maximized: bool) -> String {
        format!(
            "(normal_geometry:(outer_position_pixels:Some((x:{:?},y:{:?})),scale_factor:{:?},inner_size_points:(x:{:?},y:{:?})),fullscreen:false,maximized:{maximized})",
            self.x, self.y, self.scale, self.w, self.h
        )
    }
}

/// The strip's first place: the bottom right of `area` (physical pixels), clear of its
/// edges, for a window with `frame` (points) around its inside. A point is `scale`
/// times `zoom` pixels.
fn corner_place(area: Rect, frame: Vec2, scale: f32, zoom: f32) -> Place {
    let outer = (mini_size() + frame) * scale * zoom;
    let p = area.max - outer - Vec2::splat(MARGIN * scale);
    let p = p.max(area.min);
    Place { x: p.x.round(), y: p.y.round(), w: mini_size().x, h: mini_size().y, scale, zoom }
}

/// A window `size` (points) in the middle of `area` (physical pixels), kept inside it.
fn centered(area: Rect, size: Vec2, frame: Vec2, scale: f32, zoom: f32) -> Place {
    let outer = (size + frame) * scale * zoom;
    let p = (area.center() - outer / 2.0).max(area.min);
    Place { x: p.x.round(), y: p.y.round(), w: size.x, h: size.y, scale, zoom }
}

/// What the app can ask about the monitors (the system's; made up in tests).
#[derive(Clone, Copy)]
struct Screens {
    /// Whether some of a rectangle (physical pixels) is on a connected monitor.
    on_a_monitor: fn(Rect) -> bool,
    /// The window's monitor less the taskbar, in physical pixels.
    work_area: fn() -> Option<Rect>,
}

const SYSTEM: Screens = Screens { on_a_monitor: crate::fullscreen::on_a_monitor, work_area: crate::fullscreen::work_area };

/// A remembered strip size, within the limits (they may have changed since).
fn clamp_size(p: Place) -> Place {
    let (min, max) = width_limits();
    // The video's is any height in its limits; the strip has one.
    let h = if video_active() { p.h.clamp(VIDEO_MIN.y, VIDEO_MAX.y) } else { mini_size().y };
    Place { w: p.w.clamp(min, max), h, ..p }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Step {
    /// Waiting for a maximized full window to come down, so its normal place is known.
    Unmaximize,
    /// Move and resize now.
    Resize,
    /// Moved onto a monitor with another scale: size it once the window has that scale.
    Rescale(Place),
    /// Waiting for the window to reach its new size (None: to be maximized) before the
    /// new layout shows.
    Settle(Option<Vec2>),
}

struct Switch {
    to_mini: bool,
    step: Step,
    since: f64,
    /// The window's size when it began.
    from: Vec2,
}

/// What to draw this frame.
#[derive(Clone, Copy, PartialEq)]
pub enum Layout {
    Full,
    Mini,
    /// Just the background: mid-switch, the window no longer fits the old layout and
    /// doesn't have the new one's size yet.
    Blank,
}

pub struct Mini {
    /// The strip is showing.
    on: bool,
    pub pinned: bool,
    /// The strip's last place.
    place: Option<Place>,
    /// Picture in picture's.
    video_place: Option<Place>,
    /// The full window's, from before it shrank, and whether it was maximized.
    full: Option<(Place, bool)>,
    was_maximized: bool,
    switch: Option<Switch>,
    /// A switch asked for, started once the window is ready.
    wanted: Option<bool>,
    /// When the current layout appeared, for its fade-in.
    shown_at: f64,
    /// The scale the size limits were last sent at: winit keeps them in pixels.
    limits_scale: Option<f32>,
    screens: Screens,
    /// The full window's normal place after coming back maximized, until it's restored down.
    restored_max: Option<Place>,
    /// The shown video's width over height, while picture in picture has one.
    pub video_shape: Option<f32>,
    /// Picture in picture's last size in the video's shape, to tell which side a resize changed.
    shaped: Option<Vec2>,
    /// The window's size, and since when it's had it.
    sized: Option<(Vec2, f64)>,
    /// A size the video's shape was asked for from, so a desktop that says no isn't asked again.
    asked_from: Option<Vec2>,
}

impl Mini {
    pub fn load(get: impl Fn(&str) -> Option<String>) -> Self {
        Mini {
            on: false,
            pinned: get("mini_pinned").map_or(PINNED_BY_DEFAULT, |v| v == "1"),
            place: get("mini_window").and_then(|v| serde_json::from_str(&v).ok()).map(clamp_size),
            video_place: get(VIDEO_KEY).and_then(|v| serde_json::from_str::<Place>(&v).ok()),
            full: None,
            was_maximized: false,
            switch: None,
            wanted: None,
            shown_at: f64::NEG_INFINITY,
            limits_scale: None,
            screens: SYSTEM,
            restored_max: None,
            video_shape: None,
            shaped: None,
            sized: None,
            asked_from: None,
        }
    }

    /// Picture in picture takes the video's shape: Windows keeps it while an edge is
    /// dragged, and any other resize gets it once it's over (keeping the side changed),
    /// as does a video of another shape coming on.
    fn keep_shape(&mut self, ctx: &egui::Context, now: f64, still: bool) {
        let ratio = self.video_shape.filter(|_| still && self.showing() && video_active() && self.switch.is_none());
        #[cfg(windows)]
        {
            let px = ctx.pixels_per_point();
            *SHAPE.lock().unwrap() = ratio.map(|r| (r, VIDEO_MIN * px, VIDEO_MAX * px));
        }
        let Some(ratio) = ratio else {
            if !self.showing() {
                self.video_shape = None;
            }
            (self.shaped, self.sized, self.asked_from) = (None, None, None);
            return;
        };
        let size = ctx.content_rect().size();
        if (size.x / ratio - size.y).abs() < 1.5 {
            (self.shaped, self.asked_from) = (Some(size), None);
            return;
        }
        let since = match self.sized {
            Some((s, since)) if (s - size).length() < 0.5 => since,
            _ => {
                self.sized = Some((size, now));
                now
            }
        };
        if self.asked_from.is_some_and(|a| (a - size).length() < 0.5) {
            return;
        }
        // Still being dragged: not yet. (The app may not see the button let go after a
        // resize the system did, so the pointer can't say.)
        if MOVING.load(std::sync::atomic::Ordering::Relaxed) {
            self.sized = Some((size, now));
            ctx.request_repaint_after(Duration::from_millis(50));
            return;
        }
        let wait = SNAP_AFTER - (now - since);
        if wait > 0.0 {
            ctx.request_repaint_after(Duration::from_secs_f64(wait.max(0.05)));
            return;
        }
        let by_height = self.shaped.is_some_and(|from| height_led(from, size));
        let target = in_shape(ratio, size, by_height, VIDEO_MIN, VIDEO_MAX);
        log::debug!("picture in picture: {size:?} to the video's shape, {target:?}");
        self.asked_from = Some(size);
        ctx.send_viewport_cmd(ViewportCommand::InnerSize(target));
    }

    pub fn save(&self, storage: &mut dyn eframe::Storage) {
        storage.set_string("mini_pinned", if self.pinned { "1" } else { "0" }.into());
        for (key, place) in [("mini_window", &self.place), (VIDEO_KEY, &self.video_place)] {
            if let Some(p) = place {
                storage.set_string(key, serde_json::to_string(p).unwrap_or_default());
            }
        }
    }

    /// Ask for the strip (true) or the full window; started by [`Mini::take_request`].
    pub fn request(&mut self, on: bool) {
        self.wanted = Some(on);
    }

    /// The switch to start now, if any (not while one runs).
    pub fn take_request(&mut self) -> Option<bool> {
        if self.switch.is_some() {
            return None;
        }
        self.wanted.take()
    }

    /// Where it's headed once queued switches are done: what Ctrl+M flips.
    pub fn target(&self) -> bool {
        self.wanted.unwrap_or_else(|| self.active())
    }

    /// The strip is showing or on its way.
    pub fn active(&self) -> bool {
        self.switch.as_ref().map_or(self.on, |s| s.to_mini)
    }

    /// The strip is showing, or the window is switching either way: the full window's
    /// pages and now playing aren't up.
    pub fn busy(&self) -> bool {
        self.on || self.switch.is_some()
    }

    /// The strip is showing (not mid-switch).
    pub fn showing(&self) -> bool {
        self.on && self.switch.is_none()
    }

    /// The old layout stays until the window starts changing size.
    pub fn layout(&self, ctx: &egui::Context) -> Layout {
        match &self.switch {
            Some(sw) if (ctx.content_rect().size() - sw.from).length() > 1.0 => Layout::Blank,
            _ if self.on => Layout::Mini,
            _ => Layout::Full,
        }
    }

    /// The full window's place in eframe's format, while eframe's own would be wrong: with
    /// the strip up or on its way back (eframe's is the strip's), and while the full window
    /// is still maximized after it (eframe took the strip's as its normal size).
    pub fn full_window(&self) -> Option<String> {
        if self.active() || self.switch.is_some() {
            return self.full.map(|(p, max)| p.eframe_window(max));
        }
        self.restored_max.map(|p| p.eframe_window(true))
    }

    /// The new layout fades in from `base` (the background's own colour): drawn over
    /// everything, since pages and popups paint on layers of their own.
    pub fn fade_in(&self, ctx: &egui::Context, now: f64, base: Color32) {
        let t = ((now - self.shown_at) / 0.18).clamp(0.0, 1.0) as f32;
        if t >= 1.0 {
            return;
        }
        ctx.request_repaint();
        let layer = egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("mini-fade"));
        ctx.layer_painter(layer).rect_filled(ctx.content_rect(), 0.0, base.gamma_multiply(1.0 - theme::ease_out(t)));
    }

    /// This mini player's last place.
    fn place(&self) -> Option<Place> {
        if video_active() { self.video_place } else { self.place }
    }

    fn set_place(&mut self, p: Place) {
        if video_active() {
            self.video_place = Some(clamp_size(p));
        } else {
            self.place = Some(p);
        }
    }

    /// Switch to the strip or back. The full window leaves full screen first (the caller's).
    /// `video`: going to the mini player with a video on show, which it then plays.
    pub fn toggle(&mut self, ctx: &egui::Context, now: f64, video: bool) {
        if self.switch.is_some() {
            return;
        }
        let to_mini = !self.on;
        if to_mini {
            VIDEO.store(video, std::sync::atomic::Ordering::Relaxed);
        }
        // Named like a browser's picture in picture while it's one. Wayland lets no app put
        // itself above others: a KDE window rule for this title (and the app, BitChord) can.
        if video_active() {
            ctx.send_viewport_cmd(ViewportCommand::Title(if to_mini { "Picture in picture" } else { "BitChord" }.into()));
        }
        let mut step = Step::Resize;
        if to_mini {
            self.was_maximized = ctx.input(|i| i.viewport().maximized == Some(true));
            if self.was_maximized {
                ctx.send_viewport_cmd(ViewportCommand::Maximized(false));
                step = Step::Unmaximize;
            }
        }
        self.switch = Some(Switch { to_mini, step, since: now, from: ctx.content_rect().size() });
        ctx.request_repaint();
    }

    pub fn set_pinned(&mut self, ctx: &egui::Context, on: bool) {
        self.pinned = on;
        if self.active() {
            ctx.send_viewport_cmd(ViewportCommand::WindowLevel(if on { WindowLevel::AlwaysOnTop } else { WindowLevel::Normal }));
        }
    }

    /// Once a frame: drive a switch, note the strip's place, keep the size limits right.
    pub fn tick(&mut self, ctx: &egui::Context, now: f64) {
        let scale = ctx.input(|i| i.viewport().native_pixels_per_point);
        // winit keeps the limits in pixels: again after a move to another scale or a zoom.
        if self.limits_scale.is_some_and(|s| s != ctx.pixels_per_point()) && self.switch.is_none() {
            self.limits(ctx, self.on);
        }
        if self.restored_max.is_some() && self.switch.is_none() && ctx.input(|i| i.viewport().maximized == Some(false)) {
            self.restored_max = None;
        }
        let (minimized, maximized) = ctx.input(|i| (i.viewport().minimized == Some(true), i.viewport().maximized == Some(true)));
        // Win+Up or a drag onto the screen's top edge: the strip has one height.
        if self.showing() && maximized && !minimized {
            ctx.send_viewport_cmd(ViewportCommand::Maximized(false));
        }
        // Minimized, the platform says no place (and the strip's isn't one maximized).
        if self.showing()
            && !minimized
            && !maximized
            && let Some(p) = Place::current(ctx).filter(|p| p.w >= width_limits().0 - 1.0)
        {
            self.set_place(p);
        }
        self.keep_shape(ctx, now, !minimized && !maximized);
        let Some(sw) = &mut self.switch else { return };
        ctx.request_repaint_after(Duration::from_millis(16));
        let waited = now - sw.since;
        let to_mini = sw.to_mini;
        let next = match sw.step {
            Step::Unmaximize => {
                let down = ctx.input(|i| i.viewport().maximized != Some(true));
                // Its normal place, at once: closing before the next step still saves it.
                if down {
                    self.full = Place::current(ctx).map(|p| (p, true));
                }
                (down || waited > 0.5).then_some(Step::Resize)
            }
            Step::Resize => Some(self.resize(ctx, to_mini)),
            Step::Rescale(target) => (scale.is_some_and(|s| (s - target.scale).abs() < 0.01) || waited > 0.6).then(|| {
                self.limits(ctx, to_mini);
                let size = target.size_at(ctx.zoom_factor());
                ctx.send_viewport_cmd(ViewportCommand::InnerSize(size));
                Step::Settle(Some(size))
            }),
            Step::Settle(size) => {
                let done = match size {
                    Some(size) => (ctx.content_rect().size() - size).length() < 2.0,
                    None => ctx.input(|i| i.viewport().maximized == Some(true)),
                };
                if done || waited > 0.4 {
                    log::info!("{}: {:?}{}", if to_mini { "mini player" } else { "full window" }, Place::current(ctx), if done { "" } else { " (timed out)" });
                    self.on = to_mini;
                    if !to_mini {
                        self.restored_max = self.full.take().filter(|(_, max)| *max).map(|(p, _)| p);
                    }
                    self.switch = None;
                    self.shown_at = now;
                    return;
                }
                None
            }
        };
        if let (Some(step), Some(sw)) = (next, &mut self.switch) {
            sw.step = step;
            sw.since = now;
        }
    }

    /// Whether a place is (at least partly) on a connected monitor. Test runs live off-screen.
    fn visible(&self, p: Place, frame: Vec2) -> bool {
        // Its top middle, where it can be grabbed: a sliver on screen isn't enough.
        let r = p.rect_px(frame);
        let grip = Rect::from_center_size(pos2(r.center().x, r.top() + 12.0 * p.scale), vec2(r.width().min(120.0 * p.scale), 16.0 * p.scale));
        std::env::var_os("YTM_BACKGROUND").is_some() || (self.screens.on_a_monitor)(grip)
    }

    /// Move and size the window for the new mode; the step to wait in after.
    fn resize(&mut self, ctx: &egui::Context, to_mini: bool) -> Step {
        let now_place = Place::current(ctx);
        let frame = ctx.input(|i| {
            let vp = i.viewport();
            vp.outer_rect.zip(vp.inner_rect).map_or(Vec2::ZERO, |(o, i)| o.size() - i.size())
        });
        if to_mini {
            self.full = now_place.map(|p| (p, self.was_maximized));
            let target = match (self.place(), now_place) {
                // Test runs stay off-screen, where they are; YTM_MINI_W tries other widths.
                (_, Some(here)) if std::env::var_os("YTM_BACKGROUND").is_some() => {
                    let w = std::env::var("YTM_MINI_W").ok().and_then(|w| w.parse().ok()).unwrap_or(mini_size().x);
                    clamp_size(Place { w, h: mini_size().y, ..here })
                }
                (Some(p), _) if self.visible(p, frame) => p,
                (_, here) => {
                    let scale = here.map_or(1.0, |h| h.scale);
                    // Not Windows: the bottom right of the full window, where the player bar was.
                    let area = (self.screens.work_area)()
                        .or_else(|| here.map(|h| h.rect_px(frame)))
                        .unwrap_or(Rect::from_min_size(Pos2::ZERO, mini_size() * scale));
                    corner_place(area, frame, scale, ctx.zoom_factor())
                }
            };
            ctx.send_viewport_cmd(ViewportCommand::WindowLevel(if self.pinned { WindowLevel::AlwaysOnTop } else { WindowLevel::Normal }));
            self.go(ctx, target, false, true)
        } else {
            if let Some(p) = now_place {
                self.set_place(p);
            }
            ctx.send_viewport_cmd(ViewportCommand::WindowLevel(WindowLevel::Normal));
            // Kept until the window has settled: a save meanwhile still writes it.
            match self.full {
                Some((p, max)) if self.visible(p, frame) => self.go(ctx, p, max, false),
                // Its monitor is gone: its size, in the middle of the strip's monitor.
                full => {
                    let zoom = ctx.zoom_factor();
                    let size = full.map_or(Vec2::from(crate::DEFAULT_SIZE), |(p, _)| p.size_at(zoom));
                    let max = full.is_some_and(|(_, max)| max);
                    match ((self.screens.work_area)(), now_place) {
                        (Some(area), Some(here)) => {
                            // Its new place is the one to save (and restore down to).
                            let target = centered(area, size, frame, here.scale, zoom);
                            self.full = Some((target, max));
                            self.go(ctx, target, max, false)
                        }
                        _ => {
                            self.limits(ctx, false);
                            ctx.send_viewport_cmd(ViewportCommand::InnerSize(size));
                            Step::Settle(Some(size))
                        }
                    }
                }
            }
        }
    }

    /// Move first, then the limits (changing them can resize the window at once, which
    /// could carry it onto another monitor before the move), then the size.
    fn go(&mut self, ctx: &egui::Context, target: Place, maximize: bool, mini: bool) -> Step {
        let ppp = ctx.pixels_per_point();
        ctx.send_viewport_cmd(ViewportCommand::OuterPosition(pos2(target.x, target.y) / ppp));
        self.limits(ctx, mini);
        let scale = ctx.input(|i| i.viewport().native_pixels_per_point);
        if scale.is_some_and(|s| (s - target.scale).abs() >= 0.01) && !maximize {
            // Points mean another size there: wait for the move to land.
            return Step::Rescale(target);
        }
        let size = target.size_at(ctx.zoom_factor());
        ctx.send_viewport_cmd(ViewportCommand::InnerSize(size));
        if maximize {
            ctx.send_viewport_cmd(ViewportCommand::Maximized(true));
            return Step::Settle(None);
        }
        Step::Settle(Some(size))
    }

    /// The window's size limits for a mode. Shrinking sets them first, so the full
    /// window's minimum doesn't hold the strip up.
    fn limits(&mut self, ctx: &egui::Context, mini: bool) {
        if mini && video_active() {
            ctx.send_viewport_cmd(ViewportCommand::MinInnerSize(VIDEO_MIN));
            ctx.send_viewport_cmd(ViewportCommand::MaxInnerSize(VIDEO_MAX));
        } else if mini {
            let (min, max) = width_limits();
            ctx.send_viewport_cmd(ViewportCommand::MinInnerSize(vec2(min, mini_size().y)));
            ctx.send_viewport_cmd(ViewportCommand::MaxInnerSize(vec2(max, mini_size().y)));
        } else {
            ctx.send_viewport_cmd(ViewportCommand::MaxInnerSize(Vec2::INFINITY));
            ctx.send_viewport_cmd(ViewportCommand::MinInnerSize(Vec2::from(crate::MIN_SIZE)));
        }
        self.limits_scale = Some(ctx.pixels_per_point());
    }
}

/// The strip: cover, title and artists, like / previous / play / next / volume, a seek
/// line along the bottom, and pin / full window / close at the top right.
pub fn strip(app: &mut App, ui: &mut Ui, now: f64) {
    let ctx = ui.ctx().clone();
    let r = ctx.content_rect();
    // Its background moves the window; a double-click goes back to the full window.
    // The seek line's band along the bottom stays out of it, so a press there seeks
    // instead of dragging the window (and on macOS, where the strip keeps the system
    // frame, the bottom edge is easy to land on by mistake).
    let drag = Rect::from_min_max(r.min, pos2(r.right(), r.bottom() - SEEK_BAND));
    if titlebar::drag_strip(ui, drag, "mini").double_clicked() {
        app.actions.push(Action::Mini(false));
    }
    ui.painter().rect_filled(r, theme::thumb_radius() + 1.0, theme::panel(0.62));

    // An Automix handoff keeps the outgoing song here until the next one is louder.
    let hold = app.mix_hold().is_some();
    let track = if hold { app.shown().cloned() } else { app.current().cloned() };
    let pad = 12.0;
    let art = Rect::from_min_size(r.min + Vec2::splat(pad), Vec2::splat(r.height() - 2.0 * pad - 4.0));
    match &track {
        Some(t) => paint_cover(ui, art, &t.thumbs, theme::thumb_radius() + 1.0, 1.0),
        None => {
            ui.painter().rect_filled(art, theme::thumb_radius() + 1.0, theme::glass(0.08));
            icon_at(ui, art.center(), icon::MUSIC, 28.0, theme::text_faint());
        }
    }
    // The song's animated cover over the still: Apple's clip for its release, or
    // Spotify's canvas where Apple has none.
    if let Some(t) = &track {
        let px = (art.width() * ctx.pixels_per_point()).round().max(256.0) as u32;
        if let Some(s) = app.animated_cover(&ctx, t, px) {
            widgets::paint_motion(ui, art, theme::thumb_radius() + 1.0, &s);
        }
    }

    // Title and artists.
    let x0 = art.right() + 12.0;
    let x1 = r.right() - 10.0;
    let text_w = (x1 - x0).max(20.0);
    let (title, artists) = match &track {
        Some(t) => (t.title.clone(), t.artists.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(", ")),
        None => ("Nothing playing".to_owned(), String::new()),
    };
    let explicit = track.as_ref().is_some_and(|t| t.explicit);
    let tr = super::widgets::title_at(ui, pos2(x0, art.top() + 1.0), &title, explicit, theme::semibold(14.5), theme::text(), text_w);
    if let Some(t) = &track {
        named(ui.interact(tr, ui.scope_id().with("mini-title"), Sense::hover()), &super::widgets::song_name(&t.title, t.explicit)).on_hover_text(&t.title);
    }
    let ar = text_at(ui, pos2(x0, art.top() + 22.0), &artists, theme::regular(12.5), theme::text_dim(), text_w);
    // The quality glyph follows the artists, centred on their line, when there's room.
    let bw = super::nowplaying::audio_badge_icon_width(app, true);
    if bw > 0.0 && ar.right() + 8.0 + bw <= x1 {
        super::nowplaying::audio_badge_icon(app, ui, pos2(ar.right() + 8.0 + bw / 2.0, ar.center().y), true);
    }

    // Top right: pin, full window, close. They show while the pointer is on the strip (or
    // one of them has the keyboard), over a backdrop, so the title has the whole width.
    let scope = ui.scope_id();
    // By a key, not the label: the pin's changes, and its focus has to stay.
    let btn_id = move |key: &str| scope.with(("mini-btn", key));
    let focused = ctx.memory(|m| m.focused()).is_some_and(|f| ["close", "full", "pin"].iter().any(|k| btn_id(k) == f));
    let over = ctx.input(|i| i.pointer.hover_pos()).is_some_and(|p| r.contains(p));
    let reveal = theme::anim_bool(&ctx, "mini-btns", over || focused, 0.15);
    let count = if titlebar::custom() { 3.0 } else { 2.0 };
    let backdrop = Rect::from_min_max(pos2(r.right() - 6.0 - 30.0 * count - 2.0, r.top() + 2.0), pos2(r.right() - 2.0, r.top() + 38.0));
    // Opaque, so the title under it fades out instead of showing through.
    let pill = theme::lerp_color(super::shell::base_color(app), if theme::light() { Color32::WHITE } else { Color32::BLACK }, 0.55);
    ui.painter().rect_filled(backdrop, 9.0, pill.gamma_multiply(reveal));
    let mut bx = r.right() - 6.0;
    let mut top_btn = |ui: &mut Ui, glyph: char, key: &str, name: &str, close: bool, on: bool| -> bool {
        let b = Rect::from_min_size(pos2(bx - 28.0, r.top() + 6.0), Vec2::splat(28.0));
        bx -= 30.0;
        let resp = ui.interact(b, btn_id(key), Sense::click());
        let resp = named(resp, name).on_hover_text(name).on_hover_cursor(egui::CursorIcon::PointingHand);
        let h = theme::anim_bool(ui.ctx(), resp.id.with("h"), resp.hovered(), 0.12);
        let down = resp.is_pointer_button_down_on();
        if h > 0.0 {
            let fill = if close { Color32::from_rgb(0xC4, 0x2B, 0x1C).gamma_multiply(h) } else { theme::glass(if down { 0.16 } else { 0.10 } * h) };
            ui.painter().rect_filled(b, 6.0, fill.gamma_multiply(reveal));
        }
        let idle = if on { theme::text() } else { theme::text_dim() };
        let fg = theme::lerp_color(idle, if close { Color32::WHITE } else { theme::text() }, h);
        icon_at(ui, b.center(), glyph, if down { 13.0 } else { 14.0 }, fg.gamma_multiply(reveal));
        if resp.has_focus() {
            ui.painter().rect_stroke(b.shrink(1.0), 6.0, egui::Stroke::new(1.5, theme::text()), egui::StrokeKind::Inside);
        }
        // Not while still hidden: a quick click there was meant for the strip.
        resp.clicked() && (reveal > 0.5 || resp.has_focus())
    };
    // The system's frame has its own close button.
    if titlebar::custom() && top_btn(ui, icon::X, "close", "Close", true, false) {
        ctx.send_viewport_cmd(ViewportCommand::Close);
    }
    if top_btn(ui, icon::MAXIMIZE_2, "full", "Full window (Ctrl+M)", false, false) {
        app.actions.push(Action::Mini(false));
    }
    let pinned = app.mini.pinned;
    if top_btn(ui, if pinned { icon::PIN } else { icon::PIN_OFF }, "pin", if pinned { "Stop keeping on top" } else { "Keep on top" }, false, pinned) {
        app.mini.set_pinned(&ctx, !pinned);
    }

    // One group, centred under the text: like, previous, play, next, volume. Hovering
    // the volume slides a volume track over the group, as the player bar's does.
    let cy = art.bottom() - 17.0;
    let cx = (x0 + x1) / 2.0;
    let enabled = track.is_some();
    let vol = app.audio.volume();
    let vpos = pos2(cx + 88.0, cy);
    let vhit = Rect::from_center_size(vpos, Vec2::splat(32.0));
    let slider = Rect::from_min_max(pos2(cx - 88.0, cy - 10.0), pos2(cx + 58.0, cy + 10.0));
    let open_id = ui.scope_id().with("mini-vol-open");
    let was_open = ui.data(|d| d.get_temp::<bool>(open_id).unwrap_or(false));
    // Pointer position, not `rect_contains_pointer`: layers above the mini player's own
    // (window chrome, the drag regions) would otherwise hide the pointer from it.
    let hover = |r: Rect| ui.input(|i| i.pointer.hover_pos()).is_some_and(|p| r.contains(p));
    let band = Rect::from_min_max(pos2(slider.left() - 4.0, cy - 14.0), pos2(vhit.right(), cy + 14.0));
    let open = hover(vhit) || ui.rect_contains_pointer(vhit) || (was_open && (hover(band) || ui.input(|i| i.pointer.any_down())));
    ui.data_mut(|d| d.insert_temp(open_id, open));
    if open != was_open {
        ctx.request_repaint();
    }
    let reveal = theme::anim_bool(&ctx, "mini-vol-reveal", open, 0.15);
    if reveal < 0.5 {
        // The same glyphs the player bar draws, a size down.
        let small = |ui: &mut Ui, c: Pos2, key: &str, glyph: &'static [u8], name: &str| -> bool {
            let b = Rect::from_center_size(c, Vec2::splat(34.0));
            let resp = ui.interact(b, ui.scope_id().with(("mini-t", key)), Sense::click());
            let resp = named(resp, name).on_hover_text(name).on_hover_cursor(egui::CursorIcon::PointingHand);
            let h = theme::anim_bool(ui.ctx(), resp.id.with("h"), resp.hovered(), 0.15);
            glyphs::draw(ui, c, 20.0, glyph, theme::lerp_color(theme::text_dim(), theme::text(), h));
            resp.clicked()
        };
        if small(ui, pos2(cx - 44.0, cy), "prev", glyphs::PREVIOUS, "Previous") && enabled {
            app.prev();
        }
        if small(ui, pos2(cx + 44.0, cy), "next", glyphs::NEXT, "Next") && enabled {
            app.next();
        }
        let playing = app.playing();
        let pr = Rect::from_center_size(pos2(cx, cy), Vec2::splat(40.0));
        let presp = named(ui.interact(pr, ui.scope_id().with("mini-play"), Sense::click()), if playing { "Pause" } else { "Play" }).on_hover_cursor(egui::CursorIcon::PointingHand);
        let ph = theme::anim_bool(&ctx, presp.id.with("h"), presp.hovered(), 0.15);
        let c = pr.center();
        let morph = theme::anim_bool(&ctx, "mini-play-morph", playing || app.buffering, 0.18);
        let fg = theme::lerp_color(theme::text_dim(), theme::text(), 0.55 + 0.45 * ph);
        if app.buffering {
            let a = now as f32 * 6.0;
            let pts = (0..20).map(|i| c + vec2((a + i as f32 * 0.22).cos(), (a + i as f32 * 0.22).sin()) * 7.0).collect();
            ui.painter().add(egui::Shape::Path(egui::epaint::PathShape::line(pts, egui::epaint::PathStroke::new(2.0, fg))));
            ctx.request_repaint_after(Duration::from_millis(33));
        } else {
            if morph < 1.0 {
                glyphs::draw(ui, c, 26.0, glyphs::PLAY, theme::with_alpha(fg, 1.0 - morph));
            }
            if morph > 0.0 {
                glyphs::draw(ui, c, 26.0, glyphs::PAUSE, theme::with_alpha(fg, morph));
            }
        }
        if presp.clicked() && enabled {
            app.toggle();
        }

        if let Some(t) = &track {
            // What's heard, like Now playing's menu: a song version standing in for the music video.
            let t = if hold { t.clone() } else { app.playing_track().cloned().unwrap_or_else(|| t.clone()) };
            let liked = app.liked.contains(&t.id);
            let lpos = pos2(cx - 88.0, cy);
            let lresp = ui.interact(Rect::from_center_size(lpos, Vec2::splat(32.0)), ui.scope_id().with("mini-like"), Sense::click());
            let name = if liked { "Remove like" } else { "Like" };
            let lresp = named(lresp, name).on_hover_text(name).on_hover_cursor(egui::CursorIcon::PointingHand);
            let lh = theme::anim_bool(&ctx, lresp.id.with("h"), lresp.hovered(), 0.15);
            let pop = theme::anim_bool(&ctx, ("liked", &t.id), liked, 0.3);
            // A liked song keeps a soft disc behind its star, as the player bar's does.
            let disc = (0.17 * pop + 0.10 * lh).min(0.30);
            if disc > 0.0 {
                ui.painter().circle_filled(lpos, 16.0, theme::glass(disc));
            }
            let size = 18.0 * (1.0 + 0.25 * (pop * std::f32::consts::PI).sin());
            let tint = theme::lerp_color(theme::lerp_color(theme::text_dim(), theme::text(), lh), app.accent_color(), pop);
            glyphs::draw(ui, lpos, size, glyphs::STAR, tint);
            if pop > 0.0 {
                glyphs::draw(ui, lpos, size, glyphs::STAR_FILLED, theme::with_alpha(app.accent_color(), pop));
            }
            if lresp.clicked() {
                app.actions.push(Action::Like(t.clone(), !liked));
            }
        }
    } else {
        let track_r = Rect::from_center_size(slider.center(), vec2(slider.width(), 4.0));
        ui.painter().rect_filled(track_r, 2.0, theme::glass(0.16));
        let fill = Rect::from_min_size(track_r.min, vec2(track_r.width() * vol, 4.0));
        ui.painter().rect_filled(fill, 2.0, theme::text());
        ui.painter().circle_filled(pos2(fill.right(), track_r.center().y), 6.0, theme::text());
        let sresp = ui.interact(slider.expand2(vec2(0.0, 8.0)), ui.scope_id().with("mini-vol-slider"), Sense::click_and_drag());
        let sresp = sresp.on_hover_cursor(egui::CursorIcon::PointingHand);
        sresp.widget_info(|| egui::WidgetInfo::slider(true, vol as f64, "Volume"));
        if let Some(p) = sresp.interact_pointer_pos()
            && (sresp.clicked() || sresp.dragged())
        {
            app.audio.set_volume(((p.x - track_r.left()) / track_r.width()).clamp(0.0, 1.0));
        }
    }

    // Volume: hover slides the track out, click mutes, scroll to change.
    let vol = app.audio.volume();
    let vresp = ui.interact(vhit, ui.scope_id().with("mini-vol"), Sense::click());
    let pct = format!("Volume {}%", (vol * 100.0).round());
    let vresp = named(vresp, "Mute").on_hover_text(pct).on_hover_cursor(egui::CursorIcon::PointingHand);
    let vh = theme::anim_bool(&ctx, vresp.id.with("h"), vresp.hovered(), 0.15);
    ui.painter().circle_filled(vpos, 16.0, theme::glass(0.1 * vh));
    let vglyph = glyphs::VOLUME[glyphs::volume_step(vol)];
    glyphs::draw(ui, vpos, 18.0, vglyph, theme::lerp_color(theme::text_dim(), theme::text(), vh));
    if vresp.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            app.audio.set_volume((vol + scroll / 600.0).clamp(0.0, 1.0));
        }
    }
    if vresp.clicked() {
        let stash = super::shell::unmute_id();
        if vol > 0.0 {
            ui.data_mut(|d| d.insert_temp(stash, vol));
            app.audio.set_volume(0.0);
        } else {
            app.audio.set_volume(ui.data(|d| d.get_temp::<f32>(stash)).unwrap_or(0.6));
        }
    }

    seek_line(app, ui, r, track.as_ref());
}

/// Picture in picture: the video filling the window (its cover for a song that plays
/// after it). Pointed at, previous / play / next over it, pin, the full window and close
/// at the top right, and a seek line. Drag to move, double-click for the full window.
pub fn video_pip(app: &mut App, ui: &mut Ui, _now: f64) {
    let ctx = ui.ctx().clone();
    let r = ctx.content_rect();
    if titlebar::drag_strip(ui, r, "pip").double_clicked() {
        app.actions.push(Action::Mini(false));
    }
    ui.painter().rect_filled(r, 0.0, Color32::BLACK);
    let track = app.current().cloned();
    let video = app.shown_video().cloned();
    let frame = video.as_ref().and_then(|t| super::nowplaying::video_frame(app, &ctx, t));
    // The window takes its shape (see `keep_shape`); a song without a video leaves it be.
    app.mini.video_shape = frame.as_ref().map(|tex| tex.size_vec2()).filter(|s| s.x > 0.0 && s.y > 0.0).map(|s| s.x / s.y);
    match (&frame, &video, &track) {
        (Some(tex), _, _) => {
            let size = tex.size_vec2();
            let fit = size * (r.width() / size.x).min(r.height() / size.y);
            egui::Image::from_texture((tex.id(), fit)).paint_at(ui, Rect::from_center_size(r.center(), fit));
        }
        (None, Some(v), _) => {
            let fit = vec2(16.0, 9.0) * (r.width() / 16.0).min(r.height() / 9.0);
            super::nowplaying::video_placeholder(ui, Rect::from_center_size(r.center(), fit), v, 0.0);
        }
        (None, None, Some(t)) => paint_cover(ui, Rect::from_center_size(r.center(), Vec2::splat(r.height() - 24.0)), &t.thumbs, theme::thumb_radius() + 2.0, 1.0),
        (None, None, None) => icon_at(ui, r.center(), icon::MUSIC, 32.0, theme::text_faint()),
    }

    // The controls, while pointed at (or one has the keyboard).
    let keys = ["prev", "play", "next", "pin", "full", "close"];
    let btn_id = |key: &str| egui::Id::unique(("pip-btn", key));
    let focused = ctx.memory(|m| m.focused()).is_some_and(|f| keys.iter().any(|k| btn_id(k) == f));
    let over = ctx.input(|i| i.pointer.hover_pos()).is_some_and(|p| r.contains(p));
    let reveal = theme::anim_bool(&ctx, "pip-controls", over || focused, 0.15);
    if reveal <= 0.0 {
        return;
    }
    ui.painter().rect_filled(r, 0.0, Color32::from_black_alpha((110.0 * reveal) as u8));
    let enabled = track.is_some();
    let btn = |ui: &mut Ui, c: Pos2, side: f32, key: &str, name: &str| -> egui::Response {
        let b = Rect::from_center_size(c, Vec2::splat(side));
        let resp = named(ui.interact(b, btn_id(key), Sense::click()), name).on_hover_text(name).on_hover_cursor(egui::CursorIcon::PointingHand);
        let h = theme::anim_bool(ui.ctx(), resp.id.with("h"), resp.hovered(), 0.12);
        if h > 0.0 {
            ui.painter().circle_filled(c, side / 2.0, Color32::WHITE.gamma_multiply(0.16 * h * reveal));
        }
        if resp.has_focus() {
            ui.painter().circle_stroke(c, side / 2.0 - 1.0, egui::Stroke::new(1.5, Color32::WHITE));
        }
        resp
    };
    // Not while still fading in: a quick click there was meant for the window.
    let live = |resp: &egui::Response| resp.clicked() && (reveal > 0.5 || resp.has_focus());
    let white = Color32::WHITE.gamma_multiply(reveal);

    // Previous, play, next, in the middle.
    let c = r.center();
    let gap = (r.width() * 0.16).clamp(48.0, 84.0);
    let prev = btn(ui, c - vec2(gap, 0.0), 44.0, "prev", "Previous");
    glyphs::draw(ui, c - vec2(gap, 0.0), 22.0, glyphs::PREVIOUS, white);
    if live(&prev) && enabled {
        app.prev();
    }
    let next = btn(ui, c + vec2(gap, 0.0), 44.0, "next", "Next");
    glyphs::draw(ui, c + vec2(gap, 0.0), 22.0, glyphs::NEXT, white);
    if live(&next) && enabled {
        app.next();
    }
    let play = btn(ui, c, 56.0, "play", if app.playing() { "Pause" } else { "Play" });
    ui.painter().circle_filled(c, 26.0, white);
    let black = Color32::BLACK.gamma_multiply(reveal);
    let morph = theme::anim_bool(&ctx, "pip-play-morph", app.playing() || app.buffering, 0.18);
    if morph < 1.0 {
        glyphs::draw(ui, c, 26.0, glyphs::PLAY, theme::with_alpha(black, 1.0 - morph));
    }
    if morph > 0.0 {
        glyphs::draw(ui, c, 26.0, glyphs::PAUSE, theme::with_alpha(black, morph));
    }
    if live(&play) && enabled {
        app.toggle();
    }

    // Top right: close (with no system frame to have one), the full window, pin.
    let mut x = r.right() - 22.0;
    let mut corner = |ui: &mut Ui, glyph: char, key: &str, name: &str| -> bool {
        let resp = btn(ui, pos2(x, r.top() + 22.0), 32.0, key, name);
        icon_at(ui, pos2(x, r.top() + 22.0), glyph, 16.0, white);
        x -= 36.0;
        live(&resp)
    };
    if titlebar::custom() && corner(ui, icon::X, "close", "Close") {
        ctx.send_viewport_cmd(ViewportCommand::Close);
    }
    if corner(ui, icon::MAXIMIZE_2, "full", "Full window (Ctrl+M)") {
        app.actions.push(Action::Mini(false));
    }
    let pinned = app.mini.pinned;
    if corner(ui, if pinned { icon::PIN } else { icon::PIN_OFF }, "pin", if pinned { "Stop keeping on top" } else { "Keep on top" }) {
        app.mini.set_pinned(&ctx, !pinned);
    }

    let mut faded = ui.new_child(egui::UiBuilder::new().max_rect(r));
    faded.multiply_opacity(reveal);
    seek_line(app, &mut faded, r, track.as_ref());
}

/// A line along the bottom; thicker on hover, with the time under the pointer.
fn seek_line(app: &mut App, ui: &mut Ui, r: Rect, track: Option<&crate::model::Track>) {
    let status = app.audio.status.clone();
    let hold = app.mix_hold().map(|h| (h.total, app.mix_hold_pos(h)));
    let total = match &hold {
        Some((total, _)) => *total,
        None => status.duration().map(|d| d.as_secs_f32()).or(track.and_then(|t| t.duration).map(|d| d as f32)).unwrap_or(0.0),
    };
    let smooth = match &hold {
        Some((_, pos)) => *pos,
        None => status.position_smooth().as_secs_f32(),
    };
    let held = hold.is_some();
    let pos = app.seek_drag.unwrap_or(smooth).min(total.max(0.0));
    // Runs the window's full width, like Apple's strip; the corner radius is small
    // enough that the bottom edge rounds it off on its own. The band is taller than
    // the line — and clear of the transport above it — so a press anywhere near the
    // line lands on the seek, however imprecise the pointer (a trackpad especially).
    let (l, rr) = (r.left(), r.right());
    let hit = Rect::from_min_max(pos2(l, r.bottom() - SEEK_BAND), pos2(rr, r.bottom()));
    let resp = ui.interact(hit, ui.scope_id().with("mini-seek"), if held { Sense::hover() } else { Sense::click_and_drag() });
    let resp = if total > 0.0 && !held { resp.on_hover_cursor(egui::CursorIcon::PointingHand) } else { resp };
    resp.widget_info(|| egui::WidgetInfo::slider(total > 0.0 && !held, pos as f64, "Seek"));
    let h = theme::anim_bool(ui.ctx(), "mini-seek-h", total > 0.0 && !held && (resp.hovered() || resp.dragged()), 0.15);
    let thick = 4.0 + 3.0 * h;
    let line = Rect::from_min_max(pos2(l, r.bottom() - thick), pos2(rr, r.bottom()));
    ui.painter().rect_filled(line, 0.0, theme::glass(0.2));
    let frac = if total > 0.0 { pos / total } else { 0.0 };
    ui.painter().rect_filled(Rect::from_min_size(line.min, vec2(line.width() * frac, thick)), 0.0, app.accent_color());
    let at = |p: Pos2| ((p.x - l) / (rr - l).max(1.0)).clamp(0.0, 1.0) * total;
    if h > 0.0
        && let Some(p) = resp.hover_pos().or(resp.interact_pointer_pos())
    {
        let txt = format!("{} / {}", fmt_time(at(p) as u32), fmt_time(total as u32));
        let g = widgets::time_galley(ui, &txt, theme::semibold(11.5), Color32::WHITE);
        let half = g.size().x / 2.0 + 6.0;
        let x = p.x.clamp(r.left() + half + 2.0, r.right() - half - 2.0);
        let bg = Rect::from_center_size(pos2(x, line.top() - 13.0), g.size() + vec2(12.0, 5.0));
        ui.painter().rect_filled(bg, 5.0, theme::shade(0.8).gamma_multiply(h));
        ui.painter().galley(bg.min + vec2(6.0, 2.5), g, Color32::WHITE.gamma_multiply(h));
    }
    if total > 0.0
        && let Some(p) = resp.interact_pointer_pos()
    {
        if resp.dragged() {
            app.seek_drag = Some(at(p));
        }
        if resp.drag_stopped() || resp.clicked() {
            app.seek_drag = None;
            app.actions.push(Action::Seek(at(p)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picture_in_picture_takes_the_videos_shape_within_its_limits() {
        let wide = 16.0 / 9.0;
        // A 4:3 video in the first 16:9 window: the width stays.
        assert_eq!(in_shape(4.0 / 3.0, vec2(480.0, 270.0), false, VIDEO_MIN, VIDEO_MAX), vec2(480.0, 360.0));
        // Made taller: the height stays.
        assert_eq!(in_shape(wide, vec2(480.0, 360.0), true, VIDEO_MIN, VIDEO_MAX), vec2(640.0, 360.0));
        // Never past the limits, in either direction, and still the video's shape.
        assert_eq!(in_shape(wide, vec2(2000.0, 100.0), false, VIDEO_MIN, VIDEO_MAX), vec2(1280.0, 720.0));
        assert_eq!(in_shape(wide, vec2(100.0, 100.0), false, VIDEO_MIN, VIDEO_MAX), vec2(240.0, 135.0));
        // A tall video can't be wider than the tallest window allows.
        let tall = in_shape(9.0 / 16.0, vec2(480.0, 270.0), false, VIDEO_MIN, VIDEO_MAX);
        assert!(tall.y <= VIDEO_MAX.y + 0.01 && (tall.x / tall.y - 9.0 / 16.0).abs() < 0.001);
    }

    #[test]
    fn the_side_a_resize_changed_more_is_the_one_kept() {
        assert!(height_led(vec2(480.0, 270.0), vec2(490.0, 400.0)));
        assert!(!height_led(vec2(480.0, 270.0), vec2(700.0, 280.0)));
        // Both by the same share: the width.
        assert!(!height_led(vec2(400.0, 200.0), vec2(800.0, 400.0)));
    }

    #[test]
    fn first_place_is_the_work_areas_corner() {
        let area = Rect::from_min_max(pos2(0.0, 0.0), pos2(2560.0, 1400.0));
        let p = corner_place(area, Vec2::ZERO, 1.0, 1.0);
        assert_eq!((p.x, p.y, p.w, p.h), (2560.0 - 400.0 - 16.0, 1400.0 - 112.0 - 16.0, SIZE.x, SIZE.y));
        // Scaled: pixels grow with the scale, points don't.
        let p = corner_place(area, vec2(0.0, 0.0), 1.5, 1.0);
        assert_eq!((p.x, p.y, p.w), (2560.0 - 600.0 - 24.0, 1400.0 - 168.0 - 24.0, SIZE.x));
        // A second monitor left of the main one has negative coordinates.
        let left = Rect::from_min_max(pos2(-1920.0, 100.0), pos2(0.0, 1140.0));
        let p = corner_place(left, vec2(2.0, 2.0), 1.0, 1.0);
        assert_eq!((p.x, p.y), (-402.0 - 16.0, 1140.0 - 114.0 - 16.0));
        // Never past the area's top left, however small it is.
        let tiny = Rect::from_min_max(pos2(10.0, 20.0), pos2(200.0, 50.0));
        let p = corner_place(tiny, Vec2::ZERO, 1.0, 1.0);
        assert_eq!((p.x, p.y), (10.0, 20.0));
        // Zoomed in, a point is bigger: the strip still ends at the margin.
        let p = corner_place(area, Vec2::ZERO, 1.0, 2.0);
        assert_eq!((p.x, p.y, p.zoom), (2560.0 - 800.0 - 16.0, 1400.0 - 224.0 - 16.0, 2.0));
    }

    #[test]
    fn a_lost_full_window_is_centred() {
        let area = Rect::from_min_max(pos2(1920.0, 0.0), pos2(3840.0, 1040.0));
        let p = centered(area, vec2(1200.0, 780.0), Vec2::ZERO, 1.0, 1.0);
        assert_eq!((p.x, p.y, p.w, p.h), (1920.0 + 360.0, 130.0, 1200.0, 780.0));
        // Bigger than the area: its top left stays on it.
        let p = centered(area, vec2(1000.0, 800.0), Vec2::ZERO, 2.0, 1.0);
        assert_eq!((p.x, p.y), (1920.0, 0.0));
    }

    #[test]
    fn remembered_sizes_stay_in_limits() {
        let p = Place { x: 5.0, y: 6.0, w: 2000.0, h: 300.0, scale: 1.25, zoom: 1.0 };
        assert_eq!(clamp_size(p), Place { w: MAX_W, h: SIZE.y, ..p });
        assert_eq!(clamp_size(Place { w: 10.0, ..p }).w, MIN_W);
    }

    #[test]
    fn full_window_is_saved_in_eframes_format() {
        let p = Place { x: 814.0, y: -183.0, w: 1200.0, h: 812.0, scale: 1.0, zoom: 1.0 };
        assert_eq!(
            p.eframe_window(false),
            "(normal_geometry:(outer_position_pixels:Some((x:814.0,y:-183.0)),scale_factor:1.0,inner_size_points:(x:1200.0,y:812.0)),fullscreen:false,maximized:false)"
        );
        let s = p.eframe_window(true);
        assert!(s.ends_with("maximized:true)"));
    }

    #[test]
    fn places_round_trip_through_settings() {
        let p = Place { x: -1500.0, y: 900.0, w: 420.0, h: SIZE.y, scale: 1.5, zoom: 1.25 };
        let back: Place = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(back, p);
        // Saved before places had a zoom: unzoomed.
        let old: Place = serde_json::from_str(r#"{"x":1.0,"y":2.0,"w":400.0,"h":112.0,"scale":1.0}"#).unwrap();
        assert_eq!(old.zoom, 1.0);
        // A full window kept at 100% comes back the same size in pixels at 125%.
        assert_eq!(Place { w: 1250.0, h: 1000.0, zoom: 1.0, ..p }.size_at(1.25), vec2(1000.0, 800.0));
        let m = Mini::load(|k| (k == "mini_window").then(|| serde_json::to_string(&p).unwrap()));
        assert_eq!(m.place, Some(p));
        assert_eq!(m.pinned, PINNED_BY_DEFAULT);
        let m = Mini::load(|k| (k == "mini_pinned").then(|| "0".to_owned()));
        assert!(!m.pinned && m.place.is_none() && !m.active());
    }
}
