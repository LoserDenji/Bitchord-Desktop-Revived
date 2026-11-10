//! The dynamic island: a floating now-playing pill that lives in its own always-on-top
//! window, over the system and other apps — not inside the app's window. It stands out
//! only while the app isn't looked at (another window has focus, or the app window is
//! minimized or hidden), and folds away the moment the app is back in focus.
//!
//! The behaviour is the in-app island's: a mini pill (artwork, title, remaining time, a
//! hair of progress) that expands into a full card (scrubber, transport) under the
//! user's own click and folds back after a few seconds' dwell; a new track never
//! expands it, and a right-click puts it away until the song changes. The card's title
//! or artwork brings the app back, and the transport and scrubber work without it.
//!
//! It's a deferred viewport: its own window repaints on its own clock (the hair of
//! progress, the tape), so it keeps living even while the app window is minimized and
//! eframe runs no pass for it. The app hands over what to show each pass of its own and
//! takes commands back over a channel (see [`Cmd`]).

use std::sync::{Arc, Mutex};

use crossbeam_channel::Sender;
use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Ui, Vec2, ViewportBuilder, ViewportId, pos2, vec2};

use super::widgets;
use super::App;
use crate::audio::AudioHandle;
use crate::theme;

/// How long an expanded card stays out on its own (the dwell time).
const COLLAPSE_AFTER: f64 = 5.0;
/// The pill: how tall, and how wide at least.
const PILL_H: f32 = 40.0;
const PILL_MIN_W: f32 = 216.0;
/// The expanded card's size.
const CARD_W: f32 = 400.0;
const CARD_H: f32 = 148.0;
/// The island window's size: the card, with room for the drop shadow under it.
const WINDOW_W: f32 = CARD_W + 36.0;
const WINDOW_H: f32 = CARD_H + 26.0;
/// The hairline of progress along the pill's bottom edge.
const PROGRESS_H: f32 = 2.0;
/// The title tape's pace, when a name doesn't fit (px/s, the island's "slowly").
const TAPE_SPEED: f32 = 24.0;
/// How long the tape rests at each end before turning around, in seconds.
const TAPE_REST: f32 = 1.6;

/// The island's own window.
pub fn viewport() -> ViewportId {
    ViewportId::from_hash_of(egui::Id::unique("bitchord-island"))
}

/// What the pill shows, handed over by the app each pass of its own.
pub struct TrackInfo {
    pub title: String,
    pub artist: String,
    pub thumbs: Vec<crate::model::Thumb>,
}

/// A click in the island window, for the app to carry out on its next pass.
pub enum Cmd {
    /// Bring the app back to the front (it's minimized, hidden or unfocused).
    Restore,
    Toggle,
    Next,
    Prev,
    Seek(f32),
}

/// The island's state, shared between the app and the island window: the app writes
/// what to show (track, colours, where the pill hangs, whether it's wanted at all);
/// the window writes back what it did (card out, a right-click dismissal, a scrub drag).
pub struct Island {
    // app-written
    pub track: Option<TrackInfo>,
    pub accent: Color32,
    /// The app window's last known top centre (in screen points): where the pill hangs.
    pub site: Pos2,
    /// A handle on the player, for the live position and the play state.
    pub audio: Option<AudioHandle>,
    /// The app isn't in focus (or is minimized/hidden): the island stands out alone.
    pub visible: bool,

    // island-window-written
    /// The full card is out (a click put it there; nothing else does).
    pub expanded: bool,
    /// App time when it expanded (and when the track last changed): the dwell runs from it.
    pub dwell_since: f64,
    /// Where a scrubber drag holds the card's position, in seconds.
    pub seek: Option<f32>,
    /// The track's id a right-click dismissal belongs to: the island stays away until
    /// the song changes, never coming back on a change by itself.
    pub dismissed_for: Option<String>,
    /// The id of the track last seen, so a change can restart the dwell.
    pub track_id: Option<String>,
    /// The show animation's last value: the app keeps the window up while it fades out.
    pub fade: f32,
}

impl Default for Island {
    fn default() -> Self {
        Self {
            track: None,
            accent: theme::RED,
            site: Pos2::ZERO,
            audio: None,
            visible: false,
            expanded: false,
            dwell_since: 0.0,
            seek: None,
            dismissed_for: None,
            track_id: None,
            fade: 0.0,
        }
    }
}

