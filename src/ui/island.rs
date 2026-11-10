//! The dynamic island: a floating now-playing pill pinned to the top centre of the
//! window, ported from ForkChord's island (there a separate always-on-top window;
//! here everything is drawn by the app itself, so it lives in the window as an
//! overlay, the way a phone's hangs under its notch).
//!
//! The behaviour is ForkChord's: a mini pill (artwork, title, remaining time, a hair
//! of progress) that expands into a full card (scrubber, transport) under the user's
//! own click and folds back after a few seconds' dwell; a new track never expands it,
//! and a right-click puts it away until the song changes. It vanishes while paused
//! (an open card keeps its ground so it can't be lost mid-look), and it never stands
//! over the now playing view — that page is the same thing, full size.

use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Ui, Vec2, pos2, vec2};

use super::widgets;
use super::{App, Action};
use crate::theme;

/// How long an expanded card stays out on its own (Airbeats' dwell time).
const COLLAPSE_AFTER: f64 = 5.0;
/// The pill: how tall, and how wide at least.
const PILL_H: f32 = 40.0;
const PILL_MIN_W: f32 = 216.0;
/// The expanded card's size.
const CARD_W: f32 = 400.0;
const CARD_H: f32 = 148.0;
/// The hairline of progress along the pill's bottom edge.
const PROGRESS_H: f32 = 2.0;
/// The title tape's pace, when a name doesn't fit (px/s, the island's "slowly").
const TAPE_SPEED: f32 = 24.0;
/// How long the tape rests at each end before turning around, in seconds.
const TAPE_REST: f32 = 1.6;

/// The island's state: what's expanded, since when, and what it was dismissed for.
pub struct Island {
    /// The full card is out (a click put it there; nothing else does).
    expanded: bool,
    /// App time when it expanded (and when the track last changed): the dwell runs from it.
    dwell_since: f64,
    /// The track's id a right-click dismissal belongs to: the island stays away until
    /// the song changes, never coming back on a change by itself.
    dismissed_for: Option<String>,
    /// The id of the track last seen, so a change can restart the dwell.
    track_id: Option<String>,
    /// Where a scrubber drag holds the card's position, in seconds.
    seek: Option<f32>,
}

impl Default for Island {
    fn default() -> Self {
        Self { expanded: false, dwell_since: 0.0, dismissed_for: None, track_id: None, seek: None }
    }
}

impl Island {
    /// Fold the card back in (Esc, now playing coming up, or the track ending).
    pub(super) fn collapse(&mut self) {
        self.expanded = false;
        self.seek = None;
    }
}

/// The island's overlay: nothing when it has nothing to say.
pub fn overlay(app: &mut App, ctx: &egui::Context, now: f64) {
    let playing = app.playing() || app.buffering;
    let Some(track) = app.shown() else {
        app.island.collapse();
        return;
    };
    // Owned up front: the island mutates the app below, so the song can't stay borrowed.
    let track_id = track.id.clone();
    // A new track never expands the island, but one that's out refreshes its dwell.
    if app.island.track_id.as_deref() != Some(track_id.as_str()) {
        app.island.track_id = Some(track_id.clone());
        app.island.dwell_since = now;
    }
    let np = theme::anim_bool(ctx, "np-open", app.np_open && app.current().is_some(), 0.42);
    let visible = app.island_on
        && !app.fullscreen
        && !app.mini.busy()
        && np < 0.6
        && (playing || app.island.expanded)
        && app.island.dismissed_for.as_deref() != Some(track_id.as_str());
    let t = theme::anim_bool(ctx, "island-show", visible, 0.3);
    if t <= 0.001 {
        if app.island.expanded && !visible {
            app.island.collapse();
        }
        return;
    }
    // The dwell folds the card back in on its own; under the pointer it waits.
    let screen = ctx.content_rect();
    let card = card_rect(screen);
    let hovering = ctx.pointer_hover_pos().is_some_and(|p| card.contains(p));
    if app.island.expanded && now - app.island.dwell_since > COLLAPSE_AFTER && !hovering {
        app.island.collapse();
    }
    // Esc folds an open card back in (the shortcuts' Esc closes now playing, which is
    // already closed here, so the two never fight over the key).
    if app.island.expanded && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.island.collapse();
    }

    // The island's surface, top centre, fading and dropping in with the show.
    let k = theme::anim_bool(ctx, "island-expand", app.island.expanded, 0.32);
    let rect = pill_rect(screen).lerp_inside(card, k);
    egui::Area::new(egui::Id::unique("island"))
        .order(egui::Order::Foreground)
        .fixed_pos(rect.min)
        .show(ctx, |ui| {
            ui.set_clip_rect(rect);
            ui.set_opacity(t);
            paint_island(app, ui, rect, k, t, now);
        });
}

