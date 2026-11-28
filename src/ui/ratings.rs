//! Read the signed-in YouTube Music ratings shown by the player and visible song rows.
//!
//! The fork has no dislikes: what's read and shown here is the like half of YouTube
//! Music's ratings, with the hold-to-unlike gesture that keeps a slip of the mouse from
//! taking a like away.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use egui::{Color32, Context, Id, Key, LayerId, Order, PointerButton, Rect, Response, Ui, pos2, vec2};

use crate::backend::{Backend, Req};
use crate::model::Track;
use crate::theme;
use crate::ytm::SongRating;

use super::App;

const MAX_READS: usize = 3;
const READ_GAP: f64 = 0.2;
// Give the longest account pause time to end before asking again.
const RETRY_DELAY: f64 = crate::ytmapi::MAX_SIGNED_IN_PAUSE.as_secs_f64() + 1.0;
/// Ratings kept for rows that have scrolled away.
const KEEP: usize = 2000;
/// Seconds before coming back to the window asks for the ratings on show again.
const REFRESH_AFTER: f64 = 600.0;
pub(super) const MIN_UNLIKE_MS: f32 = 200.0;
pub(super) const MAX_UNLIKE_MS: f32 = 3000.0;
pub(super) const DEFAULT_UNLIKE_MS: f32 = 600.0;
const UNLIKE_MS: &str = "ratings_unlike_hold_ms";
const UNLIKE_HINT: &str = "Hold to remove like";

pub(super) struct Settings {
    pub player_button: bool,
    pub list_icons: bool,
    pub confirm_unlike: bool,
    pub unlike_hold_ms: f32,
    pub unlike_notices: bool,
}

impl Settings {
    pub fn load(get: impl Fn(&str) -> Option<String>) -> Self {
        let on = |key| get(key).is_none_or(|v| v == "1");
        let number = |key| get(key).and_then(|v| v.parse::<f32>().ok()).filter(|v| v.is_finite());
        let hold = number(UNLIKE_MS).or_else(|| number("ratings_unlike_hold_seconds").map(|s| s * 1000.0));
        Self {
            player_button: on("ratings_player_button"),
            list_icons: on("ratings_list_icons"),
            confirm_unlike: on("ratings_confirm_unlike"),
            unlike_hold_ms: hold.map_or(DEFAULT_UNLIKE_MS, |v| v.round().clamp(MIN_UNLIKE_MS, MAX_UNLIKE_MS)),
            unlike_notices: on("ratings_unlike_notices"),
        }
    }

    pub fn save(&self, storage: &mut dyn eframe::Storage) {
        for (key, on) in [
            ("ratings_player_button", self.player_button),
            ("ratings_list_icons", self.list_icons),
            ("ratings_confirm_unlike", self.confirm_unlike),
            ("ratings_unlike_notices", self.unlike_notices),
        ] {
            storage.set_string(key, if on { "1" } else { "0" }.to_owned());
        }
        storage.set_string(UNLIKE_MS, self.unlike_hold_ms.to_string());
    }

    pub fn share_unlike_settings(&self, ctx: &Context) {
        ctx.data_mut(|d| d.insert_temp(Id::unique("unlike-gesture-settings"), (self.confirm_unlike, f64::from(self.unlike_hold_ms) / 1000.0)));
    }
}

#[derive(Clone, Copy)]
enum HoldPhase {
    Pressed(f64),
    Returning { since: f64, from: f32 },
    Accepted(f64),
}

#[derive(Clone, Copy)]
enum HoldInput {
    Pointer,
    Keyboard,
}

#[derive(Clone, Copy)]
struct UnlikeHold {
    target: Id,
    last_pass: u64,
    input: HoldInput,
    phase: HoldPhase,
    progress: f32,
    hint_until: f64,
}