impl Island {
    /// Fold the card back in (the dwell, a click elsewhere, or the track ending).
    pub fn collapse(&mut self) {
        self.expanded = false;
        self.seek = None;
    }
}

/// The island's state, owned jointly by the app and the island window.
pub type Shared = Arc<Mutex<Island>>;

/// The app's side, each pass: hand over what to show and put the window up (or let it
/// go once its fade has run). Runs from the app's own UI passes — including the odd
/// one the island's own visibility keeps alive while the app window is hidden.
pub fn window(app: &mut App, ctx: &egui::Context, now: f64) {
    let playing = app.playing() || app.buffering;
    // What's shown, owned up front: the shared state takes a copy.
    let track = app.shown().map(|t| TrackInfo { title: t.title.clone(), artist: t.artist_line(), thumbs: t.thumbs.clone() });
    let track_id = app.shown().map(|t| t.id.clone());
    let np = theme::anim_bool(ctx, "np-open", app.np_open && app.current().is_some(), 0.42);
    // The island is for when the app isn't looked at: no focus, or no window at all.
    let looked_at = !app.window_hidden && ctx.input(|i| i.viewport().focused) != Some(false);
    let visible = {
        let mut s = app.island.lock().unwrap();
        // A new track never expands the island, but one that's out refreshes its dwell,
        // and a right-click dismissal belongs to the song it was made on.
        if s.track_id.as_deref() != track_id.as_deref() {
            s.track_id = track_id.clone();
            s.dwell_since = now;
        }
        s.track = track;
        s.accent = app.accent_color();
        s.audio.get_or_insert_with(|| app.audio.clone());
        if !app.window_hidden && let Some(r) = ctx.input(|i| i.viewport().outer_rect) {
            s.site = pos2(r.center().x, (r.top() + 8.0).max(8.0));
        }
        let dismissed = track_id.as_deref().is_some_and(|id| s.dismissed_for.as_deref() == Some(id));
        app.island_on
            && !app.fullscreen
            && !app.mini.busy()
            && np < 0.6
            && !looked_at
            && !dismissed
            && (playing || s.expanded)
    };
    let mut s = app.island.lock().unwrap();
    s.visible = visible;
    let declare = visible || s.fade > 0.001;
    let site = s.site;
    drop(s);
    if !declare {
        return;
    }
    let shared = app.island.clone();
    let tx = app.island_tx.clone();
    let builder = ViewportBuilder::default()
        .with_title("BitChord")
        .with_decorations(false)
        .with_transparent(true)
        .with_resizable(false)
        // A look shouldn't steal the focus from whatever the listener is doing.
        .with_active(false)
        .with_always_on_top()
        .with_taskbar(false)
        .with_has_shadow(false)
        .with_position(pos2(site.x - WINDOW_W * 0.5, site.y))
        .with_inner_size(vec2(WINDOW_W, WINDOW_H));
    ctx.show_viewport_deferred(viewport(), builder, move |ui, _| paint(shared.clone(), tx.clone(), ui));
}

/// The island window itself: the pill's face for `1 - k` of the frame, the card's for
/// `k`. Runs on the island window's own passes, whether or not the app is drawing.
fn paint(shared: Shared, tx: Sender<Cmd>, ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    let now = ctx.input(|i| i.time);
    // One read of everything the frame needs; the lock never spans a repaint request.
    let Some((track, track_id, accent, audio)) = (|| {
        let s = shared.lock().unwrap();
        let t = s.track.as_ref()?;
        Some((TrackInfo { title: t.title.clone(), artist: t.artist.clone(), thumbs: t.thumbs.clone() }, s.track_id.clone().unwrap_or_default(), s.accent, s.audio.clone()))
    })() else {
        shared.lock().unwrap().fade = 0.0;
        return;
    };
    let (visible, expanded, dwell_since) = {
        let s = shared.lock().unwrap();
        (s.visible, s.expanded, s.dwell_since)
    };
    let t = theme::anim_bool(&ctx, "island-show", visible, 0.3);
    shared.lock().unwrap().fade = t;
    if t <= 0.001 {
        if expanded {
            shared.lock().unwrap().collapse();
        }
        return;
    }
    // The dwell folds the card back in on its own; under the pointer it waits.
    let card = card_rect();
    let hovering = ctx.pointer_hover_pos().is_some_and(|p| card.contains(p));
    if expanded && now - dwell_since > COLLAPSE_AFTER && !hovering {
        shared.lock().unwrap().collapse();
    }
    // Esc folds an open card back in (when the island window itself has the keyboard).
    if expanded && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        shared.lock().unwrap().collapse();
    }
    let k = theme::anim_bool(&ctx, "island-expand", expanded, 0.32);
    let rect = pill_rect().lerp_inside(card, k);
    let tape_overflow = tape_overflow(&track.title, rect.width() * 0.5);
    paint_island(&shared, &tx, ui, &track, track_id.as_str(), accent, audio.as_ref(), rect, k, t, now);
    // Frames while something moves; otherwise just the progress hair's slow tick.
    let busy = expanded || k < 1.0 || t < 1.0 || tape_overflow;
    ctx.request_repaint_after(std::time::Duration::from_millis(if busy { 16 } else { 250 }));
}

