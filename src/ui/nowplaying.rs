//! Now-playing overlay: big art + synced lyrics / up next, over the full-strength
//! animated background. The lyrics themselves are drawn by `karaoke` (a port of the
//! better-lyrics renderer); this adds their toolbar and the plain-text fallback.

use egui::{Color32, Pos2, Rect, Sense, Ui, Vec2, pos2, vec2};

use super::widgets::{self, named, text_at};
use super::{Action, App, NpTab, Repeat, glyphs, karaoke};
use crate::lyrics::{Lyrics, Sync, SyncKind};
use crate::shader::Params;
use crate::theme::{self, icon};

/// A video's glow: its colours now, where they're easing to, and its picture.
pub(super) struct Glow {
    current: Vec<[f32; 3]>,
    target: Vec<[f32; 3]>,
    tex: egui::TextureHandle,
}

pub fn overlay(app: &mut App, ctx: &egui::Context, now: f64) {
    use crate::preset::BgMode;
    let t = theme::anim_bool_timed(ctx, "np-open", app.np_open && app.current().is_some(), 0.42);
    if t <= 0.001 {
        // Closed: nothing to decode for. It picks up from the music's place when opened.
        if app.video.take().is_some() {
            app.video_frame = None;
            app.glow = None;
        }
        return;
    }
    let screen = ctx.content_rect();
    // The player brings its own transport, so it takes the whole window: the capsule
    // under it belongs to the pages it covers.
    let full = screen;
    let rect = full.translate(vec2(0.0, full.height() * (1.0 - t)));
    let look = app.look();
    let b = &look.background;
    let video_only = app.fullscreen && app.video_full;
    let video_only = video_only.then(|| app.shown_video().cloned()).flatten();
    egui::Area::new(egui::Id::unique("now-playing"))
        .order(egui::Order::Middle)
        .fixed_pos(rect.min)
        .show(ctx, |ui| {
            // Swallow input so the page underneath doesn't react.
            ui.allocate_rect(rect, Sense::click_and_drag());
            if let Some(video) = &video_only {
                video_fullscreen(app, ui, rect, video, now);
                return;
            }
            // It covers the window's top too: that still moves the window.
            if !app.fullscreen {
                super::titlebar::drag_area(ui, Rect::from_min_size(rect.min, vec2(rect.width(), super::titlebar::HEIGHT)), "now-playing");
            }
            let painter = ui.painter_at(rect);
            let mode = app.np_bg.unwrap_or(b.mode);
            let art = matches!(mode, BgMode::Kawarp | BgMode::CoverStatic);
            if mode == BgMode::CoverFullbleed {
                // The cover itself fills the screen behind a scrim. With the lyrics-only
                // layout on a wide window it fills the height on the left instead and
                // fades out toward the lyrics (reCover).
                let solid = look.solid();
                painter.rect_filled(rect, 0.0, solid);
                if let Some(track) = app.shown() {
                    let side_by_side = look.cover.layout == crate::preset::Layout::LyricsOnly && rect.width() >= 900.0;
                    let area = if side_by_side { Rect::from_min_size(rect.min, Vec2::splat(rect.height())) } else { rect };
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(area));
                    child.set_clip_rect(area);
                    widgets::paint_cover_fill(&child, area, &track.thumbs, Color32::from_gray((b.np_dim.clamp(0.0, 1.0) * 255.0) as u8));
                    if side_by_side {
                        let fade = Rect::from_min_max(pos2(area.center().x - area.width() * 0.1, area.top()), area.max);
                        super::shell::hgradient(&painter, fade, Color32::TRANSPARENT, solid);
                    }
                }
            } else if let Some(bg) = app.bg.as_ref().filter(|_| art && !app.disable_blur) {
                let blend = theme::ease_in_out(((now - app.art_changed) / 1.4) as f32);
                let level = if app.reactive_bg && app.playing() { app.audio.status.level() * b.reactive } else { 0.0 };
                bg.paint(
                    &painter,
                    rect,
                    Params {
                        time: app.bg_time as f32,
                        blend,
                        intensity: if mode == BgMode::CoverStatic { 0.0 } else { b.intensity },
                        saturation: b.saturation,
                        dim: b.np_dim,
                        scale: b.scale + level.min(0.5) * 0.12,
                    },
                );
            } else if mode == BgMode::Solid {
                painter.rect_filled(rect, 0.0, look.solid());
            } else {
                painter.rect_filled(rect, 0.0, theme::lerp_color(theme::bg(), app.accent_color(), 0.25));
            }
            let k = b.scrim / 0.3;
            super::shell::vgradient(ui.painter(), rect, theme::shade((0.18 * k).min(0.95)), theme::shade((0.42 * k).min(0.95)));
            if app.ambient_light && app.shown_video().is_some() {
                paint_glow(app, ui, rect);
            }
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(32.0, 20.0))));
            body(app, &mut child, now);
        });
}

/// The chevron that closes now playing, drawn from two strokes with a round joint —
/// the way the window buttons draw theirs, with no font involved.
fn chevron_down(ui: &Ui, center: egui::Pos2, half: f32, color: Color32) {
    let stroke = egui::Stroke::new(2.0, color);
    let left = center + vec2(-half, -half * 0.45);
    let point = center + vec2(0.0, half * 0.55);
    let right = center + vec2(half, -half * 0.45);
    ui.painter().line_segment([left, point], stroke);
    ui.painter().line_segment([point, right], stroke);
    ui.painter().circle_filled(point, stroke.width / 2.0, color);
}

fn body(app: &mut App, ui: &mut Ui, now: f64) {
    use crate::preset::{Layout, Side};
    // An Automix handoff keeps the outgoing song here until the next one is the louder
    // of the two (see `MixFrom`): its track, seek line and lyrics lag the queue's switch.
    let hold = app.mix_hold().map(|h| (h.total, h.mix, app.mix_hold_pos(h)));
    let Some(track) = app.current().cloned() else { return };
    let track = match &hold {
        Some(_) => app.shown().cloned().unwrap_or(track),
        None => track,
    };
    let look = app.look();
    let cs = &look.cover;
    let area = ui.max_rect();
    let video = app.shown_video().cloned();
    let video_mode = video.is_some();
    let no_lyrics = app.no_lyrics() || video_mode;
    // The content is this area less its margins: a cover column beside the panel needs
    // 860 points, and below that the cover gives way to the panel.
    let narrow = area.width() - 64.0 < 860.0;
    // Header: on the window buttons' line, when the app draws them.
    let lift = 20.0 - f32::from(super::titlebar::top_margin(20, 44.0));
    let header = Rect::from_min_size(area.min - vec2(0.0, if app.fullscreen { 0.0 } else { lift }), vec2(area.width(), 44.0));
    // A song gets the switch too once its music video is found (or is playing). Songs, not
    // music videos, with videos off: no switch on a song, and no music video looked up for
    // it. A music video keeps its own: it's the only way from its song back to the video.
    let songs_only = app.play_songs && !app.show_video;
    if !app.jam.active() && (track.has_video() || (!songs_only && (app.playing_as().is_some() || app.song_video().is_some()))) {
        song_video_switch(app, ui, header);
    }
    let mut hui = ui.new_child(egui::UiBuilder::new().max_rect(header).layout(egui::Layout::left_to_right(egui::Align::Center)));
    // macOS: past the traffic lights, which stay over now playing.
    let gap = super::titlebar::left_gap(hui.ctx(), hui.max_rect(), 0.0);
    if gap > 0.0 {
        hui.add_space(gap);
    }
    // The player's own close, top left; the window buttons keep the top right. The
    // chevron is drawn with strokes rather than read from the icon font: a glyph the
    // atlas hasn't got paints nothing at all, which used to leave this button
    // clickable but without a visible icon.
    let (close_rect, close_resp) = hui.allocate_exact_size(Vec2::splat(38.0), Sense::click());
    let close_resp = widgets::named(close_resp, "Close now playing").on_hover_text("Close now playing").on_hover_cursor(egui::CursorIcon::PointingHand);
    let hover = theme::anim_bool(ui.ctx(), close_resp.id.with("h"), close_resp.hovered(), 0.15);
    if hover > 0.0 {
        ui.painter().circle_filled(close_rect.center(), 19.0, theme::glass(0.10 * hover));
    }
    let press = if close_resp.is_pointer_button_down_on() { 0.9 } else { 1.0 };
    chevron_down(ui, close_rect.center(), 11.0 * press, theme::lerp_color(theme::text_dim(), theme::text(), hover));
    if close_resp.clicked() {
        app.np_open = false;
        if app.fullscreen {
            app.set_fullscreen(ui.ctx(), false);
        }
    }
    hui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.add_space(super::titlebar::right_gap(ui.ctx(), ui.max_rect(), 0.0));
        // Its actions (like, download, share…) are for what's heard: a song version
        // standing in for the music video. It lives beside the star under the cover;
        // only when the cover column is hidden (a narrow window showing lyrics) does
        // it stay up here.
        let cover_hidden = (narrow || cs.layout == Layout::LyricsOnly) && app.np_tab != NpTab::Art && !no_lyrics;
        if cover_hidden {
            let (r, _) = ui.allocate_exact_size(vec2(34.0, 34.0), Sense::hover());
            let heard = heard_track(app, &track, hold.is_some());
            options_menu(app, ui, r, &heard, app.np_tab == NpTab::Lyrics && !no_lyrics);
        }
    });

    let content = Rect::from_min_max(pos2(area.left(), header.bottom() + 12.0), area.max);
    if let Some(video) = video.as_ref() {
        video_stage(app, ui, content, video);
        return;
    }
    if app.video.take().is_some() {
        app.video_frame = None;
        app.glow = None;
    }
    // The sleeve's star and its menus go by what's heard: one clone a frame, shared
    // with the cover column rather than taken again inside it.
    let heard = heard_track(app, &track, hold.is_some());
    // What the margins came off: the whole player. The queue panel runs to its edge so
    // its scroll bar hugs the window's edge.
    let full = area.expand2(vec2(32.0, 20.0));
    // The side panel (lyrics / up next) slides in and out when its chip is toggled.
    // A song without lyrics shows just the artwork, with no transition.
    let last_id = egui::Id::unique("np-last-panel");
    let panel_tab = if app.np_tab == NpTab::Art {
        ui.ctx().data(|d| d.get_temp(last_id)).unwrap_or(NpTab::Lyrics)
    } else {
        ui.ctx().data_mut(|d| d.insert_temp(last_id, app.np_tab));
        app.np_tab
    };
    // Linear 0..1 over the transition; each part eases its own slice of it.
    let shown = ui.ctx().animate_bool_with_time(egui::Id::unique("np-panel"), app.np_tab != NpTab::Art, 0.6 * theme::speed());
    if shown > 0.0 && shown < 1.0 {
        // Every frame while sliding, not the now-playing view's usual 30 fps.
        ui.ctx().request_repaint();
    }
    let ppp = ui.ctx().pixels_per_point();
    let p = if panel_tab == NpTab::Lyrics && no_lyrics { 0.0 } else { shown };
    // Whichever is leaving slides and fades out over the first 45%, then the other
    // comes in over the last 70%, both easing out, in either direction.
    let (panel_vis, art_vis) = if app.np_tab != NpTab::Art {
        (theme::ease_out((p - 0.3) / 0.7), 1.0 - theme::ease_out(p / 0.45))
    } else {
        let q = 1.0 - p;
        (1.0 - theme::ease_out(q / 0.45), theme::ease_out((q - 0.3) / 0.7))
    };
    // The cover gliding between its two places (wide layouts): a gentle S-curve.
    let glide = p * p * (3.0 - 2.0 * p);
    let lyrics_only = narrow || cs.layout == Layout::LyricsOnly;
    // reCover: the backdrop cover sits on the left, so the lyrics take the right side.
    // The backdrop override counts: the layout follows the mode actually showing.
    let beside_cover = !narrow && cs.layout == Layout::LyricsOnly
        && app.np_bg.unwrap_or(look.background.mode) == crate::preset::BgMode::CoverFullbleed;
    let centered = lyrics_only && !beside_cover;
    // Artwork alone: centered.
    let col_art = Rect::from_center_size(content.center(), vec2(content.width().min(content.height() * 0.9), content.height()));
    let slide = 72.0;
    let right = if lyrics_only {
        // No cover column next to the panel: the artwork fades in from the left as the panel leaves.
        let px = (art_side(cs, col_art) * ppp) as u32;
        if art_vis > 0.001 {
            let mut cui = ui.new_child(egui::UiBuilder::new().max_rect(content));
            cui.multiply_opacity(art_vis);
            cover_column(app, &mut cui, col_art.translate(vec2(-slide * (1.0 - art_vis), 0.0)), &track, &heard, no_lyrics, now, px);
        } else {
            // Hidden behind the lyrics: have it loaded for when they're turned off.
            widgets::preload_cover(ui.ctx(), &track.thumbs, px);
        }
        if beside_cover { Rect::from_min_max(pos2(content.left() + content.width() * 0.46, content.top()), content.max) } else { content }
    } else {
        // Cover column, on the preset's side; it glides to the center as the panel leaves.
        let frac = if cs.layout == Layout::CoverHero { 0.5 } else { 0.42 };
        let col_w = content.width() * frac;
        let col_panel = if cs.side == Side::Right {
            Rect::from_min_size(pos2(content.right() - col_w, content.top()), vec2(col_w, content.height()))
        } else {
            Rect::from_min_size(content.min, vec2(col_w, content.height()))
        };
        let col = Rect::from_min_max(col_art.min.lerp(col_panel.min, glide), col_art.max.lerp(col_panel.max, glide));
        // One image size for the whole glide, the bigger of its two ends.
        let px = (art_side(cs, col_art).max(art_side(cs, col_panel)) * ppp) as u32;
        cover_column(app, ui, col, &track, &heard, no_lyrics, now, px);
        if cs.side == Side::Right {
            Rect::from_min_max(content.min, pos2(col_panel.left() - 28.0, content.bottom()))
        } else {
            Rect::from_min_max(pos2(col_panel.right() + 28.0, content.top()), content.max)
        }
    };
    if panel_vis > 0.001 {
        let toward = if !lyrics_only && cs.side == Side::Right { -slide } else { slide };
        let rect = right.translate(vec2(toward * (1.0 - panel_vis), 0.0));
        // Switching between Lyrics and Up next: the old one fades out over the first 55%,
        // the new one fades in and rises into place from a third of the way.
        for (tab, key) in [(NpTab::Lyrics, "np-tab-lyrics"), (NpTab::UpNext, "np-tab-upnext"), (NpTab::Comments, "np-tab-comments")] {
            let w = ui.ctx().animate_bool_with_time(egui::Id::unique(key), panel_tab == tab, 0.45 * theme::speed());
            if w > 0.0 && w < 1.0 {
                ui.ctx().request_repaint();
            }
            let vis = if panel_tab == tab {
                theme::ease_out((w - 0.35) / 0.65)
            } else {
                let x = ((w - 0.45) / 0.55).clamp(0.0, 1.0);
                x * x * (3.0 - 2.0 * x)
            };
            if vis <= 0.001 || (tab == NpTab::Lyrics && no_lyrics) {
                continue;
            }
            let rise = if panel_tab == tab { 18.0 * (1.0 - vis) } else { 0.0 };
            let mut r = rect.translate(vec2(0.0, rise));
            if tab == NpTab::UpNext && r.right() >= content.right() - 0.5 {
                // The queue's scroll bar hugs the window's edge (less the resize border,
                // like the drawer's): its panel runs out to there, the rows inset inside.
                r.max.x = full.right() - super::titlebar::edge_inset();
            }
            let mut rui = ui.new_child(egui::UiBuilder::new().max_rect(r));
            rui.set_clip_rect(r.intersect(ui.clip_rect()));
            rui.multiply_opacity(panel_vis * vis);
            match tab {
                NpTab::Lyrics => lyrics_panel(app, &mut rui, r, now, centered),
                NpTab::Comments => super::comments::panel(app, &mut rui),
                _ => super::shell::queue_list(app, &mut rui, "np", 14.0),
            }
        }
    }

    // Corner toggles, both bottom-right: up next in the corner, lyrics beside it. Each
    // lights up while its panel is the one showing; clicking the lit one returns to the
    // artwork alone — the same trip the ⋯ menu's rows and the L/Q keys make.
    let accent = app.accent_color();
    let ink = |h: f32| theme::lerp_color(theme::text_dim(), theme::text(), h);
    let corner = |ui: &mut Ui, center: Pos2, key: &str, name: &str| -> egui::Response {
        let r = Rect::from_center_size(center, Vec2::splat(34.0));
        let resp = ui.interact(r, egui::Id::unique(("np-corner", key)), Sense::click());
        named(resp, name).on_hover_text(name).on_hover_cursor(egui::CursorIcon::PointingHand)
    };
    let r = corner(ui, pos2(area.right() - 64.0, area.bottom() - 20.0), "lyrics", "Lyrics");
    let h = theme::anim_bool(ui.ctx(), r.id.with("h"), r.hovered(), 0.15);
    let on = app.np_tab == NpTab::Lyrics && !no_lyrics;
    ui.painter().circle_filled(r.rect.center(), 17.0, theme::glass(0.09 + 0.07 * h));
    glyphs::draw(ui, r.rect.center(), 18.0, if on { glyphs::LYRICS } else { glyphs::LYRICS_OFF }, if on { accent } else { ink(h) });
    if r.clicked() {
        app.np_tab = if on { NpTab::Art } else { NpTab::Lyrics };
    }
    let r = corner(ui, pos2(area.right() - 20.0, area.bottom() - 20.0), "queue", "Up next");
    let h = theme::anim_bool(ui.ctx(), r.id.with("h"), r.hovered(), 0.15);
    let on = app.np_tab == NpTab::UpNext;
    ui.painter().circle_filled(r.rect.center(), 17.0, theme::glass(0.09 + 0.07 * h));
    glyphs::draw(ui, r.rect.center(), 18.0, glyphs::QUEUE, if on { accent } else { ink(h) });
    if r.clicked() {
        app.np_tab = if on { NpTab::Art } else { NpTab::UpNext };
    }

    edge_volume(app, ui, full, content);
}