/// The pill's rect for a screen, top centre under the title row.
fn pill_rect(screen: Rect) -> Rect {
    let x = screen.center().x - PILL_MIN_W * 0.5;
    let y = screen.top() + 8.0 + super::titlebar::top_margin(8, PILL_H) as f32;
    Rect::from_min_size(pos2(x, y), vec2(PILL_MIN_W, PILL_H))
}

/// The expanded card's rect for a screen, sharing the pill's top centre.
fn card_rect(screen: Rect) -> Rect {
    let x = screen.center().x - CARD_W * 0.5;
    let y = screen.top() + 8.0 + super::titlebar::top_margin(8, PILL_H) as f32;
    Rect::from_min_size(pos2(x, y), vec2(CARD_W, CARD_H))
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

/// The island itself: the pill's face for `1 - k` of the frame, the card's for `k`.
fn paint_island(app: &mut App, ui: &mut Ui, rect: Rect, k: f32, t: f32, now: f64) {
    // The track's bits, owned: the island mutates the app while it draws (expand,
    // dismiss, seek), so nothing of the song stays borrowed.
    let Some(track) = app.shown() else { return };
    let (title, artist, thumbs, track_id) = (track.title.clone(), track.artist_line(), track.thumbs.clone(), track.id.clone());
    let response = ui.interact(rect, egui::Id::unique("island-hit"), Sense::click());
    if response.secondary_clicked() {
        // Right-click puts it away for this stretch of playback; it comes back when
        // the song changes, never on the change itself.
        app.island.dismissed_for = Some(track_id);
        app.island.collapse();
        return;
    }
    // The glass: the player pill's recipe, a touch denser so it reads over any page.
    let radius = rect.height() * 0.5;
    let accent = app.accent_color();
    let painter = ui.painter_at(rect);
    widgets::soft_shadow(ui, rect, radius, 14.0, 0.35 * t);
    painter.rect_filled(rect, radius as u8, theme::glass(0.32));
    painter.rect_stroke(rect, radius as u8, egui::Stroke::new(1.0, theme::glass(0.14)), egui::StrokeKind::Inside);
    // The artwork, shared by both faces, growing a little with the card.
    let art_side = 32.0 + 66.0 * k;
    let art = Rect::from_min_size(pos2(rect.left() + 4.0 + 14.0 * k, rect.center().y - art_side * 0.5), Vec2::splat(art_side));
    let art_radius = if k > 0.5 { 12.0 } else { (art_side * 0.5).min(12.0) };
    widgets::paint_cover(ui, art, &thumbs, art_radius, 1.0);

    // The pill's face: title, then the time left, then the hair of progress.
    if k < 1.0 {
        let a = 1.0 - k;
        let ink = theme::with_alpha(theme::text(), a);
        let dim = theme::with_alpha(theme::text_dim(), a);
        let left = art.right() + 10.0;
        let right = rect.right() - 12.0;
        let tape = Rect::from_min_max(pos2(left, rect.top()), pos2((right - 52.0).max(left), rect.bottom()));
        tape_text(ui, tape, &title, theme::semibold(13.0), ink, now);
        let remaining = match app.audio.status.duration() {
            Some(d) => {
                let left = (d.as_secs_f32() - app.audio.status.position_smooth().as_secs_f32()).max(0.0);
                super::fmt_time(left.ceil() as u32)
            }
            None => "—".to_owned(),
        };
        painter.text(pos2(rect.right() - 14.0, rect.center().y), Align2::RIGHT_CENTER, remaining, theme::regular(12.5), dim);
        // Progress: the song's own, along the pill's bottom edge.
        progress_hair(&painter, rect, app, accent, a, PROGRESS_H);
        if response.clicked() && k < 0.5 {
            expand(app, now);
        }
    }

    // The card's face: what's playing, the scrubber, and the transport.
    if k > 0.0 {
        let a = k;
        let ink = theme::with_alpha(theme::text(), a);
        let dim = theme::with_alpha(theme::text_dim(), a);
        let left = art.right() + 16.0;
        let top = rect.top() + 16.0;
        let right = rect.right() - 18.0;
        // Title and artist, two lines the card clips (long names hold, not crawl: the
        // pill already showed them whole).
        painter.text(pos2(left, top), Align2::LEFT_TOP, &title, theme::semibold(15.0), ink);
        painter.text(pos2(left, top + 22.0), Align2::LEFT_TOP, artist, theme::regular(12.5), dim);

        // The scrubber, the player bar's: drag to seek, release to land there.
        let bar_y = rect.top() + 78.0;
        let bar = Rect::from_min_max(pos2(left, bar_y), pos2(right, bar_y + 4.0));
        painter.rect_filled(bar, 2.0, theme::with_alpha(theme::text(), 0.12 * a));
        let (p, total) = (app.audio.status.position_smooth().as_secs_f32(), app.audio.status.duration().map(|d| d.as_secs_f32()).unwrap_or(0.0));
        let frac = if total > 0.0 { (p / total).clamp(0.0, 1.0) } else { 0.0 };
        painter.rect_filled(Rect::from_min_max(bar.min, pos2(bar.left() + bar.width() * frac, bar.max.y)), 2.0, theme::with_alpha(accent, 0.9 * a));
        let at = app.island.seek.unwrap_or(p);
        painter.text(pos2(left, bar_y + 17.0), Align2::LEFT_CENTER, super::fmt_time(at as u32), theme::regular(11.5), dim);
        if total > 0.0 {
            painter.text(pos2(right, bar_y + 17.0), Align2::RIGHT_CENTER, format!("-{}", super::fmt_time((total - at.min(total)).ceil() as u32)), theme::regular(11.5), dim);
        }
        let scrub = Rect::from_min_max(pos2(left, bar_y - 8.0), pos2(right, bar_y + 10.0));
        let sresp = ui.interact(scrub, egui::Id::unique("island-scrub"), Sense::click_and_drag());
        if total > 0.0 && let Some(p) = sresp.interact_pointer_pos() {
            let at = (((p.x - bar.left()) / bar.width().max(1.0)).clamp(0.0, 1.0) * total).max(0.0);
            if sresp.dragged() {
                app.island.seek = Some(at);
            }
            if sresp.drag_stopped() || sresp.clicked() {
                app.island.seek = None;
                app.actions.push(Action::Seek(at));
            }
        }

        // Transport: previous, play (with the bar's buffer spinner), next.
        let mid = rect.center().x;
        let row = rect.bottom() - 30.0;
        let prev_r = Rect::from_center_size(pos2(mid - 42.0, row), Vec2::splat(34.0));
        let play_r = Rect::from_center_size(pos2(mid, row), Vec2::splat(38.0));
        let next_r = Rect::from_center_size(pos2(mid + 42.0, row), Vec2::splat(34.0));
        for (r, glyph, name) in [(prev_r, super::glyphs::PREVIOUS, "Previous"), (next_r, super::glyphs::NEXT, "Next")] {
            let h = theme::anim_bool(ui.ctx(), egui::Id::unique(("island-btn", name)), ui.rect_contains_pointer(r), 0.12);
            super::glyphs::draw(ui, r.center(), 22.0, glyph, theme::with_alpha(theme::text(), a * (0.55 + 0.45 * h)));
            if ui.interact(r, egui::Id::unique(("island-hit", name)), Sense::click()).clicked() {
                if name == "Previous" {
                    app.prev();
                } else {
                    app.next();
                }
            }
        }
        let h = theme::anim_bool(ui.ctx(), egui::Id::unique("island-btn-play"), ui.rect_contains_pointer(play_r), 0.12);
        painter.circle_filled(play_r.center(), 19.0, theme::with_alpha(Color32::WHITE, 0.10 * a * (0.6 + 0.4 * h)));
        if app.buffering {
            let spin = now as f32 * 6.0;
            let pts: Vec<Pos2> = (0..16).map(|i| play_r.center() + vec2((spin + i as f32 * 0.24).cos(), (spin + i as f32 * 0.24).sin()) * 8.0).collect();
            painter.add(egui::Shape::Path(egui::epaint::PathShape::line(pts, egui::epaint::PathStroke::new(2.0, theme::with_alpha(theme::text(), 0.6 * a)))));
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
        } else {
            let glyph = if app.playing() { super::glyphs::PAUSE } else { super::glyphs::PLAY };
            super::glyphs::draw(ui, play_r.center(), 24.0, glyph, theme::with_alpha(theme::text(), a));
        }
        if ui.interact(play_r, egui::Id::unique("island-hit-play"), Sense::click()).clicked() {
            app.toggle();
        }
        // A click on the card (not on a control) holds the card out a while longer.
        if response.clicked() && k > 0.5 {
            app.island.dwell_since = now;
        }
    }
}

/// Mark the card out and start its dwell.
fn expand(app: &mut App, now: f64) {
    app.island.expanded = true;
    app.island.dwell_since = now;
}

/// The pill's hairline of progress along its bottom edge.
fn progress_hair(painter: &egui::Painter, rect: Rect, app: &App, accent: Color32, alpha: f32, height: f32) {
    let (p, total) = (app.audio.status.position_smooth().as_secs_f32(), app.audio.status.duration().map(|d| d.as_secs_f32()).unwrap_or(0.0));
    let frac = if total > 0.0 { (p / total).clamp(0.0, 1.0) } else { 0.0 };
    let line = Rect::from_min_max(pos2(rect.left() + 4.0, rect.bottom() - height), pos2(rect.right() - 4.0, rect.bottom()));
    painter.rect_filled(line, height, theme::with_alpha(theme::text(), 0.10 * alpha));
    if frac > 0.0 {
        painter.rect_filled(Rect::from_min_max(line.min, pos2(line.left() + line.width() * frac, line.bottom())), height, theme::with_alpha(accent, 0.9 * alpha));
    }
}

/// The pill's title tape (ForkChord's, to egui): text that fits sits still; text that
/// doesn't crawls left, rests, and comes back, keeping its start legible at rest.
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