/// The pill's rect in the island window, top centre.
fn pill_rect() -> Rect {
    let screen_w = WINDOW_W;
    Rect::from_min_size(pos2(screen_w * 0.5 - PILL_MIN_W * 0.5, 0.0), vec2(PILL_MIN_W, PILL_H))
}

/// The expanded card's rect, sharing the pill's top centre.
fn card_rect() -> Rect {
    Rect::from_min_size(pos2(WINDOW_W * 0.5 - CARD_W * 0.5, 0.0), vec2(CARD_W, CARD_H))
}

trait LerpRect {
    fn lerp_inside(self, to: Rect, k: f32) -> Rect;
}

impl LerpRect for Rect {
    fn lerp_inside(self, to: Rect, k: f32) -> Rect {
        let lerp = |a: f32, b: f32| a + (b - a) * k;
        Rect::from_min_max(pos2(lerp(self.left(), to.left()), lerp(self.top(), to.top())), pos2(lerp(self.right(), to.right()), lerp(self.bottom(), to.bottom())))
    }
}

/// Whether the title crawls (so the island window keeps animating while it does).
fn tape_overflow(title: &str, tape_w: f32) -> bool {
    // Rough: a point of semibold 13 is a bit over half a character of average width.
    (title.chars().count() as f32 * 6.8) > tape_w.max(1.0) + 4.0
}

/// Whether the player is playing (the audio status is live even when the app is hidden).
fn playing(audio: Option<&AudioHandle>) -> bool {
    audio.is_some_and(|a| a.status.playing.load(std::sync::atomic::Ordering::Relaxed))
}