/// The volume at Now playing's left edge, in whichever shape Settings › Appearance
/// asks for: the bar the view grew up with — the default — or the half disc on the
/// edge that came for it later.
fn edge_volume(app: &mut App, ui: &mut Ui, full: Rect, content: Rect) {
    if app.np_vol_dial { volume_dial(app, ui, full, content) } else { volume_bar(app, ui, full, content) }
}

/// The volume along the view's left edge: the margin stays empty until the pointer
/// comes into it, then a bar grows in — dragged or clicked like any slider, its foot
/// a mute, the wheel a nudge. The view keeps its breathing room; the edge only has
/// to be asked.
fn volume_bar(app: &mut App, ui: &mut Ui, full: Rect, content: Rect) {
    let open_id = egui::Id::unique("np-vol-open");
    let was = ui.ctx().data(|d| d.get_temp::<bool>(open_id)).unwrap_or(false);
    // The window's resize border owns the outermost sliver in a layer above this
    // one, so the hover is read from the pointer's position rather than egui's
    // `rect_contains_pointer`, which would report the border's verdict instead.
    let hover = |r: Rect| ui.input(|i| i.pointer.hover_pos()).is_some_and(|p| r.contains(p));
    // The strip that wakes the bar starts past the resize border; once awake, the
    // bar's own ground holds it — and any press, so a drag past the track doesn't
    // leave the knob behind a closing bar.
    let strip = Rect::from_min_max(pos2(full.left() + 5.0, content.top()), pos2(full.left() + 38.0, content.bottom()));
    let h = (content.height() * 0.55).clamp(150.0, 320.0);
    let track = Rect::from_center_size(pos2(full.left() + 16.0, content.center().y), vec2(6.0, h));
    let ground = Rect::from_min_max(pos2(full.left(), track.top() - 6.0), pos2(full.left() + 34.0, track.bottom() + 42.0));
    let open = hover(strip) || (was && (hover(ground) || ui.input(|i| i.pointer.any_down())));
    ui.ctx().data_mut(|d| d.insert_temp(open_id, open));
    let reveal = theme::anim_bool(ui.ctx(), "np-vol-reveal", open, 0.15);
    if reveal <= 0.001 {
        return;
    }
    let vol = app.audio.volume();
    let resp = ui
        .interact(track.expand2(vec2(5.0, 0.0)), egui::Id::unique("np-vol"), Sense::click_and_drag())
        .on_hover_cursor(egui::CursorIcon::ResizeVertical);
    let vh = theme::anim_bool(ui.ctx(), resp.id.with("h"), resp.hovered() || resp.dragged(), 0.15);
    resp.widget_info(|| egui::WidgetInfo::slider(true, vol as f64, "Volume"));
    if let Some(p) = resp.interact_pointer_pos() && (resp.dragged() || resp.clicked()) {
        app.audio.set_volume(((track.bottom() - p.y) / track.height()).clamp(0.0, 1.0));
    }
    if resp.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            app.audio.set_volume((vol + scroll / 600.0).clamp(0.0, 1.0));
        }
    }
    // The pill the bar sits in, glass over whatever cover the background carries.
    let pill = Rect::from_min_max(pos2(track.left() - 8.0, track.top() - 8.0), pos2(track.right() + 8.0, track.bottom() + 40.0));
    ui.painter().rect_filled(pill, pill.width() / 2.0, theme::shade(0.30 * reveal));
    let tw = 3.0 + 1.5 * vh;
    let tr = Rect::from_center_size(track.center(), vec2(tw, track.height()));
    ui.painter().rect_filled(tr, tw / 2.0, theme::with_alpha(Color32::WHITE, 0.22 * reveal));
    let now = app.audio.volume();
    let fill = Rect::from_min_size(pos2(tr.left(), tr.bottom() - tr.height() * now), vec2(tw, tr.height() * now));
    let (bar, knob) = if app.accent_controls { let a = app.accent_color(); (a, a) } else { (Color32::WHITE, Color32::WHITE) };
    ui.painter().rect_filled(fill, tw / 2.0, theme::with_alpha(bar, reveal));
    let knob_at = pos2(tr.center().x, tr.bottom() - tr.height() * now);
    ui.painter().circle_filled(knob_at, 4.5 + 1.5 * vh, theme::with_alpha(knob, reveal));
    if vh > 0.0 {
        // How loud it stands, said beside the knob only while the bar is being held.
        let at = pos2(pill.right() + 6.0, knob_at.y);
        ui.painter().text(at, egui::Align2::LEFT_CENTER, format!("{}%", (now * 100.0).round()), theme::semibold(11.5), theme::with_alpha(theme::text(), vh * reveal));
    }
    // The mute at the pill's foot, the player bar's own trip.
    let m = Rect::from_center_size(pos2(pill.center().x, pill.bottom() - 15.0), Vec2::splat(26.0));
    let mresp = ui.interact(m, egui::Id::unique("np-vol-mute"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    let mh = theme::anim_bool(ui.ctx(), mresp.id.with("h"), mresp.hovered(), 0.15);
    if mh > 0.0 {
        ui.painter().circle_filled(m.center(), 15.0, theme::glass(0.09 * mh));
    }
    glyphs::draw(ui, m.center(), 15.0, glyphs::VOLUME[glyphs::volume_step(now)], theme::lerp_color(theme::text_dim(), theme::text(), mh.max(reveal)));
    if mresp.clicked() {
        super::shell::toggle_mute(app, ui.ctx());
    }
}

/// The volume at the screen's left edge: a half disc pressed flat against it, its rim
/// running the whole window edge with no spacing. The level is a neon arc round the
/// rim — the foot silence, the crown full — the number at its heart, the speaker a
/// mute. Dragged up and down along the rim like the arc reads it, the wheel a nudge.
/// Only its own ground wakes it: the edge stays empty until the pointer comes onto
/// the half disc, then the dial grows in.
fn volume_dial(app: &mut App, ui: &mut Ui, full: Rect, content: Rect) {
    let open_id = egui::Id::unique("np-vol-open");
    let was = ui.ctx().data(|d| d.get_temp::<bool>(open_id)).unwrap_or(false);
    // The window's resize border owns the outermost sliver in a layer above this
    // one, so the hover is read from the pointer's position rather than egui's
    // `rect_contains_pointer`, which would report the border's verdict instead.
    let hover_pos = ui.input(|i| i.pointer.hover_pos());
    let r = 74.0;
    // The flat side sits on the window's left edge, centred on the view: no spacing
    // anywhere, the dial hanging off the screen's border like the reference's.
    let at = pos2(full.left(), content.center().y);
    let dial = Rect::from_min_size(pos2(full.left(), at.y - r), vec2(r, r * 2.0));
    // Its own ground, and nothing else: on the half disc (the corners of its rect
    // are air), a touch past the rim so the edge is easy to catch.
    let over = |p: Pos2, pad: f32| dial.expand(pad).contains(p) && p.distance(at) <= r + pad;
    // Once awake, the dial's own ground holds it — and any press, so a drag off the
    // dial doesn't drop it mid-turn.
    let open = hover_pos.is_some_and(|p| over(p, 12.0)) || (was && (hover_pos.is_some_and(|p| over(p, 10.0)) || ui.input(|i| i.pointer.any_down())));
    ui.ctx().data_mut(|d| d.insert_temp(open_id, open));
    let reveal = theme::anim_bool(ui.ctx(), "np-vol-reveal", open, 0.15);
    if reveal <= 0.001 {
        return;
    }
    let vol = app.audio.volume();
    let resp = ui
        .interact(dial, egui::Id::unique("np-vol"), Sense::click_and_drag())
        .on_hover_cursor(egui::CursorIcon::ResizeVertical);
    let vh = theme::anim_bool(ui.ctx(), resp.id.with("h"), resp.hovered() || resp.dragged(), 0.15);
    resp.widget_info(|| egui::WidgetInfo::slider(true, vol as f64, "Volume"));
    // Round the rim the way the arc reads it: the foot silence, the crown full, a
    // drag up the half disc louder and down quieter.
    if let Some(p) = resp.interact_pointer_pos()
        && (resp.dragged() || resp.clicked())
    {
        let a = (p.y - at.y).atan2(p.x - at.x);
        app.audio.set_volume(((std::f32::consts::FRAC_PI_2 - a) / std::f32::consts::PI).clamp(0.0, 1.0));
    }
    if resp.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            app.audio.set_volume((vol + scroll / 600.0).clamp(0.0, 1.0));
        }
    }
    // The disc: darker than the old pill, so the gauge reads over whatever the
    // background carries, with a rim to lift it off the art. The window clips its
    // half past the edge — the flat side is the screen's own border.
    let outer = ui.clip_rect();
    ui.set_clip_rect(Rect::from_min_max(pos2(outer.left().max(full.left()), (outer.top()).max(full.top())), pos2(outer.right(), outer.bottom())));
    ui.painter().circle_filled(at, r, theme::shade(0.60 * reveal));
    ui.painter().circle_stroke(at, r, egui::Stroke::new(1.0, theme::glass(0.10 * reveal)));
    // The level as a neon arc: from the dial's foot, round the rim and up to where
    // the volume puts it — a notch stays at the foot, as in the reference, so full
    // never closes. egui can't blur; layered strokes stand in for the glow, lit a
    // touch stronger while it has the pointer.
    let accent = if app.accent_controls { app.accent_color() } else { Color32::WHITE };
    let len = std::f32::consts::PI * vol.clamp(0.0, 1.0);
    let steps = 48;
    let arc_r = r - 9.0;
    let pts: Vec<Pos2> = (0..=steps)
        .map(|i| {
            let a = std::f32::consts::FRAC_PI_2 - len * i as f32 / steps as f32;
            at + vec2(a.cos(), a.sin()) * arc_r
        })
        .collect();
    for (w, a) in [(9.0, 0.10), (5.5, 0.22), (2.8 + 0.6 * vh, 1.0)] {
        ui.painter().add(egui::Shape::Path(egui::epaint::PathShape::line(pts.clone(), egui::epaint::PathStroke::new(w, theme::with_alpha(accent, a * reveal)))));
    }
    // Round caps: egui paths end square, the reference's glow doesn't.
    if len > 0.01 {
        for p in [pts[0], *pts.last().unwrap()] {
            ui.painter().circle_filled(p, 1.5, theme::with_alpha(accent, reveal));
        }
    }
    // The level, said the way the reference says it: MAX near the crown, the bare
    // number under way, both at the heart of the half disc.
    let label = if vol >= 0.995 { "MAX".to_owned() } else { format!("{}", (vol * 100.0).round()) };
    ui.painter().text(pos2(at.x + r * 0.5, at.y - 24.0), egui::Align2::CENTER_CENTER, &label, theme::bold(26.0), theme::with_alpha(Color32::WHITE, reveal));
    // The speaker in its puck under the number: a click mutes (the player bar's own
    // trip), a hover rings it in the accent, as the reference's hovered icon.
    let chip = Rect::from_center_size(pos2(at.x + r * 0.5, at.y + 26.0), Vec2::splat(34.0));
    let mresp = ui.interact(chip, egui::Id::unique("np-vol-mute"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    let mh = theme::anim_bool(ui.ctx(), mresp.id.with("h"), mresp.hovered(), 0.15);
    ui.painter().circle_filled(chip.center(), 17.0, theme::glass(0.10 * reveal));
    if mh > 0.0 {
        ui.painter().circle_stroke(chip.center(), 19.0, egui::Stroke::new(2.0, theme::with_alpha(accent, mh * reveal)));
    }
    glyphs::draw(ui, chip.center(), 17.0, glyphs::VOLUME[glyphs::volume_step(vol)], theme::with_alpha(Color32::WHITE, reveal));
    ui.set_clip_rect(outer);
    if mresp.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            app.audio.set_volume((vol + scroll / 600.0).clamp(0.0, 1.0));
        }
    }
    if mresp.clicked() {
        super::shell::toggle_mute(app, ui.ctx());
    }
}