impl UnlikeHold {
    fn update(&mut self, now: f64, seconds: f64, down: bool, released_here: bool, clicked: bool) -> bool {
        match self.phase {
            HoldPhase::Pressed(start) => {
                let elapsed = ((now - start) / seconds).clamp(0.0, 1.0) as f32;
                if elapsed >= 1.0 && (down || released_here) {
                    self.phase = HoldPhase::Accepted(now);
                    self.progress = 1.0;
                    return true;
                }
                if down {
                    self.progress = elapsed;
                } else {
                    if clicked {
                        self.hint_until = now + 3.0;
                    }
                    if released_here {
                        self.progress = elapsed;
                    }
                    self.phase = HoldPhase::Returning { since: now, from: self.progress };
                }
            }
            HoldPhase::Accepted(_) if !down => {
                self.phase = HoldPhase::Returning { since: now, from: 1.0 };
            }
            HoldPhase::Returning { since, from } => {
                self.progress = from * (1.0 - (now - since).clamp(0.0, 1.0) as f32);
                if clicked {
                    self.hint_until = now + 3.0;
                }
            }
            HoldPhase::Accepted(_) => {}
        }
        false
    }
}

pub(super) struct LikeGesture {
    pub activate: bool,
    pub progress: f32,
    pub hint: bool,
}

// The input owner and target are both part of the identity. A new button or selection
// starts from zero, even when it occupies the same place as the previous one.
// The palette keeps focus in its search field and forwards the initial Enter press.
pub(super) fn like_gesture(
    ui: &Ui,
    response: &Response,
    target: impl std::hash::Hash + std::fmt::Debug,
    liked: bool,
    selected_enter_press: Option<bool>,
) -> LikeGesture {
    let ctx = ui.ctx();
    let (enabled, seconds) = ctx.data(|d| d.get_temp::<(bool, f64)>(Id::unique("unlike-gesture-settings"))).unwrap_or_default();
    let target = response.id.with(target);
    let state = Id::unique("unlike-gesture");
    let pass = ctx.cumulative_pass_nr();
    let (now, mouse_down, mouse_pressed, mouse_released, key_down, key_pressed, key_released) = ui.input(|i| {
        let keys = [Key::Enter, Key::Space];
        let keys = if selected_enter_press.is_some() { &keys[..1] } else { &keys[..] };
        (
            i.time, i.pointer.primary_down(), i.pointer.button_pressed(PointerButton::Primary), i.pointer.button_released(PointerButton::Primary),
            keys.iter().any(|&key| i.key_down(key)),
            selected_enter_press.unwrap_or_else(|| i.events.iter().any(|event| {
                matches!(event, egui::Event::Key { key, pressed: true, repeat: false, .. } if keys.contains(key))
            })),
            keys.iter().any(|&key| i.key_released(key)),
        )
    });
    let focused = response.enabled() && (response.has_focus() || selected_enter_press.is_some());
    let pointer = response.enabled() && response.contains_pointer();
    let start = if pointer && mouse_down && mouse_pressed && response.is_pointer_button_down_on() {
        // The row's drag grip would otherwise take over when egui's ordinary click times out.
        if enabled && liked {
            ctx.stop_dragging();
        }
        Some(HoldInput::Pointer)
    } else if focused && key_down && key_pressed {
        Some(HoldInput::Keyboard)
    } else {
        None
    };
    let input_state = |input| match input {
        HoldInput::Pointer => (pointer && mouse_down, pointer && mouse_released),
        HoldInput::Keyboard => (focused && key_down, focused && key_released),
    };
    let clicked = response.clicked_by(PointerButton::Primary) || (focused && key_released);
    let regular_click = response.clicked();
    let (activate, progress, hint) = ctx.data_mut(|d| {
        let existing = d.get_temp::<UnlikeHold>(state).filter(|hold| hold.target == target && pass - hold.last_pass <= 1);
        if !enabled || !liked {
            // A completed hold already drew the outline. Keep that look while the
            // player's ordinary unlike animation catches up with the new state.
            let completed = existing.filter(|_| enabled && start.is_none());
            let finish_press = completed.is_some_and(|hold| {
                let (down, released) = input_state(hold.input);
                matches!(hold.phase, HoldPhase::Accepted(_)) && (down || released)
            });
            let progress = if finish_press {
                1.0
            } else {
                completed.map_or(0.0, |hold| match hold.phase {
                    HoldPhase::Accepted(at) if now - at < 0.35 => 1.0,
                    HoldPhase::Returning { since, from: 1.0 } if now - since < 0.35 => 1.0,
                    _ => 0.0,
                })
            };
            if progress == 0.0 && existing.is_some() {
                d.remove::<UnlikeHold>(state);
            } else if let Some(mut hold) = existing {
                hold.last_pass = pass;
                d.insert_temp(state, hold);
            }
            return (regular_click && !finish_press, progress, false);
        }
        let mut hold = if let Some(input) = start {
            UnlikeHold { target, last_pass: pass, input, phase: HoldPhase::Pressed(now), progress: 0.0, hint_until: 0.0 }
        } else if let Some(hold) = existing {
            hold
        } else if clicked {
            UnlikeHold { target, last_pass: pass, input: HoldInput::Pointer, phase: HoldPhase::Returning { since: now, from: 0.0 }, progress: 0.0, hint_until: now + 3.0 }
        } else {
            return (false, 0.0, false);
        };
        let (down, released_here) = input_state(hold.input);
        let activate = hold.update(now, seconds, down, released_here, clicked);
        hold.last_pass = pass;
        let hint = now < hold.hint_until;
        let progress = hold.progress;
        if progress != 0.0 || hint || matches!(hold.phase, HoldPhase::Pressed(_)) {
            d.insert_temp(state, hold);
        } else {
            d.remove::<UnlikeHold>(state);
        }
        (activate, progress, hint)
    });
    if start.is_some() || progress > 0.0 || hint || activate {
        ctx.request_repaint_after(Duration::from_millis(16));
    }
    LikeGesture { activate, progress, hint }
}