/// The island's surface, and everything on it. All clicks go through `tx` (the app
/// carries them out on its next pass) or straight into the shared state.
#[allow(clippy::too_many_arguments)]
fn paint_island(
    shared: &Shared,
    tx: &Sender<Cmd>,
    ui: &mut Ui,
    track: &TrackInfo,
    track_id: &str,
    accent: Color32,
    audio: Option<&AudioHandle>,
    rect: Rect,
    k: f32,
    t: f32,
    now: f64,
) {
    let response = ui.interact(rect, egui::Id::unique("island-hit"), Sense::click());
    if response.secondary_clicked() {
        // Right-click puts it away for this stretch of playback; it comes back when
        // the song changes, never on the change itself.
        let mut s = shared.lock().unwrap();
        s.dismissed_for = Some(track_id.to_owned());
        s.collapse();
        return;
    }
    // The glass: the player pill's recipe, a touch denser so it reads over anything.
    let radius = rect.height() * 0.5;
    let painter = ui.painter_at(rect);
    widgets::soft_shadow(ui, rect, radius, 14.0, 0.35 * t);
    painter.rect_filled(rect, radius as u8, theme::glass(0.32));
    painter.rect_stroke(rect, radius as u8, egui::Stroke::new(1.0, theme::glass(0.14)), egui::StrokeKind::Inside);
    // The artwork, shared by both faces, growing a little with the card.
    let art_side = 32.0 + 66.0 * k;
    let art = Rect::from_min_size(pos2(rect.left() + 4.0 + 14.0 * k, rect.center().y - art_side * 0.5), Vec2::splat(art_side));
    let art_radius = if k > 0.5 { 12.0 } else { (art_side * 0.5).min(12.0) };
    widgets::paint_cover(ui, art, &track.thumbs, art_radius, 1.0);

    // The pill's face: title, then the time left, then the hair of progress.
    if k < 1.0 {
        let a = 1.0 - k;
        let ink = theme::with_alpha(theme::text(), a);
        let dim = theme::with_alpha(theme::text_dim(), a);
        let left = art.right() + 10.0;
        let right = rect.right() - 12.0;
        let tape = Rect::from_min_max(pos2(left, rect.top()), pos2((right - 52.0).max(left), rect.bottom()));
        tape_text(ui, tape, &track.title, theme::semibold(13.0), ink, now);
        let remaining = match audio.and_then(|a| a.status.duration()) {
            Some(d) => {
                let left = (d.as_secs_f32() - audio.unwrap().status.position_smooth().as_secs_f32()).max(0.0);
                super::fmt_time(left.ceil() as u32)
            }
            None => "—".to_owned(),
        };
        painter.text(pos2(rect.right() - 14.0, rect.center().y), Align2::RIGHT_CENTER, remaining, theme::regular(12.5), dim);
        // Progress: the song's own, along the pill's bottom edge.
        progress_hair(&painter, rect, audio, accent, a, PROGRESS_H);
        if response.clicked() && k < 0.5 {
            let mut s = shared.lock().unwrap();
            s.expanded = true;
            s.dwell_since = now;
        }
    }

    // The card's face: what's playing (a click on it brings the app back), the
    // scrubber, and the transport.
    if k > 0.0 {
        let a = k;
        let ink = theme::with_alpha(theme::text(), a);
        let dim = theme::with_alpha(theme::text_dim(), a);
        let left = art.right() + 16.0;
        let top = rect.top() + 16.0;
        let right = rect.right() - 18.0;
        // Title and artist, two lines the card clips (long names hold, not crawl: the
        // pill already showed them whole). Clicking them returns to the app.
        let head = Rect::from_min_max(pos2(left, top), pos2(right, top + 40.0));
        painter.text(pos2(left, top), Align2::LEFT_TOP, &track.title, theme::semibold(15.0), ink);
        painter.text(pos2(left, top + 22.0), Align2::LEFT_TOP, &track.artist, theme::regular(12.5), dim);
        let head = widgets::named(ui.interact(head, egui::Id::unique("island-head"), Sense::click()), "Back to BitChord")
            .on_hover_text("Back to BitChord")
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if head.clicked() {
            let _ = tx.send(Cmd::Restore);
        }

        // The scrubber, the player bar's: drag to seek, release to land there.
        let bar_y = rect.top() + 78.0;
        let bar = Rect::from_min_max(pos2(left, bar_y), pos2(right, bar_y + 4.0));
        painter.rect_filled(bar, 2.0, theme::with_alpha(theme::text(), 0.12 * a));
        let (p, total) = match audio {
            Some(a) => (a.status.position_smooth().as_secs_f32(), a.status.duration().map(|d| d.as_secs_f32()).unwrap_or(0.0)),
            None => (0.0, 0.0),
        };
        let frac = if total > 0.0 { (p / total).clamp(0.0, 1.0) } else { 0.0 };
        painter.rect_filled(Rect::from_min_max(bar.min, pos2(bar.left() + bar.width() * frac, bar.max.y)), 2.0, theme::with_alpha(accent, 0.9 * a));
        let at = shared.lock().unwrap().seek.unwrap_or(p);
        painter.text(pos2(left, bar_y + 17.0), Align2::LEFT_CENTER, super::fmt_time(at as u32), theme::regular(11.5), dim);
        if total > 0.0 {
            painter.text(pos2(right, bar_y + 17.0), Align2::RIGHT_CENTER, format!("-{}", super::fmt_time((total - at.min(total)).ceil() as u32)), theme::regular(11.5), dim);
        }
        let scrub = Rect::from_min_max(pos2(left, bar_y - 8.0), pos2(right, bar_y + 10.0));
        let sresp = ui.interact(scrub, egui::Id::unique("island-scrub"), Sense::click_and_drag());
        if total > 0.0 && let Some(p) = sresp.interact_pointer_pos() {
            let at = (((p.x - bar.left()) / bar.width().max(1.0)).clamp(0.0, 1.0) * total).max(0.0);
            if sresp.dragged() {
                shared.lock().unwrap().seek = Some(at);
            }
            if sresp.drag_stopped() || sresp.clicked() {
                shared.lock().unwrap().seek = None;
                let _ = tx.send(Cmd::Seek(at));
            }
        }

        // Transport: previous, play, next.
        let mid = rect.center().x;
        let row = rect.bottom() - 30.0;
        let prev_r = Rect::from_center_size(pos2(mid - 42.0, row), Vec2::splat(34.0));
        let play_r = Rect::from_center_size(pos2(mid, row), Vec2::splat(38.0));
        let next_r = Rect::from_center_size(pos2(mid + 42.0, row), Vec2::splat(34.0));
        for (r, glyph, name, cmd) in [(prev_r, super::glyphs::PREVIOUS, "Previous", Cmd::Prev), (next_r, super::glyphs::NEXT, "Next", Cmd::Next)] {
            let h = theme::anim_bool(ui.ctx(), egui::Id::unique(("island-btn", name)), ui.rect_contains_pointer(r), 0.12);
            super::glyphs::draw(ui, r.center(), 22.0, glyph, theme::with_alpha(theme::text(), a * (0.55 + 0.45 * h)));
            if ui.interact(r, egui::Id::unique(("island-hit", name)), Sense::click()).clicked() {
                let _ = tx.send(cmd);
            }
        }
        let is_playing = playing(audio);
        let h = theme::anim_bool(ui.ctx(), egui::Id::unique("island-btn-play"), ui.rect_contains_pointer(play_r), 0.12);
        painter.circle_filled(play_r.center(), 19.0, theme::with_alpha(Color32::WHITE, 0.10 * a * (0.6 + 0.4 * h)));
        let glyph = if is_playing { super::glyphs::PAUSE } else { super::glyphs::PLAY };
        super::glyphs::draw(ui, play_r.center(), 24.0, glyph, theme::with_alpha(theme::text(), a));
        if ui.interact(play_r, egui::Id::unique("island-hit-play"), Sense::click()).clicked() {
            let _ = tx.send(Cmd::Toggle);
        }
        // A click on the card (not on a control) folds it back into the pill.
        if response.clicked() && k > 0.5 {
            shared.lock().unwrap().collapse();
        }
    }
}