/// What's heard while the sleeve shows `track`: a handoff's outgoing song stands in for
/// the queue's, otherwise it's whatever actually plays. The clone is for the menus,
/// which need `app` mutably beside it — so it's taken only where they're drawn, not for
/// the whole frame (the video stage has no use for it).
fn heard_track(app: &App, track: &crate::model::Track, hold: bool) -> crate::model::Track {
    if hold { track.clone() } else { app.playing_track().cloned().unwrap_or_else(|| track.clone()) }
}

/// Song or Video, in the middle of the header, as YouTube Music's own apps have it.
fn song_video_switch(app: &mut App, ui: &mut Ui, header: Rect) {
    let rect = Rect::from_center_size(header.center(), vec2(170.0, header.height()));
    let mut row = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::left_to_right(egui::Align::Center)));
    row.spacing_mut().item_spacing.x = 6.0;
    let picked = app.video_picked();
    let song = widgets::chip(&mut row, "Song", !picked).clicked();
    let video = widgets::chip(&mut row, "Video", picked).clicked();
    if (song && picked) || (video && !picked) {
        // On a song, its switch swaps in (or back from) its music video.
        let on_song = app.current().is_some_and(|t| !t.has_video());
        app.actions.push(if on_song { Action::SongVideo(!picked) } else { Action::ToggleVideo });
    }
}

/// The video as the whole view, with Up next or Comments beside it when that's open.
fn video_stage(app: &mut App, ui: &mut Ui, content: Rect, track: &crate::model::Track) {
    let frame = video_frame(app, ui.ctx(), track);
    // Up next or Comments: beside the video, or under it when there's no room.
    let comments = app.np_tab == NpTab::Comments && app.show_comments;
    let side = app.np_tab == NpTab::UpNext || comments;
    let queue_w = if side && content.width() > 900.0 { 360.0 } else { 0.0 };
    let queue_h = if side && queue_w == 0.0 { content.height() * 0.45 } else { 0.0 };
    let stage = Rect::from_min_max(content.min, pos2(content.right() - queue_w, content.bottom() - queue_h));
    let (w, h) = frame.as_ref().map_or((16.0, 9.0), |t| (t.size()[0] as f32, t.size()[1] as f32));
    let scale = (stage.width() / w.max(1.0)).min((stage.height() - 64.0).max(120.0) / h.max(1.0));
    let fit = vec2(w * scale, h * scale);
    // In the middle of what's left over, with the title under it (the 64 kept for it): a
    // window taller than the video is wide leaves room above and below, not all below.
    let top = stage.top() + ((stage.height() - 64.0 - fit.y) / 2.0).max(0.0);
    let picture = Rect::from_center_size(pos2(stage.center().x, top + fit.y / 2.0), fit);
    let radius = egui::CornerRadius::same(10);
    match &frame {
        Some(tex) => {
            egui::Image::from_texture((tex.id(), fit)).corner_radius(radius).paint_at(ui, picture);
        }
        None => video_placeholder(ui, picture, track, radius.nw as f32),
    }
    let cover = named(ui.interact(picture, ui.scope_id().with(("video-playback", &track.id)), Sense::click()), if app.playing() { "Pause" } else { "Play" }).on_hover_cursor(egui::CursorIcon::PointingHand);
    let hover = theme::anim_bool(ui.ctx(), ("video-playback-hover", &track.id), cover.hovered(), 0.16);
    if hover > 0.0 {
        ui.painter().circle_filled(picture.center(), 38.0, theme::shade(0.58 * hover));
        widgets::icon_at(ui, picture.center(), if app.playing() || app.buffering { icon::PAUSE } else { icon::PLAY }, 37.0, theme::with_alpha(Color32::WHITE, hover));
    }
    if cover.double_clicked() {
        // The first of the two clicks paused it, or played it: back as it was.
        app.actions.push(Action::TogglePlay);
        app.set_video_fullscreen(ui.ctx(), true);
    } else if cover.clicked() {
        app.actions.push(Action::TogglePlay);
    }
    // Picture in picture at the video's top right while it's pointed at, full screen at
    // its bottom right.
    if ui.rect_contains_pointer(picture) || hover > 0.0 {
        let over = if ui.rect_contains_pointer(picture) { 1.0 } else { hover };
        let pip = pos2(picture.right() - 30.0, picture.top() + 30.0);
        if corner_button(ui, pip, icon::PICTURE_IN_PICTURE, "Picture in picture (Ctrl+M)", over).clicked() {
            app.actions.push(Action::Mini(true));
        }
        let full = pos2(picture.right() - 30.0, picture.bottom() - 30.0);
        if corner_button(ui, full, icon::EXPAND, "Full screen (F)", over).clicked() {
            app.set_video_fullscreen(ui.ctx(), true);
        }
    }
    let y = picture.bottom() + 12.0;
    widgets::title_at(ui, pos2(picture.left(), y), &track.title, track.explicit, theme::bold(20.0), Color32::WHITE, fit.x);
    let line = track.artist_line();
    widgets::linked_text(ui, pos2(picture.left(), y + 28.0), &line, &track.artists, theme::regular(14.0), theme::text_dim(), fit.x, &mut app.actions);
    if side {
        let r = if queue_w > 0.0 { Rect::from_min_max(pos2(stage.right() + 24.0, content.top()), content.max) } else { Rect::from_min_max(pos2(content.left(), stage.bottom() + 8.0), content.max) };
        let mut q = ui.new_child(egui::UiBuilder::new().max_rect(r));
        q.set_clip_rect(r.intersect(ui.clip_rect()));
        if comments {
            super::comments::panel(app, &mut q);
        } else {
            super::shell::queue_list(app, &mut q, "np", 14.0);
        }
    }
}

/// A round button over the video, as bright as `shown` (the pointer's on the video).
fn corner_button(ui: &Ui, c: egui::Pos2, glyph: char, name: &str, shown: f32) -> egui::Response {
    let b = Rect::from_center_size(c, Vec2::splat(40.0));
    let resp = named(ui.interact(b, ui.scope_id().with(("video-corner", name)), Sense::click()), name).on_hover_text(name).on_hover_cursor(egui::CursorIcon::PointingHand);
    let shown = if resp.hovered() { 1.0 } else { shown };
    ui.painter().circle_filled(c, 20.0, theme::shade(if resp.hovered() { 0.75 } else { 0.5 } * shown));
    widgets::icon_at(ui, c, glyph, 18.0, theme::with_alpha(Color32::WHITE, shown));
    resp
}

/// Since when the pointer has been still over the full-screen video.
fn still_since_id() -> egui::Id {
    egui::Id::unique("video-full-still-since")
}

/// Going into the video's full screen: its controls come up at first.
pub(super) fn wake_video_controls(ctx: &egui::Context) {
    ctx.data_mut(|d| d.remove::<f64>(still_since_id()));
}

/// Full screen with just the video, on black, as on YouTube: the song's title and the
/// controls come up while the pointer moves, and go, the pointer with them, once it has
/// been still a moment away from them. A click plays or pauses, a double-click leaves.
fn video_fullscreen(app: &mut App, ui: &mut Ui, rect: Rect, track: &crate::model::Track, now: f64) {
    let ctx = ui.ctx().clone();
    ui.painter().rect_filled(rect, 0.0, Color32::BLACK);
    let frame = video_frame(app, &ctx, track);
    let (w, h) = frame.as_ref().map_or((16.0, 9.0), |t| (t.size()[0] as f32, t.size()[1] as f32));
    // Not while typing: into the command palette, say.
    if !ctx.egui_wants_keyboard_input() && ui.input(|i| i.key_pressed(egui::Key::Z) && !i.modifiers.any()) {
        set_video_fill(app, !app.video_fill);
    }
    // Filling: the picture inside its own bars, as large as covers the screen, its edges cut.
    let content = if app.video_fill { app.video_content.unwrap_or(Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))) } else { Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)) };
    let (cw, ch) = ((w * content.width()).max(1.0), (h * content.height()).max(1.0));
    let scale = if app.video_fill { (rect.width() / cw).max(rect.height() / ch) } else { (rect.width() / cw).min(rect.height() / ch) };
    let uv = Rect::from_center_size(content.center(), vec2((rect.width() / scale / w).min(content.width()), (rect.height() / scale / h).min(content.height())));
    let picture = Rect::from_center_size(rect.center(), vec2(uv.width() * w * scale, uv.height() * h * scale));
    match &frame {
        Some(tex) => {
            ui.painter().image(tex.id(), picture, uv, Color32::WHITE);
        }
        None => video_placeholder(ui, picture, track, 0.0),
    }
    let stage = named(ui.interact(rect, ui.scope_id().with("video-full-stage"), Sense::click()), if app.playing() { "Pause" } else { "Play" });
    if stage.double_clicked() {
        // The first of the two clicks paused it, or played it: back as it was.
        app.actions.push(Action::TogglePlay);
        app.set_fullscreen(&ctx, false);
        return;
    } else if stage.clicked() {
        app.actions.push(Action::TogglePlay);
    }
    let bar_h = 104.0;
    let pointer = ui.input(|i| i.pointer.hover_pos());
    let on_controls = pointer.is_some_and(|p| p.y >= rect.bottom() - bar_h || p.y <= rect.top() + 88.0);
    let moved = ui.input(|i| i.pointer.delta() != Vec2::ZERO || !i.events.is_empty());
    let since = ctx.data(|d| d.get_temp::<f64>(still_since_id()));
    let since = match since {
        Some(t) if !moved => t,
        _ => {
            ctx.data_mut(|d| d.insert_temp(still_since_id(), now));
            now
        }
    };
    let busy = on_controls || app.seek_drag.is_some() || egui::Popup::is_any_open(&ctx) || !app.playing();
    let quiet = !busy && now - since > 2.5;
    let shown = theme::anim_bool(&ctx, "video-full-controls", !quiet, 0.25);
    if !quiet {
        // To see the pointer has been still long enough.
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    } else if pointer.is_some() {
        ctx.set_cursor_icon(egui::CursorIcon::None);
    }
    if shown <= 0.02 {
        return;
    }
    let painter = ui.painter();
    let top = Rect::from_min_size(rect.min, vec2(rect.width(), 120.0));
    super::shell::vgradient(painter, top, theme::shade(0.6 * shown), Color32::TRANSPARENT);
    let bottom = Rect::from_min_max(pos2(rect.left(), rect.bottom() - 160.0), rect.max);
    super::shell::vgradient(painter, bottom, Color32::TRANSPARENT, theme::shade(0.75 * shown));
    let mut cui = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    cui.multiply_opacity(shown);
    let ui = &mut cui;
    let text_w = rect.width() - 56.0;
    widgets::title_at(ui, pos2(rect.left() + 28.0, rect.top() + 22.0), &track.title, track.explicit, theme::bold(20.0), Color32::WHITE, text_w);
    let line = track.artist_line();
    widgets::linked_text(ui, pos2(rect.left() + 28.0, rect.top() + 50.0), &line, &track.artists, theme::regular(15.0), theme::text_dim(), text_w, &mut app.actions);

    // The seek bar, thickening with a knob while pointed at or dragged.
    let status = app.audio.status.clone();
    let total = status.duration().map(|d| d.as_secs_f32()).or(track.duration.map(|d| d as f32)).unwrap_or(0.0);
    let pos = app.seek_drag.unwrap_or(status.position_smooth().as_secs_f32()).min(total.max(0.0));
    let line_y = rect.bottom() - 66.0;
    let hit = Rect::from_min_max(pos2(rect.left() + 20.0, line_y - 8.0), pos2(rect.right() - 20.0, line_y + 8.0));
    let seek = ui.interact(hit, ui.scope_id().with("video-full-seek"), Sense::click_and_drag());
    let seek = if total > 0.0 { seek.on_hover_cursor(egui::CursorIcon::PointingHand) } else { seek };
    seek.widget_info(|| egui::WidgetInfo::slider(total > 0.0, pos as f64, "Seek"));
    let lit = theme::anim_bool(&ctx, "video-full-seek-h", seek.hovered() || seek.dragged(), 0.15);
    let thick = 3.0 + 2.0 * lit;
    let track_r = Rect::from_center_size(pos2(hit.center().x, line_y), vec2(hit.width(), thick));
    ui.painter().rect_filled(track_r, thick / 2.0, theme::with_alpha(Color32::WHITE, 0.25));
    let frac = if total > 0.0 { pos / total } else { 0.0 };
    let filled = Rect::from_min_size(track_r.min, vec2(track_r.width() * frac, thick));
    let accent = app.accent_color();
    ui.painter().rect_filled(filled, thick / 2.0, accent);
    if lit > 0.0 && total > 0.0 {
        ui.painter().circle_filled(pos2(filled.right(), line_y), 6.5 * lit, accent);
        if let Some(p) = seek.hover_pos() {
            let t = ((p.x - track_r.left()) / track_r.width()).clamp(0.0, 1.0) * total;
            let g = widgets::time_galley(ui, &super::fmt_time(t as u32), theme::semibold(12.0), Color32::WHITE);
            let tip = Rect::from_center_size(pos2(p.x, line_y - 22.0), g.size() + vec2(12.0, 6.0));
            ui.painter().rect_filled(tip, 6.0, theme::shade(0.75));
            ui.painter().galley(tip.min + vec2(6.0, 3.0), g, Color32::WHITE);
        }
    }
    if total > 0.0
        && let Some(p) = seek.interact_pointer_pos()
    {
        let t = ((p.x - track_r.left()) / track_r.width()).clamp(0.0, 1.0) * total;
        if seek.dragged() {
            app.seek_drag = Some(t);
        }
        if seek.drag_stopped() || seek.clicked() {
            app.seek_drag = None;
            app.actions.push(Action::Seek(t));
        }
    }

    // The buttons under it: playing and the volume on the left, the rest on the right.
    let row = Rect::from_min_max(pos2(rect.left() + 12.0, line_y + 10.0), pos2(rect.right() - 12.0, rect.bottom() - 6.0));
    let mut left = ui.new_child(egui::UiBuilder::new().max_rect(row).layout(egui::Layout::left_to_right(egui::Align::Center)));
    left.spacing_mut().item_spacing.x = 2.0;
    if widgets::icon_button(&mut left, icon::SKIP_BACK, 20.0, "Previous", false).clicked() {
        app.prev();
    }
    let playing = app.playing() || app.buffering;
    if widgets::icon_button(&mut left, if playing { icon::PAUSE } else { icon::PLAY }, 24.0, if playing { "Pause" } else { "Play" }, false).clicked() {
        app.actions.push(Action::TogglePlay);
    }
    if widgets::icon_button(&mut left, icon::SKIP_FORWARD, 20.0, "Next", false).clicked() {
        app.next();
    }
    let vol = app.audio.volume();
    let vglyph = if vol == 0.0 { icon::VOLUME_X } else if vol < 0.5 { icon::VOLUME_1 } else { icon::VOLUME_2 };
    if widgets::icon_button(&mut left, vglyph, 19.0, if vol > 0.0 { "Mute" } else { "Unmute" }, false).clicked() {
        let stash = super::shell::unmute_id();
        if vol > 0.0 {
            left.data_mut(|d| d.insert_temp(stash, vol));
            app.audio.set_volume(0.0);
        } else {
            let v = left.data(|d| d.get_temp::<f32>(stash)).unwrap_or(0.6);
            app.audio.set_volume(v);
        }
    }
    let (slider, _) = left.allocate_exact_size(vec2(90.0, 20.0), Sense::hover());
    super::shell::volume_slider(app, &mut left, slider, "video-full-vol");
    left.add_space(14.0);
    if total > 0.0 {
        let time = format!("{} / {}", super::fmt_time(pos as u32), super::fmt_time(total as u32));
        let g = widgets::time_galley(&left, &time, theme::regular(13.5), Color32::WHITE);
        left.label(g);
    }
    let mut right = ui.new_child(egui::UiBuilder::new().max_rect(row).layout(egui::Layout::right_to_left(egui::Align::Center)));
    right.spacing_mut().item_spacing.x = 2.0;
    if widgets::icon_button(&mut right, icon::SHRINK, 20.0, "Exit full screen (Esc)", false).clicked() {
        app.set_fullscreen(&ctx, false);
    }
    if widgets::icon_button(&mut right, icon::PICTURE_IN_PICTURE, 19.0, "Picture in picture (Ctrl+M)", false).clicked() {
        app.actions.push(Action::Mini(true));
    }
    let fill_tip = if app.video_fill { "Fit to screen (Z)" } else { "Fill the screen (Z)" };
    if widgets::icon_button(&mut right, icon::MAXIMIZE_2, 18.0, fill_tip, app.video_fill).clicked() {
        set_video_fill(app, !app.video_fill);
    }
    right.add_space(6.0);
    // What it plays at: a video may not come as sharp as asked.
    let playing_at = app.video.as_ref().and_then(|(_, p)| p.as_ref()).map(crate::video::Player::height).filter(|&h| h > 0);
    quality_button(app, &mut right, playing_at);
}