pub(super) fn unlike_hint(ui: &Ui, response: &Response) {
    let ctx = ui.ctx();
    let layer = LayerId::new(Order::Tooltip, response.id.with("unlike-hint"));
    let painter = ctx.layer_painter(layer);
    let anchor = ctx.layer_transform_to_global(response.layer_id).map_or(response.rect, |transform| transform * response.rect);
    let screen = ctx.content_rect();
    let text = theme::text();
    let g = painter.layout_no_wrap(UNLIKE_HINT.to_owned(), theme::semibold(12.0), text);
    let size = g.size() + vec2(16.0, 8.0);
    let left = (anchor.center().x - size.x / 2.0).min(screen.right() - size.x - 4.0).max(screen.left() + 4.0);
    let top = (anchor.top() - size.y - 4.0).max(screen.top() + 4.0);
    let rect = Rect::from_min_size(pos2(left, top), size);
    painter.rect_filled(rect, 7.0, theme::menu_fill());
    painter.galley(rect.min + vec2(8.0, 4.0), g, text);
    ctx.move_to_top(layer);
}

/// The like glyph drained while a hold takes it away; the fork's like is the star glyph
/// the player bar and now playing draw.
pub(super) fn unlike_star(ui: &Ui, center: egui::Pos2, size: f32, fill: f32, tint: Color32, progress: f32) {
    let gray = theme::with_alpha(theme::text_dim(), 1.0 - progress);
    let tint = theme::lerp_color(tint, gray, progress);
    let fill_tint = theme::with_alpha(tint, (tint.a() as f32 / 255.0) * fill * (1.0 - progress));
    super::glyphs::draw(ui, center, size, super::glyphs::STAR, tint);
    if fill_tint.a() > 0 {
        super::glyphs::draw(ui, center, size, super::glyphs::STAR_FILLED, fill_tint);
    }
}

enum Entry {
    Reading { round: u64 },
    RetryAt(f64),
    Ready(Option<SongRating>),
    Confirmed(SongRating),
}

/// Answers are kept after their rows scroll away, so scrolling back doesn't ask again,
/// up to [`KEEP`] of them. Requests for scrolled-away tracks still count toward the limit
/// until they finish.
#[derive(Default)]
pub(super) struct Ratings {
    entries: HashMap<String, Entry>,
    in_flight: HashSet<u64>,
    pending_writes: HashMap<String, bool>,
    round: u64,
    next_read: f64,
    playing: Option<(String, u64)>,
    refreshed_at: f64,
}