/// The pill's hairline of progress along its bottom edge.
fn progress_hair(painter: &egui::Painter, rect: Rect, audio: Option<&AudioHandle>, accent: Color32, alpha: f32, height: f32) {
    let (p, total) = match audio {
        Some(a) => (a.status.position_smooth().as_secs_f32(), a.status.duration().map(|d| d.as_secs_f32()).unwrap_or(0.0)),
        None => (0.0, 0.0),
    };
    let frac = if total > 0.0 { (p / total).clamp(0.0, 1.0) } else { 0.0 };
    let line = Rect::from_min_max(pos2(rect.left() + 4.0, rect.bottom() - height), pos2(rect.right() - 4.0, rect.bottom()));
    painter.rect_filled(line, height, theme::with_alpha(theme::text(), 0.10 * alpha));
    if frac > 0.0 {
        painter.rect_filled(Rect::from_min_max(line.min, pos2(line.left() + line.width() * frac, line.bottom())), height, theme::with_alpha(accent, 0.9 * alpha));
    }
}

/// The pill's title tape: text that fits sits still; text that doesn't crawls left,
/// rests, and comes back, keeping its start legible at rest.
fn tape_text(ui: &Ui, tape: Rect, text: &str, font: FontId, color: Color32, now: f64) {
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, color);
    let overflow = galley.size().x - tape.width();
    let slide = if overflow > 4.0 {
        // Ping-pong between the ends, resting at each; time is the wall clock.
        let span = 2.0 * (overflow / TAPE_SPEED + TAPE_REST);
        let phase = ((now as f32) % span) / span;
        let along = if phase < 0.5 { phase * 2.0 } else { 2.0 - phase * 2.0 };
        let travel = (along * 4.0 - 1.0).clamp(0.0, 1.0);
        let pause = |x: f32| if x < 0.12 { 0.0 } else if x > 0.88 { 1.0 } else { (x - 0.12) / 0.76 };
        pause(travel) * overflow
    } else {
        0.0
    };
    ui.painter_at(tape).galley(pos2(tape.left() - slide, tape.center().y - galley.size().y * 0.5), galley, color);
}