/// Full screen fills the screen with the video, or fits it whole. Looks for its bars afresh.
fn set_video_fill(app: &mut App, on: bool) {
    app.video_fill = on;
    app.video_content = None;
    app.video_content_at = 0.0;
}

/// The video's quality, a menu of the others above it.
fn quality_button(app: &mut App, ui: &mut Ui, playing_at: Option<u32>) {
    let label = format!("{}p", playing_at.unwrap_or(app.video_quality));
    let g = widgets::galley(ui, &label, theme::semibold(13.0), Color32::WHITE, 80.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(g.size().x + 44.0, 32.0), Sense::click());
    let resp = named(resp, "Video quality").on_hover_text("Video quality").on_hover_cursor(egui::CursorIcon::PointingHand);
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&resp));
    let h = theme::anim_bool(ui.ctx(), resp.id.with("h"), resp.hovered() || open, 0.12);
    ui.painter().rect_filled(rect, 16.0, theme::glass(0.10 + 0.08 * h));
    widgets::icon_at(ui, pos2(rect.left() + 17.0, rect.center().y), icon::SETTINGS, 15.0, Color32::WHITE);
    ui.painter().galley(pos2(rect.left() + 30.0, rect.center().y - g.size().y / 2.0), g, Color32::WHITE);
    egui::Popup::menu(&resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        ui.set_width(170.0);
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        ui.add_space(4.0);
        quality_rows(app, ui);
        ui.add_space(4.0);
    });
}

/// The video qualities to pick from, in a menu. Settings › Music videos has the same.
fn quality_rows(app: &mut App, ui: &mut Ui) {
    menu_caption(ui, "VIDEO QUALITY");
    for q in super::VIDEO_QUALITIES {
        if super::pages::option_row(ui, &format!("{q}p"), q == app.video_quality).clicked() {
            set_video_quality(app, q);
            ui.close();
        }
    }
}

/// While a video is on its way, or can't be shown: its own thumbnail in its place, the
/// whole 16:9 of it, sharp once YouTube's biggest one has loaded.
pub(super) fn video_placeholder(ui: &Ui, picture: Rect, track: &crate::model::Track, radius: f32) {
    ui.painter().rect_filled(picture, radius, Color32::BLACK);
    if let Some(t) = track.thumbs.iter().max_by_key(|t| t.width) {
        let ratio = t.width as f32 / t.height.max(1) as f32;
        // 16:9 inside a 4:3 frame, as the smaller ones are: the middle three quarters.
        let uv = if (1.2..1.6).contains(&ratio) { Rect::from_min_max(pos2(0.0, 0.125), pos2(1.0, 0.875)) } else { Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)) };
        widgets::paint_remote(ui, &t.url, picture, radius, uv, Color32::WHITE);
        if let Some(big) = crate::model::ytimg_max(&t.url) {
            widgets::paint_when_ready(ui, &big, picture, radius, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)));
        }
    }
}

/// The video's colours, eased toward the latest frame's, across all of Now playing.
fn paint_glow(app: &mut App, ui: &mut Ui, rect: Rect) {
    let Some(Glow { current, target, tex }) = &mut app.glow else { return };
    let k = (ui.input(|i| i.stable_dt) * 2.5).clamp(0.0, 1.0);
    for (c, t) in current.iter_mut().zip(target.iter()) {
        for i in 0..3 {
            c[i] += (t[i] - c[i]) * k;
        }
    }
    let [w, h] = crate::video::GLOW_SIZE;
    let pixels = current.iter().map(|c| Color32::from_rgb(c[0] as u8, c[1] as u8, c[2] as u8)).collect();
    tex.set(egui::ColorImage { size: [w, h], source_size: vec2(w as f32, h as f32), pixels }, egui::TextureOptions::LINEAR);
    // Bigger than the view, so its coarse edges stay out of sight.
    let full = rect.expand2(rect.size() * 0.08);
    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    let painter = ui.painter_at(rect);
    painter.image(tex.id(), full, uv, Color32::from_gray(150));
    painter.rect_filled(rect, 0.0, theme::shade(0.25));
}