impl Ratings {
    pub fn reset(&mut self) {
        self.entries.clear();
        self.in_flight.clear();
        self.pending_writes.clear();
        self.next_read = 0.0;
        self.playing = None;
    }

    /// Back in the window: ratings changed on another device show, but asked for again
    /// at most every [`REFRESH_AFTER`], not on every switch back.
    pub fn refresh_if_stale(&mut self, now: f64) {
        if now - self.refreshed_at >= REFRESH_AFTER {
            self.refresh();
            self.refreshed_at = now;
        }
    }

    pub fn writing(&self, id: &str) -> bool {
        self.pending_writes.contains_key(id)
    }

    pub fn start_write(&mut self, id: &str, like: bool) {
        self.pending_writes.insert(id.to_owned(), like);
    }

    pub fn finish_write(&mut self, id: &str, like: bool, ok: bool) {
        if self.pending_writes.get(id) != Some(&like) {
            return;
        }
        self.pending_writes.remove(id);
        if ok {
            self.confirm(id, if like { SongRating::Like } else { SongRating::Indifferent });
        } else if !like && self.get(id).is_none() {
            // Keep an account rating we already know. Otherwise restore the like
            // that Liked Music showed before the edit, while the list reloads.
            self.confirm(id, SongRating::Like);
        }
    }

    pub fn confirm(&mut self, id: &str, rating: SongRating) {
        // An older read can still finish, but its round no longer matches this entry.
        self.entries.insert(id.to_owned(), Entry::Confirmed(rating));
    }

    pub fn refresh(&mut self) {
        self.entries.clear();
    }

    pub fn get(&self, id: &str) -> Option<SongRating> {
        if let Some(&like) = self.pending_writes.get(id) {
            return Some(if like { SongRating::Like } else { SongRating::Indifferent });
        }
        match self.entries.get(id) {
            Some(Entry::Ready(rating)) => *rating,
            Some(Entry::Confirmed(rating)) => Some(*rating),
            _ => None,
        }
    }

    pub fn finish(&mut self, video_id: String, round: u64, rating: Result<Option<SongRating>, ()>, now: f64) {
        self.in_flight.remove(&round);
        if let Some(&Entry::Reading { round: current }) = self.entries.get(&video_id)
            && round == current
        {
            let entry = match rating {
                Ok(rating) => Entry::Ready(rating),
                Err(()) => Entry::RetryAt(now + RETRY_DELAY),
            };
            self.entries.insert(video_id, entry);
        }
    }

    fn request(&mut self, backend: &Backend, video_id: String, now: f64) {
        self.round += 1;
        self.in_flight.insert(self.round);
        self.entries.insert(video_id.clone(), Entry::Reading { round: self.round });
        self.next_read = now + READ_GAP;
        backend.request(Req::SongRating { video_id, round: self.round });
    }

    fn sync(&mut self, backend: &Backend, ctx: &Context, now: f64, wanted: &[String]) {
        if self.entries.len() > KEEP {
            self.entries.retain(|id, entry| matches!(entry, Entry::Reading { .. }) || wanted.contains(id));
        }
        if self.in_flight.len() >= MAX_READS {
            return;
        }
        // The overlay reports one upload at a time. The player goes first and large
        // lists only ask for rows on screen, spaced out even when scrolling.
        let mut next_retry = f64::INFINITY;
        let next = wanted.iter().filter(|id| !self.writing(id)).find_map(|id| match self.entries.get(id) {
            None => Some(id),
            Some(Entry::RetryAt(at)) if *at <= now => Some(id),
            Some(Entry::RetryAt(at)) => {
                next_retry = next_retry.min(*at);
                None
            }
            _ => None,
        });
        if let Some(id) = next {
            if now >= self.next_read {
                self.request(backend, id.clone(), now);
                ctx.request_repaint_after(Duration::from_secs_f64(READ_GAP));
            } else {
                ctx.request_repaint_after(Duration::from_secs_f64(self.next_read - now));
            }
        } else if next_retry.is_finite() {
            ctx.request_repaint_after(Duration::from_secs_f64(next_retry.max(self.next_read) - now));
        }
    }
}