/// The video's latest frame in a texture. Keeps the player, and the glow's colours, in
/// step with what's playing.
pub(super) fn video_frame(app: &mut App, ctx: &egui::Context, track: &crate::model::Track) -> Option<egui::TextureHandle> {
    if app.video.as_ref().is_none_or(|(id, _)| *id != track.id) {
        app.video = Some((track.id.clone(), None));
        app.video_frame = None;
        app.video_content = None;
        app.glow = None;
        app.backend.request(crate::backend::Req::VideoStream { video_id: track.id.clone(), max_height: app.video_quality });
    }
    // On its way at another quality: the last picture stays up meanwhile.
    let Some(player) = app.video.as_ref()?.1.as_ref() else { return app.video_frame.clone() };
    if player.failed() {
        return None;
    }
    if let Some(frame) = player.take_due() {
        if app.ambient_light {
            let colors = crate::video::glow_colors(&frame.image);
            match &mut app.glow {
                Some(glow) => glow.target = colors,
                None => {
                    let [w, h] = crate::video::GLOW_SIZE;
                    let image = egui::ColorImage { size: [w, h], source_size: vec2(w as f32, h as f32), pixels: vec![Color32::BLACK; w * h] };
                    let tex = ctx.load_texture("video-glow", image, egui::TextureOptions::LINEAR);
                    app.glow = Some(Glow { current: colors.clone(), target: colors, tex });
                }
            }
        }
        // Where its own black bars are, for filling the screen: a few times a second, the
        // picture widening as brighter frames show more of it.
        let now = ctx.input(|i| i.time);
        if app.video_fill && app.video_full && now - app.video_content_at > 0.5 {
            app.video_content_at = now;
            if let Some(seen) = crate::video::content_uv(&frame.image) {
                app.video_content = Some(app.video_content.map_or(seen, |c| c.union(seen)));
            }
        }
        match &mut app.video_frame {
            Some(tex) => tex.set(frame.image, egui::TextureOptions::LINEAR),
            None => app.video_frame = Some(ctx.load_texture("music-video", frame.image, egui::TextureOptions::LINEAR)),
        }
    }
    if app.playing() {
        super::pace::repaint_after(ctx, std::time::Duration::from_millis(16));
    } else if app.video_frame.is_none() {
        // Paused or still loading: the first frame shows when it's here, or after a seek.
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
    app.video_frame.clone()
}

/// Play videos at most `height` tall, the one on show too: it carries on from where the
/// song is, at the new quality, once that has loaded.
pub(super) fn set_video_quality(app: &mut App, height: u32) {
    if app.video_quality == height {
        return;
    }
    app.video_quality = height;
    if let Some((id, player)) = &mut app.video {
        *player = None;
        app.backend.request(crate::backend::Req::VideoStream { video_id: id.clone(), max_height: height });
    }
}

/// Size of the artwork laid out in `col`: what is left once the sleeve's information,
/// its bar and the transport underneath it have taken their room.
fn art_side(cs: &crate::preset::CoverStyle, col: Rect) -> f32 {
    (col.width() * 0.82 * cs.size.clamp(0.5, 1.35)).min(col.height() - 190.0).max(160.0)
}

/// The sleeve: art, title and artists, the seek line with its two times, then the
/// transport — the order the reference keeps them in. `heard`: what's heard (the song
/// version of the video on show, the outgoing song of a handoff), for its star and
/// menus. `px`: the size to fetch the image at (fixed while the column glides).
fn cover_column(app: &mut App, ui: &mut Ui, col: Rect, track: &crate::model::Track, heard: &crate::model::Track, no_lyrics: bool, now: f64, px: u32) {
    use crate::preset::Shape;
    // A handoff's outgoing song is still what's heard: its seek is its own.
    let hold = app.mix_hold().map(|h| (h.total, h.mix, app.mix_hold_pos(h)));
    let look = app.look();
    let cs = &look.cover;
    let side = art_side(cs, col);
    let art = Rect::from_center_size(pos2(col.center().x, col.top() + side / 2.0 + (col.height() - side - 190.0).max(0.0) * 0.42), Vec2::splat(side));
    let round = cs.shape != Shape::Rounded;
    let radius = if round { side / 2.0 } else { cs.radius.clamp(0.0, side / 2.0) };
    let cr = egui::CornerRadius::same(radius.min(255.0) as u8);
    // Settings › Appearance: the sleeve's shadow and glow can be left out entirely.
    if app.cover_fx && cs.glow > 0.0 {
        let s = egui::epaint::Shadow { offset: [0, 16], blur: 90, spread: 6, color: theme::with_alpha(app.accent_color(), cs.glow.min(1.0) * 0.7) };
        ui.painter().add(s.as_shape(art, cr));
    }
    if app.cover_fx && cs.shadow > 0.0 {
        widgets::soft_shadow(ui, art, radius.min(255.0), 60.0, cs.shadow.min(1.0));
    }
    let tint = Color32::from_gray((cs.brightness.clamp(0.1, 1.0) * 255.0) as u8);
    if round {
        // Reduce motion (Settings › Appearance) stops the spin, not the shape.
        if cs.shape == Shape::Record && app.playing() && !app.reduce_motion {
            let dt = ui.input(|i| i.stable_dt).min(0.1) as f64;
            app.record_angle = (app.record_angle + dt * std::f64::consts::TAU / cs.spin_secs.max(4.0) as f64) % std::f64::consts::TAU;
        }
        let angle = if cs.shape == Shape::Record { app.record_angle as f32 } else { 0.0 };
        widgets::paint_disc(ui, egui::Id::unique("np-art"), art.center(), side / 2.0, &track.thumbs, angle, tint, px);
        if cs.shape == Shape::Record {
            // Vinyl: faint grooves and a spindle hole.
            let p = ui.painter();
            for i in 1..6 {
                p.circle_stroke(art.center(), side / 2.0 * (0.42 + i as f32 * 0.1), egui::Stroke::new(1.0, theme::shade(0.18)));
            }
            p.circle_filled(art.center(), side * 0.09, theme::shade(0.35));
            p.circle_filled(art.center(), side * 0.022, Color32::from_rgb(10, 10, 12));
        }
    } else {
        widgets::paint_cover_xfade_px(ui, egui::Id::unique("np-art"), art, &track.thumbs, radius, 1.0, px);
        // The song's animated cover over the still until its first frame arrives:
        // Apple's clip for its release, or Spotify's canvas where Apple has none.
        if let Some(s) = app.animated_cover(ui.ctx(), track, px) {
            widgets::paint_motion(ui, art, radius, &s);
        }
        if cs.brightness < 0.99 {
            ui.painter().rect_filled(art, cr, theme::shade(1.0 - cs.brightness));
        }
    }
    // Stats for nerds, on the foot of the sleeve: what the decoder is being fed.
    nerd_stats(app, ui, art, round);
    // Title and artists: 17 semibold over 15 secondary, left-aligned to the art.
    let w = side;
    let x0 = art.left();
    let info_y = art.bottom() + 22.0;
    // The star and the ⋯ menu share the information line's right end.
    let text_w = w - 76.0;
    widgets::title_at(ui, pos2(x0, info_y), &track.title, track.explicit, theme::semibold(17.0), Color32::WHITE, text_w);
    // Each artist opens their own page (features included); the album follows the
    // artist behind an em dash, as the reference writes the line.
    let mut line = track.artist_line();
    if let Some(album) = &track.album
        && !album.name.is_empty()
    {
        line = format!("{line} — {}", album.name);
    }
    let line_w = text_w;
    let line_rect = widgets::linked_text(ui, pos2(x0, info_y + 21.0), &line, &track.artists, theme::regular(15.0), theme::text_dim(), line_w, &mut app.actions);
    // The album after the em dash opens its page too, underlined like the artists.
    if let Some(album) = track.album.as_ref().filter(|a| !a.name.is_empty())
        && let Some(id) = album.id.clone()
    {
        let g = widgets::galley(ui, &line, theme::regular(15.0), theme::text_dim(), line_w);
        if let Some(at) = g.job.text.rfind(album.name.as_str()) {
            let c0 = g.job.text[..at].chars().count();
            let c1 = c0 + album.name.chars().count();
            let a0 = g.pos_from_cursor(egui::text::CCursor::new(c0)).min.x;
            let a1 = g.pos_from_cursor(egui::text::CCursor::new(c1)).min.x;
            if a1 - a0 >= 2.0 {
                let r = Rect::from_min_max(pos2(line_rect.left() + a0, line_rect.top()), pos2(line_rect.left() + a1, line_rect.bottom()));
                let resp = ui.interact(r, ui.scope_id().with(("album-link", &album.name)), Sense::click());
                let resp = named(resp, &album.name).on_hover_cursor(egui::CursorIcon::PointingHand);
                if resp.hovered() {
                    ui.painter().hline(r.x_range(), r.bottom() - 1.0, egui::Stroke::new(1.0, theme::text_dim()));
                }
                if resp.clicked() {
                    app.actions.push(Action::Open(super::Target::Album(id)));
                }
            }
        }
    }
    // The star: a 30 px disc that washes under the pointer and fills in the accent
    // once liked. The ⋯ menu sits at the line's end, as on music.youtube.com.
    let liked = app.liked.contains(&heard.id);
    let star = Rect::from_center_size(pos2(art.right() - 55.0, info_y + 14.0), Vec2::splat(30.0));
    let sr = ui.interact(star, ui.scope_id().with(("np-like", &heard.id)), Sense::click());
    let sr = named(sr, if liked { "Remove from favourites" } else { "Add to favourites" }).on_hover_cursor(egui::CursorIcon::PointingHand);
    let sh = theme::anim_bool(ui.ctx(), sr.id.with("h"), sr.hovered(), 0.1);
    let pop = theme::anim_bool(ui.ctx(), ("liked", &heard.id), liked, 0.3);
    // The same translucent disc as the ⋯ beside it, a little brighter once liked.
    let disc = (0.09 + 0.07 * sh + 0.05 * pop).min(0.30);
    if disc > 0.0 {
        ui.painter().circle_filled(star.center(), 15.0, theme::glass(disc));
    }
    let dim = 0.85 + 0.15 * sh;
    let tint = theme::lerp_color(theme::with_alpha(Color32::WHITE, dim), app.accent_color(), pop);
    super::glyphs::draw(ui, star.center(), 19.0, super::glyphs::STAR, tint);
    if pop > 0.0 {
        super::glyphs::draw(ui, star.center(), 19.0, super::glyphs::STAR_FILLED, theme::with_alpha(app.accent_color(), pop));
    }
    if sr.clicked() {
        app.actions.push(Action::Like(heard.clone(), !liked));
    }
    // The song's options, beside the star.
    let options = Rect::from_center_size(pos2(art.right() - 15.0, info_y + 14.0), Vec2::splat(34.0));
    options_menu(app, ui, options, heard, app.np_tab == NpTab::Lyrics && !no_lyrics);

    // Seek: a 4 px line that grows under the pointer, its times below.
    let status = app.audio.status.clone();
    let held = hold.is_some();
    let (total, mix) = match &hold {
        Some((total, mix, _)) => (*total, *mix),
        None => {
            let total = status
                .duration()
                .map(|d| d.as_secs_f32())
                .or(track.duration.map(|d| d as f32))
                .unwrap_or(0.0);
            let next = app.queue.upcoming().is_some() || app.autoplay;
            let mix = match (total > 0.0, next, app.queue.repeat) {
                (true, true, repeat) if repeat != Repeat::One => {
                    status.mix_secs(status.duration().or_else(|| track.duration.map(|s| std::time::Duration::from_secs(s as u64)))).min(total * 0.5)
                }
                _ => 0.0,
            };
            (total, mix)
        }
    };
    let smooth = match &hold {
        Some((_, _, pos)) => *pos,
        None => status.position_smooth().as_secs_f32(),
    };
    let pos = app.seek_drag.unwrap_or(smooth).min(total.max(0.0));
    let hit = Rect::from_min_size(pos2(x0, info_y + 55.0), vec2(w, 16.0));
    // Held for the handoff's first half: its last stretch is where the pointer is,
    // and seeking the queue's song from here would cut the mix.
    let sresp = ui.interact(hit, ui.scope_id().with("np-seek"), if held { Sense::hover() } else { Sense::click_and_drag() });
    let sresp = if total > 0.0 && !held { sresp.on_hover_cursor(egui::CursorIcon::PointingHand) } else { sresp };
    sresp.widget_info(|| egui::WidgetInfo::slider(total > 0.0 && !held, pos as f64, "Seek"));
    let focus = theme::anim_bool(ui.ctx(), "np-seek-h", total > 0.0 && !held && (sresp.hovered() || sresp.dragged()), 0.18);
    let thick = 4.0 + 4.0 * focus;
    let bar = Rect::from_min_max(pos2(x0, hit.center().y - thick / 2.0), pos2(x0 + w, hit.center().y + thick / 2.0));
    ui.painter().rect_filled(bar, thick / 2.0, Color32::from_white_alpha(38));
    // Automix: the handoff window tints to the song's colour with its start ticked, so
    // the blend has a place it visibly begins at.
    if mix > 0.5 {
        let edge = x0 + w * ((total - mix) / total);
        let band = Rect::from_min_max(pos2(edge, bar.top()), bar.max);
        ui.painter().rect_filled(band, thick / 2.0, theme::with_alpha(app.accent_color(), 0.32));
        let tick = Rect::from_min_size(pos2(edge - 1.0, bar.top()), vec2(2.0, thick));
        ui.painter().rect_filled(tick, 1.0, theme::with_alpha(app.accent_color(), 0.9));
    }
    let frac = if total > 0.0 { (pos / total).clamp(0.0, 1.0) } else { 0.0 };
    if frac > 0.0 {
        let fill = theme::lerp_color(Color32::from_white_alpha(163), Color32::from_white_alpha(240), focus);
        ui.painter().rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * frac, thick)), thick / 2.0, fill);
    }
    let at = |p: egui::Pos2| (((p.x - x0) / w.max(1.0)).clamp(0.0, 1.0) * total).max(0.0);
    let tc = theme::with_alpha(Color32::WHITE, 0.62);
    let g = widgets::time_galley(ui, &super::fmt_time(pos as u32), theme::semibold(11.0), tc);
    ui.painter().galley(pos2(x0, hit.bottom() + 2.0), g, tc);
    let left = super::fmt_time((total - pos).max(0.0) as u32);
    let g = widgets::time_galley(ui, &format!("-{left}"), theme::semibold(11.0), tc);
    ui.painter().galley(pos2(x0 + w - g.size().x, hit.bottom() + 2.0), g, tc);
    // The quality mark sits between the two times, as the reference draws it.
    let badge_w = audio_badge_width(ui, app, false);
    if badge_w > 0.0 {
        audio_badge(app, ui, pos2(x0 + (w - badge_w) / 2.0, hit.bottom() + 1.0), false);
    }
    // While the two songs blend, its name sits between the two times.
    let mixed = theme::anim_bool(ui.ctx(), "np-automix-on", app.mixing(), 0.3);
    if mixed > 0.01 && badge_w == 0.0 {
        let accent = theme::with_alpha(app.accent_color(), mixed);
        let g = widgets::galley(ui, "Mixing", theme::semibold(10.5), accent, 80.0);
        ui.painter().galley(pos2(x0 + (w - g.size().x) / 2.0, hit.bottom() + 3.0), g, accent);
    }
    if total > 0.0
        && let Some(p) = sresp.interact_pointer_pos()
    {
        if sresp.dragged() {
            app.seek_drag = Some(at(p));
        }
        if sresp.drag_stopped() || sresp.clicked() {
            app.seek_drag = None;
            app.actions.push(Action::Seek(at(p)));
        }
    }

    // Transport: the reference's row — shuffle, previous, play, next, repeat — 52 tall,
    // no wider than 315, its buttons 36/48/56/48/36 with icons of 30/40/48/40/30.
    let ty = info_y + 99.0 + 26.0;
    let row_w = w.min(315.0);
    let sizes = [36.0f32, 48.0, 56.0, 48.0, 36.0];
    let icons = [30.0f32, 40.0, 48.0, 40.0, 30.0];
    let gap = ((row_w - sizes.iter().sum::<f32>()) / 4.0).max(4.0);
    let mut xs = [0.0f32; 5];
    let mut cx = art.center().x - row_w / 2.0;
    for (i, s) in sizes.iter().enumerate() {
        xs[i] = cx + s / 2.0;
        cx += s + gap;
    }
    // The marks are primary white, an off shuffle or repeat tertiary; a pressed button
    // shrinks to 90%, as the reference's does.
    let ink = |h: f32, base: f32, on: f32| theme::with_alpha(theme::lerp_color(theme::with_alpha(Color32::WHITE, base), Color32::WHITE, h), on);
    let enabled = app.current().is_some();
    let dim = if enabled { 1.0 } else { 0.4 };
    let btn = |ui: &mut Ui, i: usize, key: &str, name: &str, live: bool| -> (egui::Response, f32, f32) {
        let r = Rect::from_center_size(pos2(xs[i], ty), Vec2::splat(sizes[i]));
        let resp = ui.interact(r, ui.scope_id().with(("np-t", key)), if live { Sense::click() } else { Sense::hover() });
        let resp = named(resp, name);
        let resp = if live { resp.on_hover_cursor(egui::CursorIcon::PointingHand) } else { resp };
        let h = theme::anim_bool(ui.ctx(), resp.id.with("h"), live && resp.hovered(), 0.1);
        let p = theme::anim_bool(ui.ctx(), resp.id.with("p"), live && resp.is_pointer_button_down_on(), 0.1);
        if h > 0.0 {
            ui.painter().circle_filled(pos2(xs[i], ty), sizes[i] / 2.0 * (1.0 - 0.1 * p), Color32::from_white_alpha((0.1 * h * 255.0) as u8));
        }
        (resp, h, p)
    };
    let (r, h, p) = btn(ui, 0, "shuffle", if app.queue.shuffled() { "Restore original order" } else { "Shuffle up next" }, true);
    // On while shuffled, like the repeat button's, so the toggle has a visible state.
    super::glyphs::draw(ui, r.rect.center(), icons[0] * (1.0 - 0.1 * p), super::glyphs::SHUFFLE, if app.queue.shuffled() { app.accent_color() } else { ink(h, 0.55, 1.0) });
    if r.clicked() {
        app.shuffle_toggle();
    }
    let (r, h, p) = btn(ui, 1, "prev", "Previous", enabled);
    super::glyphs::draw(ui, r.rect.center(), icons[1] * (1.0 - 0.1 * p), super::glyphs::PREVIOUS, ink(h, 1.0, dim));
    if r.clicked() && enabled {
        app.prev();
    }
    let (r, h, p) = btn(ui, 2, "play", if app.playing() || app.buffering { "Pause" } else { "Play" }, enabled);
    let c = r.rect.center();
    let size = icons[2] * (1.0 - 0.1 * p);
    let mark = ink(h, 1.0, dim);
    let morph = theme::anim_bool(ui.ctx(), "np-play-morph", app.playing() || app.buffering, 0.18);
    if app.buffering {
        let a = now as f32 * 6.0;
        let pts: Vec<egui::Pos2> = (0..16).map(|i| c + vec2((a + i as f32 * 0.24).cos(), (a + i as f32 * 0.24).sin()) * 11.0).collect();
        ui.painter().add(egui::Shape::Path(egui::epaint::PathShape::line(pts, egui::epaint::PathStroke::new(2.4, mark))));
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
    } else {
        // The two marks cross-fade: the reference keeps its pause up through a buffer,
        // and a hard cut between them flickers.
        if morph < 1.0 {
            super::glyphs::draw(ui, c, size, super::glyphs::PLAY, theme::with_alpha(mark, 1.0 - morph));
        }
        if morph > 0.0 {
            super::glyphs::draw(ui, c, size, super::glyphs::PAUSE, theme::with_alpha(mark, morph));
        }
    }
    if r.clicked() && enabled {
        app.toggle();
    }
    let (r, h, p) = btn(ui, 3, "next", "Next", enabled);
    super::glyphs::draw(ui, r.rect.center(), icons[3] * (1.0 - 0.1 * p), super::glyphs::NEXT, ink(h, 1.0, dim));
    if r.clicked() && enabled {
        app.next();
    }
    let (glyph, on) = match app.queue.repeat {
        Repeat::Off => (super::glyphs::REPEAT, false),
        Repeat::All => (super::glyphs::REPEAT, true),
        Repeat::One => (super::glyphs::REPEAT_ONE, true),
    };
    let (r, h, p) = btn(ui, 4, "repeat", "Repeat", true);
    super::glyphs::draw(ui, r.rect.center(), icons[4] * (1.0 - 0.1 * p), glyph, if on { app.accent_color() } else { ink(h, 0.55, 1.0) });
    if r.clicked() {
        app.queue.repeat = match app.queue.repeat {
            Repeat::Off => Repeat::All,
            Repeat::All => Repeat::One,
            Repeat::One => Repeat::Off,
        };
    }

    if no_lyrics && app.np_tab == NpTab::Lyrics {
        // A brief note, faded out after a few seconds.
        let since = ui.ctx().memory_mut(|m| *m.data.get_temp_mut_or_insert_with(egui::Id::unique(("no-lyrics", &track.id)), || now));
        let a = (1.0 - ((now - since - 3.0) / 1.0) as f32).clamp(0.0, 1.0);
        if a > 0.0 {
            let c = theme::with_alpha(theme::text_dim(), a * 0.8);
            text_at(ui, pos2(x0, ty + 40.0), "No lyrics for this song", theme::regular(14.0), c, w);
            // Frames only for the fade-out.
            let wait = (since + 3.0 - now).max(0.0);
            ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(if wait > 0.0 { wait } else { 0.033 }));
        }
    }
}