#[derive(Clone, Default)]
struct Rows {
    enabled: bool,
    icons: bool,
    ratings: HashMap<String, SongRating>,
    wanted: HashSet<String>,
    visible: Vec<String>,
}

impl Rows {
    fn rating(&self, id: &str, liked: bool) -> Option<SongRating> {
        // Liked Music is already loaded from the account. Show its stars while
        // the individual requests catch up, and prefer the newer answer when it arrives.
        self.ratings.get(id).copied().or(liked.then_some(SongRating::Like))
    }
}

fn rows_id() -> Id {
    Id::unique("song-ratings")
}

/// The track a rating is about: the song version where there is one, as the fork has no
/// stand-ins to unravel.
pub(super) fn rated_track(track: &Track) -> Track {
    track.clone()
}

pub(super) fn row(ui: &Ui, track: &Track, liked: bool, hovered: bool) -> (Option<SongRating>, bool) {
    let (rating, icons, added) = ui.data_mut(|d| {
        let rows = d.get_temp_mut_or_default::<Rows>(rows_id());
        if !rows.enabled || track.id.is_empty() {
            return (None, false, false);
        }
        let rating = rows.rating(&track.id, liked);
        if rows.icons && hovered && rating.is_none() {
            rows.visible.insert(0, track.id.clone());
        } else {
            rows.visible.push(track.id.clone());
        }
        (rating, rows.icons, !rows.wanted.contains(&track.id))
    });
    if added {
        ui.ctx().request_repaint();
    }
    (rating, icons)
}

impl App {
    pub(super) fn song_is_liked(&self, id: &str) -> bool {
        !self.unliking_soon(id)
            && self.ratings.get(id).map_or_else(|| self.liked.contains(id), |rating| rating == SongRating::Like)
    }