// ------------------------------------------------------------------ lyrics

fn lyrics_panel(app: &mut App, ui: &mut Ui, rect: Rect, now: f64, centered: bool) {
    // Lent out for the frame rather than cloned (the drawing needs `app` mutably).
    let Some((id, set)) = app.lyrics.take() else {
        ui.add_space(rect.height() * 0.35);
        ui.vertical_centered(|ui| {
            ui.spinner();
            ui.label(egui::RichText::new("Loading lyrics…").color(theme::text_dim()));
        });
        return;
    };
    if let Some(lyrics) = app.lyric_prefs.choose(&id, &set).and_then(|p| set.get(p)) {
        let panel = Rect::from_min_max(rect.min, pos2(rect.right(), rect.bottom() - 4.0));
        if lyrics.sync == Sync::None {
            plain_lyrics(app, ui, panel, lyrics);
        } else {
            synced(app, ui, panel, &id, lyrics, now, centered);
        }
    } else {
        ui.add_space(rect.height() * 0.4);
        ui.vertical_centered(|ui| {
            ui.label(egui::RichText::new("No lyrics found for this song").font(theme::semibold(20.0)).color(theme::text_dim()));
        });
    }
    app.lyrics = Some((id, set));
}

/// Romanization and translation for each line, the way the extension picks them: what
/// the lyrics ship (Apple-style TTML) first, then Unison/Google.
fn decorations<'a>(
    show_roman: bool,
    show_trans: bool,
    target: &'a str,
    extras: Option<&'a (String, String, String, crate::translate::Extras)>,
    id: &str,
    lyrics: &'a Lyrics,
) -> (Vec<Option<karaoke::Roman<'a>>>, Vec<Option<&'a str>>) {
    let extras = extras.filter(|(v, s, t, _)| v == id && s == lyrics.provider.key() && t == target).map(|(_, _, _, e)| e);
    let same = |a: &str, b: &str| a.trim().eq_ignore_ascii_case(b.trim());
    // A blank romanization would draw an empty pill box with nothing inside it.
    let worth = |r: &str, l: &crate::lyrics::Line| !r.trim().is_empty() && !same(r, &l.text);
    let roman = lyrics
        .lines
        .iter()
        .enumerate()
        .map(|(i, l)| {
            if !show_roman || l.instrumental {
                return None;
            }
            if let Some(r) = l.romanization.as_deref().filter(|r| worth(r, l)) {
                return Some(if l.timed_roman.is_empty() { karaoke::Roman::Text(r) } else { karaoke::Roman::Timed(&l.timed_roman, r) });
            }
            let r = extras?.roman.get(i)?.as_deref()?;
            worth(r, l).then_some(karaoke::Roman::Text(r))
        })
        .collect();
    let trans = lyrics
        .lines
        .iter()
        .enumerate()
        .map(|(i, l)| {
            if !show_trans || l.instrumental {
                return None;
            }
            let t = super::lyric_prefs::shipped_translation(l, target).or_else(|| extras?.trans.get(i)?.as_deref())?;
            (!same(t, &l.text)).then_some(t)
        })
        .collect();
    (roman, trans)
}

fn synced(app: &mut App, ui: &mut Ui, rect: Rect, id: &str, lyrics: &Lyrics, now: f64, narrow: bool) {
    // Settings › Appearance: the theme's lyrics with the blur and glow off, built once
    // per theme and kept on the App — the karaoke view caches its layout by the style's
    // address, so a fresh copy each frame would relayout every frame.
    if !app.lyric_fx && app.plain_style.as_ref().is_none_or(|(theme, _)| *theme != app.look().id) {
        let look = app.look();
        let mut plain = look.lyrics.clone();
        plain.blur = 0.0;
        plain.glow = 0.0;
        if let Some(css) = &mut plain.css {
            css.past = css.past.map(|[opacity, _]| [opacity, 0.0]);
            css.upcoming = css.upcoming.map(|[opacity, _]| [opacity, 0.0]);
        }
        app.plain_style = Some((look.id.clone(), plain));
    }
    let style = if app.lyric_fx {
        &app.look().lyrics
    } else {
        &app.plain_style.as_ref().expect("just built").1
    };
    let accent = app.accent_color();
    let centered = narrow || style.align == crate::preset::Align::Center;
    let playing = if app.mix_hold().is_some() { app.audio.status.outgoing_playing() } else { app.playing() };
    // Audible position: the device plays what the player hands it a buffer later. A
    // handoff's first half is still the outgoing song's tail, which lines up with it.
    let pos = match app.mix_hold() {
        Some(h) => app.mix_hold_pos(h) as f64 - app.audio.status.latency().as_secs_f64(),
        None => app.audio.status.position_smooth().as_secs_f64() - app.audio.status.latency().as_secs_f64(),
    };
    let pos = app.lyric_clock.tick(pos, playing, now, app.audio.status.speed() as f64);
    let t = pos - app.lyric_prefs.offset(id, lyrics.provider, lyrics.sync == Sync::Word);
    let (roman, trans) = decorations(
        app.show_roman,
        app.show_trans,
        &app.lyric_prefs.target,
        app.lyrics_extras.as_deref(),
        id,
        lyrics,
    );
    // The decoration strings only change when the song, its source, the theme or the
    // romanization/translation settings do; hashing all of them every frame of the
    // sweep was O(the lyrics' whole text). The identity is pointers and short strings.
    let identity = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        std::ptr::from_ref(lyrics).hash(&mut h);
        id.hash(&mut h);
        lyrics.provider.key().hash(&mut h);
        app.theme_id.hash(&mut h);
        app.show_roman.hash(&mut h);
        app.show_trans.hash(&mut h);
        app.lyric_prefs.target.hash(&mut h);
        app.lyrics_extras
            .as_ref()
            .map(std::sync::Arc::as_ptr)
            .hash(&mut h);
        h.finish()
    };
    let key = match app.decorations_key {
        Some((seen, key)) if seen == identity => key,
        _ => {
            let key = {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                id.hash(&mut h);
                lyrics.provider.key().hash(&mut h);
                app.theme_id.hash(&mut h);
                for r in &roman {
                    match r {
                        Some(karaoke::Roman::Timed(_, s)) | Some(karaoke::Roman::Text(s)) => s.hash(&mut h),
                        None => 0u8.hash(&mut h),
                    }
                }
                trans.hash(&mut h);
                h.finish()
            };
            app.decorations_key = Some((identity, key));
            key
        }
    };
    let scale = app.lyric_prefs.text_scale;
    let glow_boost = if app.lyric_prefs.glow_boost { app.lyric_prefs.glow_strength } else { 0.0 };
    let frame = karaoke::Frame { lyrics, key, roman, trans, t, playing, now, rect, style, accent, centered, scale, glow_boost, light: app.lyric_light };
    // The view lives in `app`; the frame borrows `app` for decorations, so swap it out.
    let mut view = std::mem::take(&mut app.lyric_view);
    let out = if karaoke::dev_off("view") { karaoke::Out { seek: None, animating: true, user_scrolling: false } } else { view.show(ui, frame) };
    app.lyric_view = view;
    if let Some(s) = out.seek {
        app.actions.push(Action::Seek(s as f32));
    }
    if out.animating {
        if app.lyrics_full_rate {
            // As fast as the screen shows them: vsync, or `pace::frame`, sets the rate.
            ui.ctx().request_repaint();
        } else {
            // Smooth, without following a fast monitor: frame rate is the main CPU and GPU cost.
            super::pace::repaint_after(ui.ctx(), std::time::Duration::from_millis(16));
        }
    }
    // The extension's `.autoscroll-resume-button` (YouTube Music's rem is 10 px): a frosted
    // pill under the top edge in the lyrics font at 600, 1.75rem; padding .5rem 1rem;
    // white 5% fill, 1px white 10% border, radius 1rem, over a 5px backdrop blur (which
    // reads as about twice those on screen). It slides down from -3em to 2em as it fades
    // in, and its text glows on hover.
    let shown = theme::anim_bool(ui.ctx(), "lyr-resume", out.user_scrolling, 0.2);
    if shown > 0.0 {
        let font = theme::lyric_font(&style.font, crate::preset::Weight::Semibold, 17.5);
        let g = widgets::galley(ui, "Resume Autoscroll", font, Color32::WHITE, rect.width() - 40.0);
        let size = g.size() + vec2(20.0, 10.0) + Vec2::splat(2.0);
        let em = 17.5;
        let y = rect.top() + 8.0 + egui::lerp(-3.0 * em..=0.5 * em, theme::ease_out(shown));
        let r = Rect::from_min_size(pos2(rect.center().x - size.x / 2.0, y), size);
        let resp = ui.interact(r, ui.scope_id().with("lyr-resume-btn"), if out.user_scrolling { Sense::click() } else { Sense::hover() });
        let resp = named(resp, "Resume Autoscroll").on_hover_cursor(egui::CursorIcon::PointingHand);
        let hover = theme::anim_bool(ui.ctx(), "lyr-resume-h", resp.hovered() && out.user_scrolling, 0.25);
        let p = ui.painter().with_clip_rect(rect.expand2(vec2(0.0, 4.0)));
        p.rect_filled(r, 10.0, theme::with_alpha(Color32::WHITE, 0.10 * shown));
        p.rect_stroke(r, 10.0, egui::Stroke::new(1.0, theme::with_alpha(Color32::WHITE, 0.2 * shown)), egui::StrokeKind::Inside);
        let at = r.center() - g.size() / 2.0;
        if hover > 0.0
            && let Some((tex, glow_rect, uv)) = app.lyric_view.text_glow(ui.ctx(), g.clone(), at, 15.0)
        {
            // `text-shadow: 0 0 1.5rem var(--blyrics-glow-color)`: a real blur of the letters.
            let glow = crate::preset::paint(&style.glow_color, accent);
            p.image(tex, glow_rect, uv, theme::with_alpha(glow, hover * shown));
        }
        p.galley_with_override_text_color(at, g, theme::with_alpha(Color32::WHITE, shown));
        if shown < 1.0 {
            ui.ctx().request_repaint();
        }
        if resp.clicked() {
            app.lyric_view.resume();
        }
    }
}

fn plain_lyrics(app: &mut App, ui: &mut Ui, rect: Rect, lyrics: &Lyrics) {
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    egui::ScrollArea::vertical().id_salt("plain-lyrics").auto_shrink(false).show(&mut child, |ui| {
        ui.label(egui::RichText::new("These lyrics aren't synced").font(theme::regular(13.0)).color(theme::text_faint()));
        ui.add_space(14.0);
        let style = &app.look().lyrics;
        let size = (32.0 * style.size.clamp(0.6, 1.6)).min(rect.width() / 12.5).max(18.0);
        for l in &lyrics.lines {
            let font = theme::lyric_font_for(&style.font, style.weight, size, &l.text);
            let color = theme::with_alpha(Color32::WHITE, 0.85);
            if crate::bidi::has_rtl(&l.text) {
                // Wrapped in reading order, each row in display order; RTL lines start on the right.
                let width = ui.available_width();
                let g = super::widgets::galley_wrapped(ui, &l.text, font, color, width, usize::MAX);
                let (r, _) = ui.allocate_exact_size(vec2(width, g.size().y), Sense::hover());
                let x = if crate::bidi::rtl_base(&l.text) { r.right() - g.size().x } else { r.left() };
                ui.painter().galley(pos2(x, r.top()), g, color);
            } else {
                ui.label(egui::RichText::new(&l.text).font(font).color(color));
            }
            ui.add_space(size * 0.25);
        }
        ui.add_space(80.0);
    });
}

/// Sync-type colours from the extension's dock (syllable, word, line, unsynced).
fn sync_color(k: SyncKind) -> Color32 {
    match k {
        SyncKind::Syllable => Color32::from_rgb(196, 160, 255),
        SyncKind::Word => Color32::from_rgb(130, 200, 255),
        SyncKind::Line => Color32::from_rgb(255, 205, 110),
        SyncKind::Unsynced => Color32::from_rgb(170, 170, 170),
    }
}