    // Run during painting, so a logic-only tick cannot consume the visible
    // tracks before the next frame has a chance to report them again.
    pub(super) fn sync_ratings(&mut self, ctx: &Context, now: f64) {
        let settings = &self.rating_settings;
        let visible = ctx.data_mut(|d| std::mem::take(&mut d.get_temp_mut_or_default::<Rows>(rows_id()).visible));
        let enabled = self.logged_in && settings.list_icons;
        let mut wanted = Vec::new();
        let playing = self.playing_id().filter(|id| self.logged_in && !id.is_empty());
        let changed = playing.map(|id| (id, self.generation)) != self.ratings.playing.as_ref().map(|(id, g)| (id.as_str(), *g));
        if changed {
            self.ratings.playing = playing.map(|id| (id.to_owned(), self.generation));
            if let Some((id, _)) = &self.ratings.playing
                && !matches!(self.ratings.entries.get(id), Some(Entry::Confirmed(_)))
            {
                self.ratings.entries.remove(id);
            }
        }
        if let Some((id, _)) = &self.ratings.playing {
            wanted.push(id.clone());
        }
        if enabled {
            wanted.extend(visible);
        }
        // A liked song's rating is known without asking, which spares a request for
        // every row of Liked Music. What's playing is always asked.
        let fetch: Vec<String> = wanted
            .iter()
            .enumerate()
            .filter(|(i, id)| !crate::local::is_local(id)
                && ((*i == 0 && self.ratings.playing.is_some())
                    || matches!(self.ratings.entries.get(*id), Some(Entry::RetryAt(_)))
                    || !self.liked.contains(*id)))
            .map(|(_, id)| id.clone())
            .collect();
        self.ratings.sync(&self.backend, ctx, now, &fetch);
        let mut ratings: HashMap<_, _> = wanted.iter().filter_map(|id| Some((id.clone(), self.ratings.get(id)?))).collect();
        ratings.extend(self.pending_unlikes().iter().map(|id| (id.clone(), SongRating::Indifferent)));
        ctx.data_mut(|d| d.insert_temp(rows_id(), Rows {
            enabled,
            icons: settings.list_icons,
            ratings,
            wanted: wanted.into_iter().collect(),
            visible: Vec::new(),
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unlike_duration_defaults_to_600_ms_and_stays_in_range() {
        let defaults = Settings::load(|_| None);
        assert!(defaults.confirm_unlike);
        assert!(defaults.unlike_notices);
        assert_eq!(defaults.unlike_hold_ms, 600.0);
        for (saved, expected) in [("0", 200.0), ("450", 450.0), ("3000", 3000.0), ("6000", 3000.0), ("NaN", 600.0)] {
            let settings = Settings::load(|key| (key == UNLIKE_MS).then(|| saved.to_owned()));
            assert_eq!(settings.unlike_hold_ms, expected);
        }
        assert_eq!(Settings::load(|key| (key == "ratings_unlike_hold_seconds").then(|| "2".to_owned())).unlike_hold_ms, 2000.0);
        assert!(!Settings::load(|key| (key == "ratings_unlike_notices").then(|| "0".to_owned())).unlike_notices);
    }

    #[test]
    fn an_unlike_hold_scales_to_the_setting_and_restores_in_one_second() {
        for seconds in [0.2, 1.5, 3.0] {
            let target = Id::unique("song");
            let mut hold = UnlikeHold { target, last_pass: 0, input: HoldInput::Pointer, phase: HoldPhase::Pressed(0.0), progress: 0.0, hint_until: 0.0 };
            assert!(!hold.update(seconds / 2.0, seconds, true, false, false));
            assert_eq!(hold.progress, 0.5);
            assert!(!hold.update(seconds / 2.0, seconds, false, true, true));
            assert!(hold.hint_until > seconds / 2.0);
            assert!(!hold.update(seconds / 2.0 + 0.5, seconds, false, false, false));
            assert_eq!(hold.progress, 0.25);
            assert!(!hold.update(seconds / 2.0 + 1.0, seconds, false, false, false));
            assert_eq!(hold.progress, 0.0);
            hold.phase = HoldPhase::Pressed(10.0);
            assert!(hold.update(10.0 + seconds + 0.001, seconds, true, false, false));
            assert_eq!(hold.progress, 1.0);
            assert!(!hold.update(10.0 + seconds + 0.1, seconds, true, false, false));
        }
    }

    #[test]
    fn ordinary_likes_click_and_changing_the_target_requires_a_new_hold() {
        use egui::{Event, Modifiers, RawInput, Sense};

        let ctx = Context::default();
        Settings::load(|_| None).share_unlike_settings(&ctx);
        let rect = Rect::from_min_size(pos2(20.0, 20.0), vec2(44.0, 34.0));
        let center = rect.center();
        let frame = |time, target: &str, liked, events| {
            let input = RawInput { time: Some(time), events, ..Default::default() };
            let mut result = None;
            ctx.run_ui(input, |ui| {
                let response = ui.interact(rect, Id::unique("rating-button"), Sense::click());
                result = Some(like_gesture(ui, &response, target, liked, None));
            })
            .textures_delta
            .clear();
            result.unwrap()
        };
        let press = |pressed| Event::PointerButton { pos: center, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
        frame(0.0, "first", false, vec![Event::PointerMoved(center)]);
        frame(0.1, "first", false, vec![press(true)]);
        assert!(frame(0.2, "first", false, vec![press(false)]).activate);
        frame(0.3, "first", true, vec![press(true)]);
        let quick = frame(0.4, "first", true, vec![press(false)]);
        assert!(quick.hint);
        assert!(!quick.activate);
        frame(0.5, "first", true, vec![press(true)]);
        assert!(frame(1.0, "first", true, vec![]).progress > 0.0);
        assert_eq!(frame(2.0, "second", true, vec![]).progress, 0.0);
        assert!(!frame(3.6, "first", true, vec![]).activate);
        assert!(!frame(3.7, "second", true, vec![press(false)]).activate);
        frame(4.0, "second", true, vec![press(true)]);
        assert!(!frame(4.59, "second", true, vec![]).activate);
        assert!(frame(4.61, "second", true, vec![]).activate);
        assert!(!frame(4.7, "second", false, vec![press(false)]).activate);
    }

    #[test]
    fn a_rating_from_before_an_edit_cannot_replace_a_new_read() {
        let mut ratings = Ratings::default();
        ratings.entries.insert("song".into(), Entry::Reading { round: 1 });
        ratings.in_flight.insert(1);
        ratings.refresh();
        ratings.entries.insert("song".into(), Entry::Reading { round: 2 });
        ratings.in_flight.insert(2);
        ratings.finish("song".into(), 1, Ok(Some(SongRating::Like)), 0.0);
        assert!(ratings.get("song").is_none());
        assert_eq!(ratings.in_flight.len(), 1);
        ratings.finish("song".into(), 2, Ok(Some(SongRating::Indifferent)), 0.0);
        assert_eq!(ratings.get("song"), Some(SongRating::Indifferent));
        assert!(ratings.in_flight.is_empty());
    }

    #[test]
    fn a_confirmed_write_replaces_a_read_even_when_its_reply_arrives_later() {
        use SongRating::*;

        for (written, read) in [(Like, Some(Indifferent)), (Like, None), (Indifferent, Some(Like))] {
            let mut ratings = Ratings::default();
            ratings.entries.insert("video".into(), Entry::Reading { round: 1 });
            ratings.in_flight.insert(1);
            ratings.confirm("audio", Like);
            ratings.confirm("video", written);
            assert_eq!(ratings.get("video"), Some(written));
            ratings.finish("video".into(), 1, Ok(read), 0.0);
            assert_eq!(ratings.get("video"), Some(written));
            assert_eq!(ratings.get("audio"), Some(Like));
            assert!(ratings.in_flight.is_empty());
            ratings.refresh();
            assert_eq!(ratings.get("video"), None);
        }
    }

    #[test]
    fn row_rating_shows_liked_music_first_and_prefers_the_fresh_answer() {
        let mut rows = Rows::default();
        assert_eq!(rows.rating("song", true), Some(SongRating::Like));
        rows.ratings.insert("song".into(), SongRating::Indifferent);
        assert_eq!(rows.rating("song", true), Some(SongRating::Indifferent));
        rows.ratings.insert("song".into(), SongRating::Like);
        assert_eq!(rows.rating("song", true), Some(SongRating::Like));
    }

    #[test]
    fn coming_back_to_the_window_asks_again_only_after_a_while() {
        let mut ratings = Ratings::default();
        ratings.refresh_if_stale(REFRESH_AFTER);
        ratings.confirm("song", SongRating::Like);
        ratings.refresh_if_stale(REFRESH_AFTER + 5.0);
        assert_eq!(ratings.get("song"), Some(SongRating::Like));
        ratings.refresh_if_stale(2.0 * REFRESH_AFTER);
        assert_eq!(ratings.get("song"), None);
    }

    #[test]
    fn failed_reads_keep_retrying_but_a_confirmed_missing_rating_is_cached() {
        let mut ratings = Ratings::default();
        for (round, now) in [(1, 10.0), (2, 10.0 + RETRY_DELAY)] {
            ratings.entries.insert("song".into(), Entry::Reading { round });
            ratings.finish("song".into(), round, Err(()), now);
            assert!(matches!(ratings.entries["song"], Entry::RetryAt(at) if at == now + RETRY_DELAY));
        }
        ratings.entries.insert("song".into(), Entry::Reading { round: 3 });
        ratings.finish("song".into(), 3, Ok(None), 10.0 + 2.0 * RETRY_DELAY);
        assert!(matches!(ratings.entries["song"], Entry::Ready(None)));
    }

    #[test]
    fn an_unlike_overrides_a_late_read_until_the_write_answers() {
        for (ok, expected) in [(true, SongRating::Indifferent), (false, SongRating::Like)] {
            let mut ratings = Ratings::default();
            ratings.entries.insert("song".into(), Entry::Reading { round: 1 });
            ratings.start_write("song", false);
            ratings.finish("song".into(), 1, Ok(Some(SongRating::Like)), 0.0);
            assert_eq!(ratings.get("song"), Some(SongRating::Indifferent));
            ratings.finish_write("song", false, ok);
            assert_eq!(ratings.get("song"), Some(expected));
            assert!(!ratings.writing("song"));
        }
    }
}