fn sync_label(k: SyncKind) -> &'static str {
    match k {
        SyncKind::Syllable => "Syllable synced",
        SyncKind::Word => "Word synced",
        SyncKind::Line => "Line synced",
        SyncKind::Unsynced => "Not synced",
    }
}

/// The ⋯ menu: the song's actions, and with lyrics showing, the source picker,
/// translation and romanization toggles, and this song's timing offset above them.
fn options_menu(app: &mut App, ui: &mut Ui, rect: Rect, track: &crate::model::Track, lyrics: bool) {
    let resp = ui.interact(rect, ui.scope_id().with("np-options"), Sense::click());
    let resp = named(resp, "Options").on_hover_cursor(egui::CursorIcon::PointingHand);
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&resp));
    let h = theme::anim_bool(ui.ctx(), resp.id.with("h"), resp.hovered() || open, 0.12);
    ui.painter().circle_filled(rect.center(), 17.0, theme::glass(0.09 + 0.07 * h));
    for dx in [-6.0, 0.0, 6.0] {
        ui.painter().circle_filled(rect.center() + vec2(dx, 0.0), 1.9, theme::with_alpha(Color32::WHITE, 0.75 + 0.25 * h));
    }
    egui::Popup::menu(&resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        ui.set_width(270.0);
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        ui.add_space(4.0);
        // Lyrics first: the source, translation and timing are what the menu is for
        // while the lyrics are up. Collapsed under a header by default, since these
        // settings only matter while the lyrics are up and the menu is tight.
        if lyrics {
            let key = ui.id().with("np-lyrics-expanded");
            let mut expanded: bool = ui.data(|d| d.get_temp::<bool>(key)).unwrap_or(false);
            if lyrics_options_header(ui, expanded) {
                expanded = !expanded;
                ui.data_mut(|d| d.insert_temp(key, expanded));
            }
            if expanded {
                lyric_options(app, ui);
            }
            menu_divider(ui);
        }
        if app.shown_video().is_some() {
            quality_rows(app, ui);
            menu_divider(ui);
        }
        // No Like here: the star under the cover does the liking, and this menu
        // keeps to what a playing song's ⋯ is for.
        widgets::song_actions(ui, track, false, None, &mut app.actions, false);
        menu_divider(ui);
        // No "Play next" / "Add to queue" here: the song is already playing, and the
        // loop button on the player bar is the right tool for what comes next. Each
        // panel hides the way comments does — back to the artwork, the same trip the
        // L/Q keys make — and its glyph is the one the player bar's button carries.
        let lyrics = app.np_tab == NpTab::Lyrics;
        if widgets::menu_row(ui, if lyrics { "Hide lyrics" } else { "View lyrics" }, |ui, at, color| glyphs::draw(ui, at, 18.0, glyphs::LYRICS, color)) {
            app.np_tab = if lyrics { NpTab::Art } else { NpTab::Lyrics };
        }
        let queue = app.np_tab == NpTab::UpNext;
        if widgets::menu_row(ui, if queue { "Hide queue" } else { "View queue" }, |ui, at, color| glyphs::draw(ui, at, 18.0, glyphs::QUEUE, color)) {
            app.np_tab = if queue { NpTab::Art } else { NpTab::UpNext };
        }
        if app.show_comments {
            let tab = app.np_tab == NpTab::Comments;
            if widgets::menu_action(ui, Some(icon::USERS), if tab { "Hide comments" } else { "View comments" }) {
                app.np_tab = if tab { NpTab::Art } else { NpTab::Comments };
            }
        }
        if widgets::menu_action(ui, Some(icon::RADIO), "Start radio") {
            app.actions.push(Action::Radio(track.clone()));
        }
        // Quality toggle at the bottom: revert when hi-res is playing, upgrade when not.
        let hires_playing = app.audio_info.as_ref().is_some_and(|(_, i)| i.lossless() || i.atmos);
        // The upgrade button shows whenever an upgrade could come from somewhere —
        // any of the configured music sources, not just the custom addon URLs, since
        // Monochrome servers, Monochrome's built-in streamer, and JioSaavn all swap
        // a YouTube stream in place the same way (see `try_upgrade_hires`).
        let addons_ready = app.addons_on && app.hires_sources.iter().enumerate().any(|(i, s)| {
            !s.trim().is_empty() && app.hires_enabled.get(i).copied().unwrap_or(true)
        });
        let any_upgrade_source = addons_ready || app.mono_servers_on || app.monochrome || app.jiosaavn;
        if hires_playing || any_upgrade_source {
            menu_divider(ui);
            if hires_playing {
                if widgets::menu_action(ui, Some(icon::MUSIC), "Revert quality") {
                    app.actions.push(Action::RevertQuality);
                }
            }
            // Upgrade stays visible even on a hi-res track: the listener may want to try
            // another source. Before, `Upgrade quality` was the else-branch of `hires_playing`,
            // so any song that already played in hi-res looked as if the option had gone
            // missing from the menu.
            if any_upgrade_source && widgets::menu_action(ui, Some(icon::MUSIC), "Upgrade quality") {
                app.actions.push(Action::UpgradeQuality);
            }
        }
        ui.add_space(4.0);
    });
}

/// The header row that opens [`lyric_options`]. Collapsed by default to save the
/// menu's vertical space — the source picker, translation toggle, romanization toggle
/// and timing nudge only matter while the lyrics are up, so they don't need to be on
/// the menu's first screen. Doesn't close the popup on click: expanding is a state
/// change, not an action, and the listener should see the rows appear in place.
fn lyrics_options_header(ui: &mut Ui, expanded: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click());
    let resp = named(resp, "Lyrics options").on_hover_cursor(egui::CursorIcon::PointingHand);
    let h = theme::anim_bool(ui.ctx(), resp.id.with("h"), resp.hovered(), 0.12);
    if h > 0.0 {
        ui.painter().rect_filled(rect, 7.0, theme::glass(0.12 * h));
    }
    let color = if resp.hovered() { theme::text() } else { theme::text_dim() };
    widgets::icon_at(ui, pos2(rect.left() + 19.0, rect.center().y), icon::LIST_MUSIC, 16.0, color);
    ui.painter().text(pos2(rect.left() + 40.0, rect.center().y), egui::Align2::LEFT_CENTER, "Lyrics options", theme::regular(14.0), theme::text());
    let chevron = if expanded { icon::CHEVRON_DOWN } else { icon::CHEVRON_RIGHT };
    widgets::icon_at(ui, pos2(rect.right() - 19.0, rect.center().y), chevron, 16.0, color);
    resp.clicked()
}

fn lyric_options(app: &mut App, ui: &mut Ui) {
    let Some((id, set)) = app.lyrics.as_ref() else { return };
    let id = id.clone();
    let available = app.lyric_prefs.available(set);
    let Some(shown) = app.lyric_prefs.choose(&id, set) else { return };
    let n = available.len();
    let mut pick = None;
    menu_caption(ui, if n > 1 { "LYRICS SOURCE" } else { "LYRICS SOURCE · ONLY ONE FOUND" });
    for p in &available {
        if source_row(ui, p.name(), p.sync(), *p == shown) {
            pick = Some(*p);
        }
    }
    menu_divider(ui);
    let tip = format!("Translation · {}", lang_name(&app.lyric_prefs.target));
    switch_row(ui, &tip, &mut app.show_trans);
    switch_row(ui, "Romanization", &mut app.show_roman);
    // This song's timing for the shown source: ‹ +0.0s ›
    let off = app.lyric_prefs.song_offset(&id, shown);
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
    ui.painter().text(pos2(row.left() + 12.0, row.center().y), egui::Align2::LEFT_CENTER, "Timing", theme::regular(14.0), theme::text());
    let mut sub = ui.new_child(egui::UiBuilder::new().max_rect(row.shrink2(vec2(8.0, 0.0))).layout(egui::Layout::right_to_left(egui::Align::Center)));
    sub.spacing_mut().item_spacing.x = 2.0;
    if mini_icon(&mut sub, icon::CHEVRON_RIGHT, "Lyrics later (+0.1 s)", true).clicked() {
        app.lyric_prefs.set_song_offset(&id, shown, off + 0.1);
    }
    let label = format!("{off:+.1}s").replace("+0.0", "0.0");
    if mini_text(&mut sub, &label, false, "This song's lyrics timing. Double-click to reset.").double_clicked() {
        app.lyric_prefs.set_song_offset(&id, shown, 0.0);
    }
    if mini_icon(&mut sub, icon::CHEVRON_LEFT, "Lyrics earlier (−0.1 s)", true).clicked() {
        app.lyric_prefs.set_song_offset(&id, shown, off - 0.1);
    }
    if let Some(p) = pick.filter(|p| *p != shown) {
        app.lyric_prefs.pick(&id, set, p);
        log::info!("lyrics: showing {} ({}/{}) by choice", p.key(), available.iter().position(|x| *x == p).map_or(0, |i| i + 1), n);
        app.lyric_view.reset();
    }
}

fn menu_caption(ui: &mut Ui, text: &str) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
    ui.painter().text(pos2(r.left() + 12.0, r.center().y + 2.0), egui::Align2::LEFT_CENTER, text, theme::semibold(11.0), theme::text_faint());
}

fn menu_divider(ui: &mut Ui) {
    ui.add_space(5.0);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(r.shrink2(vec2(8.0, 0.0)), 0.0, theme::glass(0.08));
    ui.add_space(5.0);
}

// ------------------------------------------------------------------ audio quality

fn badge_font(small: bool) -> egui::FontId {
    theme::semibold(if small { 9.0 } else { 11.0 })
}

fn glyph_side(small: bool) -> f32 {
    if small { 11.0 } else { 13.0 }
}

/// Whether the mark leads with an icon: Apple's lossless wave, or Dolby's double-D.
fn has_quality_glyph(info: &crate::hires::Info) -> bool {
    info.atmos || info.lossless()
}

/// Apple's lossless wave, and Dolby's own Atmos lockup from its newsroom asset page.
fn quality_icon(atmos: bool) -> &'static [u8] {
    if atmos {
        include_bytes!("../../assets/quality/atmos.png")
    } else {
        include_bytes!("../../assets/quality/lossless.png")
    }
}

fn quality_icon_aspect(atmos: bool) -> f32 {
    if atmos { 665.0 / 95.0 } else { 86.0 / 60.0 }
}

/// Just the glyph: the double-D cropped off the lockup's left, for the mini player and
/// the mixing badge where the words come from elsewhere.
fn quality_mark_aspect(atmos: bool) -> f32 {
    if atmos { 106.0 / 95.0 } else { 86.0 / 60.0 }
}

/// The icon as white pixels with its own alpha, so it takes the badge's tint.
fn quality_icon_texture(ui: &Ui, atmos: bool, mark: bool) -> egui::TextureHandle {
    let ctx = ui.ctx();
    let id = egui::Id::new(("quality-icon", atmos, mark));
    if let Some(tex) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return tex;
    }
    let rgba = image::load_from_memory(quality_icon(atmos)).expect("quality icon").to_rgba8();
    let rgba = if mark && atmos {
        image::imageops::crop_imm(&rgba, 0, 0, 106, rgba.height()).to_image()
    } else {
        rgba
    };
    let mut img = egui::ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw());
    for p in &mut img.pixels {
        *p = Color32::from_white_alpha(p.a());
    }
    let tex = ctx.load_texture(if atmos { "dolby-atmos" } else { "apple-lossless" }, img, egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, tex.clone()));
    tex
}

fn paint_quality_icon(ui: &Ui, center: egui::Pos2, height: f32, atmos: bool, tint: Color32, mark: bool) {
    let w = height * if mark { quality_mark_aspect(atmos) } else { quality_icon_aspect(atmos) };
    let rect = Rect::from_center_size(center, vec2(w, height));
    let uv = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    ui.painter().image(quality_icon_texture(ui, atmos, mark).id(), rect, uv, tint);
}

/// How wide the quality mark is, so a line can leave room for it.
pub(super) fn audio_badge_width(ui: &Ui, app: &App, small: bool) -> f32 {
    let Some((_, info)) = app.audio_info.as_ref() else {
        if app.upgrading_quality {
            let label = "Upgrading quality";
            let text = widgets::galley(ui, label, badge_font(small), Color32::WHITE, 200.0).size().x;
            return text + glyph_side(small) + 5.0;
        }
        return 0.0;
    };
    let gh = glyph_side(small);
    if info.atmos && !app.mixing() {
        return gh * quality_icon_aspect(true);
    }
    let label = if app.mixing() { "Mixing" } else { info.badge() };
    let text = widgets::galley(ui, label, badge_font(small), Color32::WHITE, 200.0).size().x;
    if has_quality_glyph(info) { text + gh * quality_mark_aspect(info.atmos) + 5.0 } else { text }
}

/// The quality mark — a glyph and the stream's name, tinted with the theme — drawn at
/// `at`; a click opens its details. Returns how much room it took.
pub(super) fn audio_badge(app: &App, ui: &mut Ui, at: egui::Pos2, small: bool) -> f32 {
    if app.audio_info.is_none() && app.upgrading_quality {
        // A background upgrade at work: the wave pulses while the addons are asked.
        let font = badge_font(small);
        let gh = glyph_side(small);
        let now = ui.input(|i| i.time);
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
        let pulse = ((now * 3.5).sin() * 0.5 + 0.5) as f32;
        let alpha = 0.35 + 0.65 * pulse;
        let accent = app.accent_color();
        let ink = theme::with_alpha(theme::lerp_color(accent, Color32::WHITE, 0.20), alpha);
        let mark = theme::with_alpha(accent, alpha);
        let g = widgets::galley(ui, "Upgrading quality", font, ink, 200.0);
        let w = audio_badge_width(ui, app, small);
        let h = g.size().y.max(gh);
        let rect = Rect::from_min_size(at, vec2(w, h));
        let resp = ui.interact(rect, ui.scope_id().with("audio-badge-upgrading"), Sense::hover());
        named(resp, "Audio quality").on_hover_text("Checking hi-res addons and upgrading audio quality...");
        let mut x = at.x;
        super::glyphs::draw(ui, pos2(x + gh / 2.0, at.y + h / 2.0), gh, super::glyphs::WAVE, mark);
        x += gh + 5.0;
        ui.painter().galley(pos2(x, at.y + (h - g.size().y) / 2.0), g, ink);
        return w;
    }
    let Some((_, info)) = app.audio_info.as_ref() else { return 0.0 };
    let font = badge_font(small);
    // A mix in progress wears its own name over the quality mark; the details keep the truth.
    let g = widgets::galley(ui, if app.mixing() { "Mixing" } else { info.badge() }, font, Color32::WHITE, 200.0);
    let gh = glyph_side(small);
    // Dolby's lockup carries its own words, so it stands alone instead of beside them.
    let lockup = info.atmos && !app.mixing();
    let glyph = has_quality_glyph(info) && !lockup;
    let w = audio_badge_width(ui, app, small);
    let h = if lockup { gh } else { g.size().y.max(if glyph { gh } else { 0.0 }) };
    let rect = Rect::from_min_size(at, vec2(w, h));
    let resp = ui.interact(rect, ui.scope_id().with("audio-badge"), Sense::click());
    let resp = named(resp, "Audio quality").on_hover_cursor(egui::CursorIcon::PointingHand);
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&resp));
    let hover = theme::anim_bool(ui.ctx(), resp.id.with("h"), resp.hovered() || open, 0.12);
    // Lossless and Atmos wear the song's colour; anything lossy stays quiet.
    let (ink, tint) = if info.lossless() || info.atmos {
        let accent = app.accent_color();
        (theme::lerp_color(accent, Color32::WHITE, 0.10 + 0.30 * hover), theme::with_alpha(accent, 0.85 + 0.15 * hover))
    } else {
        (theme::with_alpha(theme::text_dim(), 0.85 + 0.15 * hover), theme::text_dim())
    };
    if lockup {
        paint_quality_icon(ui, rect.center(), gh, true, tint, false);
    } else {
        let mut x = at.x;
        if glyph {
            let icon_w = gh * quality_mark_aspect(info.atmos);
            paint_quality_icon(ui, pos2(x + icon_w / 2.0, at.y + h / 2.0), gh, info.atmos, tint, true);
            x += icon_w + 5.0;
        }
        ui.painter().galley(pos2(x, at.y + (h - g.size().y) / 2.0), g, ink);
    }
    egui::Popup::menu(&resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| audio_info_menu(ui, info));
    w
}

/// Just the quality glyph, centred on `center`, for the mini player's artist line.
pub(super) fn audio_badge_icon_width(app: &App, small: bool) -> f32 {
    if app.audio_info.is_none() && app.upgrading_quality {
        return glyph_side(small);
    }
    let Some((_, info)) = app.audio_info.as_ref().filter(|(_, i)| has_quality_glyph(i)) else { return 0.0 };
    glyph_side(small) * quality_mark_aspect(info.atmos)
}

pub(super) fn audio_badge_icon(app: &App, ui: &mut Ui, center: egui::Pos2, small: bool) {
    if app.audio_info.is_none() && app.upgrading_quality {
        let gh = glyph_side(small);
        let now = ui.input(|i| i.time);
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
        let pulse = ((now * 3.5).sin() * 0.5 + 0.5) as f32;
        let alpha = 0.35 + 0.65 * pulse;
        let accent = app.accent_color();
        let mark = theme::with_alpha(accent, alpha);
        super::glyphs::draw(ui, center, gh, super::glyphs::WAVE, mark);
        return;
    }
    let Some((_, info)) = app.audio_info.as_ref().filter(|(_, i)| has_quality_glyph(i)) else { return };
    let gh = glyph_side(small);
    let rect = Rect::from_center_size(center, vec2(gh * quality_mark_aspect(info.atmos), gh));
    let resp = ui.interact(rect, ui.scope_id().with("audio-badge"), Sense::click());
    let resp = named(resp, "Audio quality").on_hover_cursor(egui::CursorIcon::PointingHand);
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&resp));
    let hover = theme::anim_bool(ui.ctx(), resp.id.with("h"), resp.hovered() || open, 0.12);
    let accent = app.accent_color();
    let tint = theme::with_alpha(accent, 0.85 + 0.15 * hover);
    paint_quality_icon(ui, center, gh, info.atmos, tint, true);
    egui::Popup::menu(&resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| audio_info_menu(ui, info));
}

/// What the stream is: its source, format and the numbers under those.
fn audio_info_menu(ui: &mut Ui, info: &crate::hires::Info) {
    ui.set_width(300.0);
    ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
    ui.add_space(4.0);
    menu_caption(ui, "AUDIO QUALITY");
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::hover());
    let g = widgets::galley(ui, info.badge(), theme::semibold(17.0), theme::text(), 200.0);
    let mut x = r.left() + 12.0;
    if info.atmos {
        // The lockup says "Dolby Atmos" itself.
        let ih = 15.0;
        let icon_w = ih * quality_icon_aspect(true);
        paint_quality_icon(ui, pos2(x + icon_w / 2.0, r.center().y), ih, true, theme::text(), false);
    } else {
        if has_quality_glyph(info) {
            let ih = 17.0;
            let icon_w = ih * quality_mark_aspect(false);
            paint_quality_icon(ui, pos2(x + icon_w / 2.0, r.center().y), ih, false, theme::text(), true);
            x += icon_w + 8.0;
        }
        ui.painter().galley(pos2(x, r.center().y - g.size().y / 2.0), g.clone(), theme::text());
    }
    menu_divider(ui);
    info_row(ui, "Source", if info.source.is_empty() { &info.host } else { &info.source });
    if !info.codec.is_empty() {
        info_row(ui, "Codec", &info.codec.to_ascii_uppercase());
    }
    if let Some(kbps) = bitrate(info) {
        info_row(ui, "Bitrate", &format!("{} kbps", thousands(kbps)));
    }
    if let Some(hz) = info.sample_rate {
        info_row(ui, "Sample rate", &rate_text(hz));
    }
    if let Some(bits) = info.bit_depth {
        info_row(ui, "Bit depth", &format!("{bits}-bit"));
    }
    if let Some(n) = info.channels {
        let text = match n {
            1 => "Mono".to_owned(),
            2 => "Stereo".to_owned(),
            _ => format!("{n} channels"),
        };
        info_row(ui, "Channels", &text);
    }
    if info.size > 0 {
        info_row(ui, "Size", &size_text(info.size));
    }
    if info.duration_ms > 0 {
        info_row(ui, "Duration", &super::fmt_time((info.duration_ms / 1000) as u32));
    }
    ui.add_space(6.0);
}

fn info_row(ui: &mut Ui, label: &str, value: &str) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::hover());
    ui.painter().text(pos2(r.left() + 12.0, r.center().y), egui::Align2::LEFT_CENTER, label, theme::regular(13.0), theme::text_dim());
    let g = widgets::galley(ui, value, theme::semibold(13.0), theme::text(), 190.0);
    ui.painter().galley(pos2(r.right() - 12.0 - g.size().x, r.center().y - g.size().y / 2.0), g, theme::text());
}

/// The stated bitrate, or the one the file's size and length imply.
fn bitrate(info: &crate::hires::Info) -> Option<u32> {
    info.bitrate_kbps
        .or_else(|| (info.duration_ms > 0 && info.size > 0).then(|| (info.size * 8 / info.duration_ms) as u32))
        .filter(|k| *k > 0)
}

fn thousands(n: u32) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn rate_text(hz: u32) -> String {
    if hz % 1000 == 0 { format!("{} kHz", hz / 1000) } else { format!("{:.1} kHz", hz as f64 / 1000.0) }
}

fn size_text(bytes: u64) -> String {
    if bytes >= 1024 * 1024 { format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0)) } else { format!("{:.0} KB", bytes as f64 / 1024.0) }
}

fn channels_text(n: u16) -> String {
    match n {
        1 => "Mono".to_owned(),
        2 => "Stereo".to_owned(),
        _ => format!("{n} channels"),
    }
}

/// Stats for nerds, on the foot of the sleeve: what the decoder is being fed, from
/// whichever source. Nothing while the setting is off.
fn nerd_stats(app: &App, ui: &Ui, art: Rect, round: bool) {
    let Some((_, stats)) = app.stream_stats.as_ref().filter(|_| app.show_nerd_stats) else { return };
    let ink = theme::with_alpha(Color32::WHITE, 0.65);
    let g = widgets::galley(ui, &stats_line(stats), theme::semibold(11.0), ink, art.width() - 20.0);
    // A disc narrows toward its foot: the line rises until it fits across.
    let foot = if round {
        let r = art.width() / 2.0;
        let half = (g.size().x / 2.0 + 10.0).min(r);
        art.center().y + (r * r - half * half).sqrt() - 8.0
    } else {
        art.bottom() - 8.0
    };
    let at = pos2(art.center().x - g.size().x / 2.0, foot - g.size().y);
    // Plain white washes out over a pale cover; the shadow under it keeps it legible.
    ui.painter().galley_with_override_text_color(at + vec2(0.0, 1.0), g.clone(), Color32::from_black_alpha(140));
    ui.painter().galley(at, g, ink);
}

/// The stats as one line: `Opus · 48 kHz · 133 kbps · Stereo`.
fn stats_line(s: &crate::stream::Stats) -> String {
    let mut parts = vec![s.codec.clone()];
    if let Some(bits) = s.bit_depth {
        parts.push(format!("{bits}-bit"));
    }
    parts.push(rate_text(s.sample_rate));
    if let Some(kbps) = s.bitrate_kbps {
        parts.push(format!("{} kbps", thousands(kbps)));
    }
    parts.push(channels_text(s.channels));
    parts.join(" · ")
}

/// A lyrics source: its sync type's dot, name and sync type, with a check on the one shown.
fn source_row(ui: &mut Ui, name: &str, sync: SyncKind, current: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::click());
    let resp = named(resp, &format!("Lyrics source {name}")).on_hover_cursor(egui::CursorIcon::PointingHand);
    let h = theme::anim_bool(ui.ctx(), resp.id.with("h"), resp.hovered(), 0.12);
    if h > 0.0 || current {
        ui.painter().rect_filled(rect, 8.0, theme::glass(0.05 * current as u8 as f32 + 0.08 * h));
    }
    ui.painter().circle_filled(pos2(rect.left() + 19.0, rect.center().y), 3.5, sync_color(sync));
    let x = rect.left() + 40.0;
    text_at(ui, pos2(x, rect.top() + 6.0), name, theme::semibold(13.5), theme::text(), rect.right() - x - 34.0);
    text_at(ui, pos2(x, rect.top() + 24.0), sync_label(sync), theme::regular(12.0), theme::text_dim(), rect.right() - x - 34.0);
    if current {
        widgets::icon_at(ui, pos2(rect.right() - 18.0, rect.center().y), icon::CHECK, 16.0, theme::text());
    }
    resp.clicked()
}

/// A label with an on/off switch; the whole row toggles.
fn switch_row(ui: &mut Ui, label: &str, on: &mut bool) {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click());
    let resp = named(resp, label).on_hover_cursor(egui::CursorIcon::PointingHand);
    if resp.clicked() {
        *on = !*on;
    }
    let h = theme::anim_bool(ui.ctx(), resp.id.with("h"), resp.hovered(), 0.12);
    if h > 0.0 {
        ui.painter().rect_filled(rect, 8.0, theme::glass(0.08 * h));
    }
    ui.painter().text(pos2(rect.left() + 12.0, rect.center().y), egui::Align2::LEFT_CENTER, label, theme::regular(14.0), theme::text());
    let t = theme::anim_bool(ui.ctx(), resp.id.with("t"), *on, 0.16);
    let track = Rect::from_center_size(pos2(rect.right() - 30.0, rect.center().y), vec2(36.0, 20.0));
    let accent = ui.ctx().data(|d| d.get_temp::<Color32>(egui::Id::unique("accent"))).unwrap_or(theme::RED);
    ui.painter().rect_filled(track, 10.0, theme::lerp_color(theme::glass(0.16 + 0.06 * h), accent, t));
    let knob = pos2(egui::lerp(track.left() + 10.0..=track.right() - 10.0, t), track.center().y);
    ui.painter().circle_filled(knob, 8.0, Color32::WHITE);
}

/// A small icon button (24 × 24).
fn mini_icon(ui: &mut Ui, glyph: char, tip: &str, enabled: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(22.0, 24.0), if enabled { Sense::click() } else { Sense::hover() });
    let resp = named(resp, tip).on_hover_text(tip);
    let resp = if enabled { resp.on_hover_cursor(egui::CursorIcon::PointingHand) } else { resp };
    let h = theme::anim_bool(ui.ctx(), resp.id.with("h"), resp.hovered() && enabled, 0.12);
    if h > 0.0 {
        ui.painter().rect_filled(rect, 7.0, theme::glass(0.12 * h));
    }
    let a = if enabled { 0.6 + 0.35 * h } else { 0.25 };
    widgets::icon_at(ui, rect.center(), glyph, 14.0, theme::with_alpha(Color32::WHITE, a));
    resp
}

/// A small text toggle or label (24 px tall), highlighted when `on`.
fn mini_text(ui: &mut Ui, text: &str, on: bool, tip: &str) -> egui::Response {
    let font = theme::semibold(12.0);
    let g = ui.fonts_mut(|f| f.layout_no_wrap(text.to_owned(), font, Color32::WHITE));
    let (rect, resp) = ui.allocate_exact_size(vec2(g.size().x + 12.0, 24.0), Sense::click());
    let resp = named(resp, tip).on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand);
    let h = theme::anim_bool(ui.ctx(), resp.id.with("h"), resp.hovered(), 0.12);
    let gold = Color32::from_rgb(255, 214, 120);
    let fill = if on { theme::with_alpha(Color32::from_rgb(255, 196, 64), 0.16 + 0.06 * h) } else { theme::glass(0.12 * h) };
    if fill.a() > 0 {
        ui.painter().rect_filled(rect, 7.0, fill);
    }
    let color = if on { gold } else { theme::with_alpha(Color32::WHITE, 0.6 + 0.35 * h) };
    ui.painter().galley_with_override_text_color(rect.center() - g.size() / 2.0, g, color);
    resp
}

fn lang_name(code: &str) -> &str {
    super::lyric_prefs::LANGUAGES.iter().find(|(c, _)| *c == code).map_or(code, |(_, n)| n)
}
