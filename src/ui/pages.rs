//! Page bodies. Every page fades and slides in when navigated to.

use std::collections::HashSet;
use std::sync::atomic::Ordering;

use egui::{Align2, Color32, Rect, RichText, Sense, Ui, Vec2, pos2, vec2};

use super::row_drag;
use super::widgets::{self, RowOpts, card, chip, h_scroll, named, paint_cover, pill, shelf_header, shelf_header_ext, track_row, video_card};
use super::{Action, App, Dialog, HomeRows, LibTab, Page, ProviderVerdicts, Selection};
use crate::backend::{PlaylistSort, compact};
use crate::model::{Card, ShelfKind, Target, Track};
use crate::theme::{self, icon};

const CARD: f32 = 180.0;

/// The central panel's left inset (its frame, in `mod.rs`), which full-bleed art —
/// the artist's banner, home's hero — paints across to reach the sidebar's edge.
pub(crate) const PAGE_INSET: i8 = 28;

/// How many tiles Apple fits across a feed row at this content width. The site
/// measures from its own viewport, which includes the navigation column this page
/// already sits past.
fn feed_columns(width: f32, square: bool) -> usize {
    let viewport = width + 260.0;
    match square {
        true if viewport < 1260.0 => 4,
        true if viewport < 1580.0 => 5,
        true => 6,
        false if viewport < 1260.0 => 2,
        false if viewport < 1580.0 => 3,
        false => 4,
    }
}

/// One tile of an Apple feed row: a share of the content on wide windows (the
/// site's own breakpoints), and below them as many whole tiles as the width
/// holds. The site's fixed narrow tile left a stub of the next card hanging off
/// the edge, with room for a whole one beside it; tiling leaves neither.
fn feed_tile(ui: &Ui, square: bool) -> f32 {
    let width = ui.available_width();
    let min = if square { 144.0 } else { 240.0 };
    if width >= 1024.0 {
        let cols = feed_columns(width, square);
        return ((width - 20.0 * (cols - 1) as f32) / cols as f32).max(min);
    }
    let cols = (((width + 20.0) / (min + 20.0)).floor() as usize).max(1);
    (width - 20.0 * (cols - 1) as f32) / cols as f32
}

pub fn page(app: &mut App, ui: &mut Ui, now: f64) {
    let t = theme::ease_out(((now - app.page_changed) / 0.32) as f32);
    if t < 1.0 {
        // The fade-in finishes even when nothing else is drawing frames.
        ui.ctx().request_repaint();
    }
    ui.set_opacity(0.15 + 0.85 * t);
    let page = app.page.clone();
    // Each page (each album, each search) keeps its own scroll position.
    let mut area = egui::ScrollArea::vertical().id_salt(("page", &page)).auto_shrink(false);
    // At the window's right edge: clear of where it resizes the window.
    ui.spacing_mut().scroll.bar_outer_margin += super::titlebar::edge_inset();
    // And below the window buttons at the top right, which its strip would cross.
    let outer = ui.max_rect();
    let bar_top = super::titlebar::buttons_rect(ui.ctx()).map_or(0.0, |b| b.height());
    area = area.scroll_bar_rect(Rect::from_min_max(pos2(outer.left(), outer.top() + bar_top), outer.max));
    // Dev hook: YTM_SCROLL=<px> pins the page scroll, for screenshots of lower sections.
    if let Some(y) = super::dev_var("YTM_SCROLL").and_then(|v| v.parse::<f32>().ok()) {
        area = area.vertical_scroll_offset(y);
    }
    // Settings manages its own scrolling, so its list of categories stays put while one
    // of them scrolls.
    if page == Page::Settings {
        settings(app, ui);
        return;
    }
    let out = area.show(ui, |ui| {
        ui.set_max_width((ui.available_width() - 20.0).max(0.0));
        ui.add_space(18.0 * (1.0 - t));
        match &page {
            Page::Home => home(app, ui, now),
            Page::New => new(app, ui, now),
            Page::Mood(p) => mood(app, ui, p, now),
            Page::Search(q) => search(app, ui, q, now),
            Page::Album(id) | Page::Playlist(id) => collection(app, ui, id, now),
            Page::Artist(id) => artist(app, ui, id, now),
            Page::ArtistShelf { artist, title } => artist_shelf(app, ui, artist, title, now),
            Page::Library(tab) => library(app, ui, *tab, now),
            Page::Downloads => downloads(app, ui, now),
            Page::Settings => {}
            Page::Transfer => super::transfer::page(app, ui, now),
            Page::Jam => super::jam::page(app, ui),
        }
        // Room to scroll the last rows clear of the player's frosted strip.
        ui.add_space(96.0);
    });
    selection_bar(app, ui.ctx(), &page, out.inner_rect);
    // Infinite scroll on home: fetch the next shelves when nearing the bottom.
    if page == Page::Home {
        let remaining = out.content_size.y - (out.state.clamped_offset().y + out.inner_rect.height());
        if let Some(h) = &mut app.home {
            // A failed fetch reports nothing (it's a quiet request): allow a retry later.
            if h.loading_more.is_some_and(|t| now - t > 20.0) {
                h.loading_more = None;
            }
            if remaining < 700.0 && h.loading_more.is_none()
                && let Some(token) = h.continuation.clone()
            {
                h.loading_more = Some(now);
                app.backend.request(crate::backend::Req::HomeMore(token));
            }
        }
    }
}

/// What every song row needs from the app, gathered once per frame.
pub(super) struct Rows {
    current: Option<String>,
    playing: bool,
    accent: Color32,
    now: f64,
    hide_explicit: bool,
}

impl Rows {
    pub(super) fn new(app: &App, now: f64) -> Self {
        Self {
            current: app.current_id().map(str::to_owned),
            playing: app.audio.status.playing.load(Ordering::Relaxed),
            accent: app.accent_color(),
            now,
            hide_explicit: app.hide_explicit,
        }
    }

    /// `t` is listed: it isn't an explicit song while those are hidden.
    pub(super) fn shows(&self, t: &Track) -> bool {
        !(self.hide_explicit && t.explicit)
    }

    pub(super) fn opts(&self, liked: &HashSet<String>, t: &Track, number: Option<usize>, show_album: bool) -> RowOpts {
        RowOpts {
            number,
            rank: false,
            show_album,
            current: self.current.as_deref() == Some(t.id.as_str()),
            playing: self.playing,
            accent: self.accent,
            time: self.now,
            liked: liked.contains(&t.id),
            added_by: None,
            pick: false,
            selected: None,
            remove: None,
            hide_duration_hover: false,
        }
    }
}

/// A page's data hasn't arrived: shimmering placeholders while it loads, a retry if the
/// request failed.
fn placeholder(app: &mut App, ui: &mut Ui, now: f64, rows: usize, card: f32) {
    if app.load_failed {
        ui.add_space(40.0);
        ui.label(RichText::new("Couldn't load this page").color(theme::text_dim()));
        ui.add_space(12.0);
        if pill(ui, "Try again", None, false).clicked() {
            app.refresh();
        }
    } else {
        widgets::skeleton(ui, now, rows, card);
    }
}

/// What a collection header's round buttons draw: a font glyph, or the three dots.
enum CircleGlyph {
    Char(char),
    More,
}

/// A round glass action on a collection header (shuffle, search, more).
fn circle_action(ui: &mut Ui, glyph: CircleGlyph, name: &str, active: bool) -> egui::Response {
    // Same 38 px height as the Play pill beside it, so the row shares one centre line.
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(38.0), Sense::click());
    let resp = named(resp, name).on_hover_text(name).on_hover_cursor(egui::CursorIcon::PointingHand);
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&resp));
    let h = theme::anim_bool(ui.ctx(), resp.id.with("h"), resp.hovered() || open, 0.15);
    let press = if resp.is_pointer_button_down_on() { 18.3 } else { 19.0 };
    ui.painter().circle_filled(rect.center(), press, theme::glass(0.10 + 0.09 * h));
    let color = theme::lerp_color(theme::text_dim(), theme::text(), h.max(active as u8 as f32));
    match glyph {
        CircleGlyph::Char(g) => widgets::icon_at(ui, rect.center(), g, 16.0, color),
        CircleGlyph::More => {
            for i in -1..=1 {
                ui.painter().circle_filled(rect.center() + vec2(i as f32 * 5.2, 0.0), 1.8, color);
            }
        }
    }
    resp
}

/// A page's main Play button. Shows Pause while this page's album / playlist / artist
/// is playing, and toggles instead of restarting when it's already loaded.
fn play_pill(ui: &mut Ui, acts: &mut Vec<Action>, target: &Target, tracks: &[Track], order: Option<&[usize]>) {
    let active = widgets::active_state(ui, target);
    let (label, glyph) = if active == Some(true) { ("Pause", icon::PAUSE) } else { ("Play", icon::PLAY) };
    if pill(ui, label, Some(glyph), true).clicked() {
        if active.is_some() {
            acts.push(Action::TogglePlay);
        } else if !tracks.is_empty() {
            // All of it from the top, in the order it's sorted.
            let plays = match order {
                Some(o) => o.iter().map(|&i| tracks[i].clone()).collect(),
                None => tracks.to_vec(),
            };
            acts.push(Action::Play(plays, 0));
        }
    }
}

fn card_row(ui: &mut Ui, id: &str, title: &str, cards: &[Card], acts: &mut Vec<Action>) {
    if cards.is_empty() {
        return;
    }
    reveal(ui, id, |ui| {
        let (l, r) = shelf_header(ui, title, None, None, widgets::shelf_overflows(ui, id));
        let nudge = r as i32 - l as i32;
        let tile = feed_tile(ui, true);
        h_scroll(ui, id, nudge, cards.len(), tile, 20.0, |ui| {
            ui.spacing_mut().item_spacing.x = 20.0;
            for c in cards {
                card(ui, c, tile, false, None, acts);
            }
        });
    });
}

/// A shelf of Mochy's concert cards: video frames a little wider than tall, the
/// title and who it's by over a scrim at the top. Empty shelves leave no gap.
fn video_row(ui: &mut Ui, id: &str, title: &str, cards: &[Card], acts: &mut Vec<Action>) {
    if cards.is_empty() {
        return;
    }
    reveal(ui, id, |ui| {
        let (l, r) = shelf_header(ui, title, None, None, widgets::shelf_overflows(ui, id));
        let nudge = r as i32 - l as i32;
        // four across a wide window, the frames keeping the phone card's proportions;
        // on a narrower one, fewer and wider rather than a frame cut off at the edge
        let width = ui.available_width();
        let mut cols = 4.0;
        while cols > 1.0 && (width - 20.0 * (cols - 1.0)) / cols < 240.0 {
            cols -= 1.0;
        }
        let tile = ((width - 20.0 * (cols - 1.0)) / cols).min(330.0).min(width - 40.0);
        h_scroll(ui, id, nudge, cards.len(), tile, 20.0, |ui| {
            ui.spacing_mut().item_spacing.x = 20.0;
            for c in cards {
                video_card(ui, c, tile, acts);
            }
        });
    });
}

/// A shelf's first appearance on screen: it fades in over half a second, the way
/// Mochy's TV rows reveal as the list carries them in — rows rise out of the page
/// instead of sitting there. Once it's played, it stays.
fn reveal(ui: &mut Ui, id: &str, body: impl FnOnce(&mut Ui)) {
    let vid = egui::Id::unique(("shelf-reveal", id));
    let now = ui.ctx().input(|i| i.time);
    // anything at or above the view's bottom edge has been reached; below the
    // fold, a shelf keeps still until the scroll brings it in
    let top = ui.cursor().top();
    let on_screen = top <= ui.clip_rect().bottom();
    let mut start = ui.ctx().data(|d| d.get_temp::<f64>(vid)).unwrap_or(f64::INFINITY);
    if start.is_infinite() && on_screen {
        start = now;
    }
    let t = if start.is_finite() { (((now - start) / 0.45) as f32).clamp(0.0, 1.0) } else { 0.0 };
    if t < 1.0 {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
    }
    // re-set every frame: temp data dies two frames after its last write
    ui.ctx().data_mut(|d| d.insert_temp(vid, start));
    let te = theme::ease_out(t);
    ui.scope(|ui| {
        ui.set_opacity(0.05 + 0.95 * te);
        body(ui);
    });
}

/// A card shelf with a Show all on its header (`all`: there is more where this came
/// from, and `open` asks for it). The arrows stay: the shelf still scrolls sideways.
fn card_row_all(ui: &mut Ui, id: &str, title: &str, cards: &[Card], all: bool, acts: &mut Vec<Action>, open: impl FnOnce() -> Action) {
    if cards.is_empty() {
        return;
    }
    let (l, r, show_all) = widgets::shelf_header_ext(ui, title, None, None, widgets::shelf_overflows(ui, id), all);
    if show_all {
        acts.push(open());
    }
    let nudge = r as i32 - l as i32;
    let tile = feed_tile(ui, true);
    h_scroll(ui, id, nudge, cards.len(), tile, 20.0, |ui| {
        ui.spacing_mut().item_spacing.x = 20.0;
        for c in cards {
            card(ui, c, tile, false, None, acts);
        }
    });
}

/// Songs laid out in columns of `rows`, scrolling sideways (Quick picks). Clicking a
/// song starts its radio. Returns whether the arrows reached the end.
#[allow(clippy::too_many_arguments)]
fn song_grid(ui: &mut Ui, rows_ctx: &Rows, liked: &HashSet<String>, acts: &mut Vec<Action>, id: &str, title: &str, strap: Option<&str>, tracks: &[Track], rows: usize) -> bool {
    let (l, r) = shelf_header(ui, title, strap, None, widgets::shelf_overflows(ui, id));
    let nudge = r as i32 - l as i32;
    let col_w = feed_tile(ui, false);
    let tracks: Vec<&Track> = tracks.iter().filter(|t| rows_ctx.shows(t)).collect();
    let mut picked = None;
    let (_, reached_end) = h_scroll(ui, id, nudge, tracks.len().div_ceil(rows.max(1)), col_w, 20.0, |ui| {
        ui.spacing_mut().item_spacing.x = 20.0;
        for (c, col) in tracks.chunks(rows.max(1)).enumerate() {
            ui.allocate_ui(vec2(col_w, rows as f32 * 60.0), |ui| {
                ui.set_width(col_w);
                ui.vertical(|ui| {
                    for (i, t) in col.iter().enumerate() {
                        let o = rows_ctx.opts(liked, t, None, false);
                        if track_row(ui, t, &o, acts).clicked() {
                            picked = Some(c * rows + i);
                        }
                    }
                });
            });
        }
    });
    if let Some(i) = picked {
        acts.push(Action::Radio(Track::clone(tracks[i])));
    }
    reached_end
}

/// Where a song list lives: the page its selection belongs to, and whether it's your own
/// playlist (so songs can be taken out of it).
struct ListCtx<'a> {
    sel: &'a mut Selection,
    page: &'a Page,
    own: bool,
}

/// A long song list; only the rows on screen are built. Clicking one plays `tracks` from it,
/// or picks it while selecting: Ctrl-click picks one song, Shift-click a run of them.
#[allow(clippy::too_many_arguments)]
fn list(ui: &mut Ui, rows: &Rows, liked: &HashSet<String>, tracks: &[Track], order: Option<&[usize]>, show_album: bool, number: impl Fn(usize, &Track) -> Option<usize>, cx: ListCtx, drag: &mut Option<row_drag::RowDrag>, drag_list: egui::Id, acts: &mut Vec<Action>) {
    let ListCtx { sel, page, own } = cx;
    let selecting = sel.on(page);
    let mods = ui.input(|i| i.modifiers);
    widgets::visible_rows(ui, order.map_or(tracks.len(), |o| o.len()), |ui, k| {
        let i = order.map_or(k, |o| o[k]);
        let t = &tracks[i];
        // Keyed on the song as well as the place: a song's open menu closes when the order
        // changes under it, instead of turning into another song's.
        ui.push_id(&t.id, |ui| {
            // Any song can be dragged onto a playlist in the sidebar.
            row_drag::grip(ui, drag, drag_list, i, t);
            let o = RowOpts {
                selected: selecting.then(|| sel.rows.contains(&i)),
                remove: own.then_some(i),
                ..rows.opts(liked, t, number(i, t), show_album)
            };
            if track_row(ui, t, &o, acts).clicked() {
                if selecting || mods.command || mods.shift {
                    // A run of picks is of the rows on show, in their order on screen.
                    sel.pick(page, i, mods.shift, order);
                } else if let Some(o) = order {
                    // What's on show plays, in its order.
                    let plays: Vec<Track> = o.iter().map(|&r| tracks[r].clone()).collect();
                    acts.push(Action::Play(plays, k));
                } else {
                    acts.push(Action::Play(tracks.to_vec(), i));
                }
            }
        });
    });
    // These lists don't change order: a picked-up song is only carried out, onto a
    // playlist in the sidebar.
    row_drag::carry(ui, drag, drag_list);
}

/// A list page's "Select" button: turns select mode on and off.
fn select_pill(ui: &mut Ui, sel: &mut Selection, page: &Page) {
    let on = sel.on(page);
    if pill(ui, if on { "Done" } else { "Select" }, Some(icon::LIST_CHECKS), false).clicked() {
        if on { sel.clear() } else { sel.start(page) }
    }
}

/// A list's Sort button, with its orders in a menu. Away while the list is still coming in.
fn sort_pill(ui: &mut Ui, sorts: &mut super::sorts::ListSorts, page: &Page, held: bool) {
    if let Some(list) = super::sorts::list_id(page) {
        sort_pill_list(ui, sorts, page, &list, held);
    }
}

/// The same Sort button for a list that isn't the page's only one (the Uploads page's
/// local music): the page's orders, kept under the list's own id.
fn sort_pill_list(ui: &mut Ui, sorts: &mut super::sorts::ListSorts, page: &Page, list: &str, held: bool) {
    if held {
        return;
    }
    let options = super::sorts::options(page);
    let now = sorts.sort(list);
    let label = match options.iter().find(|(s, _)| *s == now && now != super::sorts::Sort::Default) {
        Some((_, name)) => format!("Sort: {name}"),
        None => "Sort".to_owned(),
    };
    let resp = pill(ui, &label, None, false);
    egui::Popup::menu(&resp).show(|ui| {
        ui.set_width(190.0);
        ui.add_space(3.0);
        for (s, name) in options {
            if widgets::menu_choice(ui, name, s == now) {
                sorts.set_sort(list, s);
            }
        }
        ui.add_space(3.0);
    });
}

/// Your own playlist's Sort button: YouTube Music's orders, saved to the playlist as there,
/// which then loads again in the new order. Away while the list is still coming in.
fn saved_sort_pill(ui: &mut Ui, sorts: &mut super::sorts::ListSorts, id: &str, saved: &[PlaylistSort], busy: bool, sorting: bool, acts: &mut Vec<Action>) {
    // It comes in the order picked here: none of the app's own sorts on top.
    if sorts.sort(id) != super::sorts::Sort::Default {
        sorts.set_sort(id, super::sorts::Sort::Default);
    }
    // One sort at a time: what's ticked is stale until the playlist has loaded again.
    let label = match saved.iter().find(|s| s.on && !s.manual) {
        _ if sorting => "Sorting…".to_owned(),
        Some(s) => format!("Sort: {}", s.name),
        None => "Sort".to_owned(),
    };
    let resp = ui.add_enabled_ui(!busy, |ui| pill(ui, &label, None, false)).inner;
    egui::Popup::menu(&resp).show(|ui| {
        ui.set_width(190.0);
        ui.add_space(3.0);
        for s in saved {
            if widgets::menu_choice(ui, &s.name, s.on) && !s.on {
                acts.push(Action::SortPlaylist(id.to_owned(), s.actions.clone()));
            }
        }
        ui.add_space(3.0);
    });
}

/// The songs a page lists, and whether they're your own playlist's.
fn selectable<'a>(app: &'a App, page: &Page) -> Option<(&'a [Track], bool)> {
    match page {
        Page::Album(id) | Page::Playlist(id) => {
            app.opened(id).map(|o| &o.c).map(|c| (c.tracks.as_slice(), app.logged_in && c.owned && !c.is_album))
        }
        Page::Library(LibTab::Songs) => app.opened(super::LIKED_MUSIC).map(|o| (o.c.tracks.as_slice(), false)),
        Page::Library(LibTab::History) => app.history.as_deref().map(|h| (h, false)),
        Page::Library(LibTab::Uploads) => app.uploads.as_deref().map(|u| (u, false)),
        _ => None,
    }
}

/// While selecting: a bar floating over the bottom of the page with what to do with the picks.
fn selection_bar(app: &mut App, ctx: &egui::Context, page: &Page, area: Rect) {
    if !app.selection.on(page) {
        return;
    }
    let Some((tracks, own)) = selectable(app, page) else { return };
    let n = tracks.len();
    // In the order on screen.
    let order = super::sorts::list_id(page).and_then(|l| app.sorts.order(&l, tracks));
    let picked: Vec<Track> = match &order {
        Some(s) => s.iter().filter(|i| app.selection.rows.contains(i)).map(|&i| tracks[i].clone()).collect(),
        None => app.selection.rows.iter().filter_map(|&i| tracks.get(i).cloned()).collect(),
    };
    let all = !picked.is_empty() && picked.len() == n;
    let all_liked = !picked.is_empty() && picked.iter().all(|t| app.liked.contains(&t.id));
    // Labelled buttons when there's room, icons with tooltips when the window is narrow.
    let wide = area.width() > 820.0;
    let button = |ui: &mut Ui, label: &str, glyph: char, primary: bool| {
        if wide { pill(ui, label, Some(glyph), primary) } else { widgets::icon_button(ui, glyph, 18.0, label, false) }
    };
    let mut acts = Vec::new();
    let mut toggle_all = false;
    let mut done = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    egui::Area::new(egui::Id::unique("selection-bar"))
        .order(egui::Order::Foreground)
        .pivot(Align2::CENTER_BOTTOM)
        .fixed_pos(pos2(area.center().x, area.bottom() - 18.0))
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(theme::menu_fill())
                .corner_radius(14)
                .inner_margin(egui::Margin { left: 20, right: 10, top: 8, bottom: 8 })
                .shadow(egui::epaint::Shadow { offset: [0, 8], blur: 24, spread: 0, color: theme::shade(0.45) })
                .show(ui, |ui| {
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        let count = if picked.is_empty() { "Select songs".to_owned() } else { format!("{} selected", picked.len()) };
                        ui.label(RichText::new(count).font(theme::semibold(14.0)).color(theme::text()));
                        ui.add_space(10.0);
                        if !picked.is_empty() {
                            if button(ui, "Add to playlist", icon::LIST_PLUS, true).clicked() {
                                acts.push(Action::AddToPlaylist(picked.clone()));
                            }
                            if own && button(ui, "Remove from playlist", icon::LIST_MINUS, false).clicked() {
                                acts.push(Action::RemoveFromPlaylist(app.selection.rows.iter().copied().collect()));
                            }
                            if button(ui, if all_liked { "Remove like" } else { "Like" }, icon::HEART, false).clicked() {
                                acts.push(Action::LikeMany(picked.clone(), !all_liked));
                            }
                            if button(ui, "Add to queue", icon::LIST_MUSIC, false).clicked() {
                                acts.extend(picked.iter().cloned().map(Action::Enqueue));
                            }
                            // Local songs have nothing to download: their file is already here.
                            let downloadable: Vec<Track> = picked.iter().filter(|t| !crate::local::is_local(&t.id)).cloned().collect();
                            if !downloadable.is_empty() && !matches!(page, Page::Downloads) && button(ui, "Download", icon::DOWNLOAD, false).clicked() {
                                acts.push(Action::DownloadMany(None, String::new(), Some(downloadable)));
                            }
                        }
                        toggle_all = button(ui, if all { "Select none" } else { "Select all" }, icon::LIST_CHECKS, false).clicked();
                        done |= widgets::icon_button(ui, icon::X, 18.0, "Done selecting", false).clicked();
                    });
                });
        });
    if toggle_all {
        let page = page.clone();
        app.selection.start(&page);
        if !all {
            app.selection.rows = (0..n).collect();
        }
    }
    if done || !acts.is_empty() {
        app.selection.clear();
    }
    app.actions.extend(acts);
}

// ------------------------------------------------------------------ home

fn home(app: &mut App, ui: &mut Ui, now: f64) {
    // No title of its own: the hero opens the page, the way Mochy's phone and TV
    // homes both do — the stage is the greeting.
    // A refresh keeps showing the current feed; the skeleton is only for an empty one.
    if app.home.as_ref().is_none_or(|h| app.home_loading && h.shelves.is_empty()) {
        placeholder(app, ui, now, 3, CARD);
        return;
    }
    let rows = Rows::new(app, now);
    // The hero that opens the page: what's playing, a roll, discoveries, playlists.
    super::hero::draw(app, ui, now);
    // The local rows and the recommendation rows, grown from what you play here,
    // dealt one against the other the way Mochy's home deals its categories between
    // its own rows — two of its own first, the releases after the first round of
    // recommendation rows.
    let reco: Vec<crate::recommend::Row> = app.reco.rows.clone();
    let new_from = app.reco.new_from.clone();
    if app.home.as_ref().is_none_or(|h| h.shelves.is_empty()) && reco.is_empty() && new_from.is_empty() && app.home_rows.is_empty() {
        ui.add_space(40.0);
        ui.label(RichText::new("Nothing here yet").color(theme::text_dim()));
    }
    // Which local row is next: recently played and most played open the page, keep
    // listening and the forgotten favourites follow against the recommendation rows.
    let mut local = 0usize;
    for (i, r) in reco.iter().enumerate() {
        while local < if i == 0 { 2 } else { 1 } {
            local = draw_local_row(&app.home_rows, local, ui, &rows, &app.liked, &mut app.actions);
        }
        if i == 3 && !new_from.is_empty() {
            card_row(ui, "reco-new", "New from your artists", &new_from, &mut app.actions);
        }
        let id = format!("reco-{i}-{}", r.title);
        reco_grid(ui, &rows, &app.liked, &mut app.actions, &id, r);
    }
    while local < 4 {
        local = draw_local_row(&app.home_rows, local, ui, &rows, &app.liked, &mut app.actions);
    }
    if reco.len() < 3 && !new_from.is_empty() {
        card_row(ui, "reco-new", "New from your artists", &new_from, &mut app.actions);
    }
    // From the community — playlists off the artists you play that aren't YouTube's
    // own — and the live shows and music videos of theirs, as Mochy's concert cards:
    // wide frames with the words laid over the top
    video_row(ui, "local-community", "From the community", &app.community, &mut app.actions);
    video_row(ui, "local-live", "Live shows", &app.live_shows, &mut app.actions);
    video_row(ui, "local-videos", "Music videos for you", &app.videos_for_you, &mut app.actions);
    let Some(h) = &app.home else { return };
    let mut more_picks = None;
    for (i, s) in h.shelves.iter().enumerate() {
        // "All time hits" after the first feed shelf, the way Mochy slots the charts in
        if i == 1 {
            card_row(ui, "local-hits", "All time hits", &app.all_time_hits, &mut app.actions);
        }
        let id = format!("home-{i}-{}", s.title);
        match &s.kind {
            ShelfKind::Cards(cards) => {
                let cards: Vec<&Card> = cards.iter().filter(|c| !matches!(&c.target, Target::Song(t) if !rows.shows(t))).collect();
                if cards.is_empty() {
                    continue;
                }
                let (strap, thumb) = if s.title == "Listen again" { (None, None) } else { (s.strapline.as_deref(), s.strap_thumb.as_deref()) };
                reveal(ui, &id, |ui| {
                    let (l, r) = shelf_header(ui, &s.title, strap, thumb, widgets::shelf_overflows(ui, &id));
                    let nudge = r as i32 - l as i32;
                    let tile = feed_tile(ui, true);
                    let acts = &mut app.actions;
                    let liked = &app.liked;
                    h_scroll(ui, &id, nudge, cards.len(), tile, 20.0, |ui| {
                        ui.spacing_mut().item_spacing.x = 20.0;
                        for c in cards {
                            let is_liked = matches!(&c.target, Target::Song(t) if liked.contains(&t.id));
                            card(ui, c, tile, is_liked, None, acts);
                        }
                    });
                });
            }
            ShelfKind::Songs { tracks, rows: n } => {
                let reached_end = song_grid(ui, &rows, &app.liked, &mut app.actions, &id, &s.title, s.strapline.as_deref(), tracks, *n);
                if s.title == "Quick picks" && reached_end && !h.picks_loading && !h.picks_expanded {
                    more_picks = tracks.last().map(|t| t.id.clone());
                }
            }
        }
    }
    let loading_more = h.loading_more.is_some();
    if let Some(seed) = more_picks
        && let Some(h) = &mut app.home
    {
        h.picks_loading = true;
        app.backend.request(crate::backend::Req::MoreQuickPicks(seed));
    }
    if loading_more {
        widgets::skeleton(ui, now, 1, CARD);
    }
}

/// One of Home's local rows (the listening-history half): `which` — recently
/// played (0), most played (1), keep listening (2), the forgotten favourites (3).
/// Empty rows leave no gap: the count moves on either way. Returns the next row.
fn draw_local_row(hr: &HomeRows, which: usize, ui: &mut Ui, rows_ctx: &Rows, liked: &HashSet<String>, acts: &mut Vec<Action>) -> usize {
    match which {
        0 => card_row(ui, "local-recent", "Recently played", &hr.recently_played, acts),
        1 => {
            if !hr.most_played.is_empty() {
                ranked_grid(ui, rows_ctx, liked, acts, "local-most", "Most played", &hr.most_played);
            }
        }
        2 => card_row(ui, "local-keep", "Keep listening", &hr.keep, acts),
        _ => {
            if !hr.forgotten.is_empty() {
                song_grid(ui, rows_ctx, liked, acts, "local-forgotten", "Don't forget these songs", None, &hr.forgotten, 3);
            }
        }
    }
    which + 1
}

/// "Most played", each song with its rank in front of it — Mochy's HomeTrackGrid.
fn ranked_grid(ui: &mut Ui, rows_ctx: &Rows, liked: &HashSet<String>, acts: &mut Vec<Action>, id: &str, title: &str, tracks: &[Track]) {
    reveal(ui, id, |ui| {
        let (l, r) = shelf_header(ui, title, None, None, widgets::shelf_overflows(ui, id));
        let nudge = r as i32 - l as i32;
        let col_w = feed_tile(ui, false);
        let tracks: Vec<&Track> = tracks.iter().filter(|t| rows_ctx.shows(t)).collect();
        let mut picked = None;
        h_scroll(ui, id, nudge, tracks.len().div_ceil(3), col_w, 20.0, |ui| {
            ui.spacing_mut().item_spacing.x = 20.0;
            for (c, col) in tracks.chunks(3).enumerate() {
                ui.allocate_ui(vec2(col_w, 3.0 * 60.0), |ui| {
                    ui.set_width(col_w);
                    ui.vertical(|ui| {
                        for (i, t) in col.iter().enumerate() {
                            // the rank, not a track number: 1 to 20 in front of each song
                            let o = rows_ctx.opts(liked, t, Some(c * 3 + i + 1), false);
                            let o = RowOpts { rank: true, ..o };
                            if track_row(ui, t, &o, acts).clicked() {
                                picked = Some(c * 3 + i);
                            }
                        }
                    });
                });
            }
        });
        if let Some(i) = picked {
            acts.push(Action::Radio(Track::clone(tracks[i])));
        }
    });
}

/// One of Home's recommendation rows (the HomeRecommender port): a song grid with
/// a header for what it grew from — the seed's cover beside the title, a Show all
/// onto the artist or genre — and clicking a song starts its radio, as the feed's do.
/// A row with nowhere to send a Show all (a "Because you like" one) is a handful
/// of songs rather than a grid's worth: it lays out as big covers, the way Listen
/// again does.
fn reco_grid(ui: &mut Ui, rows_ctx: &Rows, liked: &HashSet<String>, acts: &mut Vec<Action>, id: &str, r: &crate::recommend::Row) {
    let strap = match r.kind {
        crate::recommend::Kind::BecauseYouLike => "Because you like",
        crate::recommend::Kind::MoreLike => "More like",
        crate::recommend::Kind::YourGenre => "Because you listen to",
        crate::recommend::Kind::NewGenre => "Try something new",
    };
    reveal(ui, id, |ui| {
        let (l, right, show_all) = shelf_header_ext(ui, &r.title, Some(strap), r.thumb.as_deref(), widgets::shelf_overflows(ui, id), r.endpoint.is_some());
        if show_all {
            match r.endpoint.as_ref() {
                Some(crate::recommend::Endpoint::Artist(id)) => acts.push(Action::Go(Page::Artist(id.clone()))),
                Some(crate::recommend::Endpoint::Mood(params)) => acts.push(Action::Go(Page::Mood(params.clone()))),
                None => {}
            }
        }
        let tracks: Vec<&Track> = r.tracks.iter().filter(|t| rows_ctx.shows(t)).collect();
        if tracks.is_empty() {
            return;
        }
        let nudge = right as i32 - l as i32;
        if r.endpoint.is_none() {
            // Nothing behind a Show all: the songs are the row's whole worth, and
            // covers say more than thin rows do at that size.
            let tile = feed_tile(ui, true);
            h_scroll(ui, id, nudge, tracks.len(), tile, 20.0, |ui| {
                ui.spacing_mut().item_spacing.x = 20.0;
                for t in &tracks {
                    let is_liked = liked.contains(&t.id);
                    card(ui, &super::responses::song_card(t), tile, is_liked, None, acts);
                }
            });
            return;
        }
        let col_w = feed_tile(ui, false);
        let mut picked = None;
        h_scroll(ui, id, nudge, tracks.len().div_ceil(3), col_w, 20.0, |ui| {
            ui.spacing_mut().item_spacing.x = 20.0;
            for (c, col) in tracks.chunks(3).enumerate() {
                ui.allocate_ui(vec2(col_w, 3.0 * 60.0), |ui| {
                    ui.set_width(col_w);
                    ui.vertical(|ui| {
                        for (i, t) in col.iter().enumerate() {
                            let o = rows_ctx.opts(liked, t, None, false);
                            if track_row(ui, t, &o, acts).clicked() {
                                picked = Some(c * 3 + i);
                            }
                        }
                    });
                });
            }
        });
        if let Some(i) = picked {
            acts.push(Action::Radio(Track::clone(tracks[i])));
        }
    });
}

// ------------------------------------------------------------------ new

fn new(app: &mut App, ui: &mut Ui, now: f64) {
    page_title(ui, "New");
    if app.explore.is_none() {
        placeholder(app, ui, now, 2, CARD);
        return;
    }
    let rows = Rows::new(app, now);
    let Some(e) = &app.explore else { return };
    card_row(ui, "ex-new", if app.logged_in { "New releases for you" } else { "Popular new releases" }, &e.new_albums, &mut app.actions);
    if !e.top.is_empty() {
        song_grid(ui, &rows, &app.liked, &mut app.actions, "ex-top", "Top songs", None, &e.top, 4);
    }
    card_row(ui, "ex-charts", "Charts", &e.charts, &mut app.actions);
    for g in &e.moods {
        mood_grid(ui, g, &mut app.actions);
    }
}

/// A group of Moods & genres: its categories in as many columns as fit, each opening its page.
fn mood_grid(ui: &mut Ui, g: &crate::ytm::MoodGroup, acts: &mut Vec<Action>) {
    shelf_header(ui, &g.title, None, None, false);
    let gap = 14.0;
    let w = ui.available_width();
    let cols = ((w + gap) / (190.0 + gap)).floor().max(1.0);
    // A little under the exact share, so rounding never pushes the last one to a new line.
    let bw = ((w - gap * (cols - 1.0)) / cols - 0.5).max(60.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(gap, gap);
        for m in &g.moods {
            if mood_button(ui, m, bw).clicked() {
                acts.push(Action::Go(Page::Mood(m.params.clone())));
            }
        }
    });
}

/// A category's button: its name, and the stripe of its own colour down the left edge.
fn mood_button(ui: &mut Ui, m: &crate::ytm::Mood, w: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 48.0), Sense::click());
    let resp = named(resp, &m.title).on_hover_cursor(egui::CursorIcon::PointingHand);
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let h = theme::anim_bool(ui.ctx(), resp.id.with("h"), resp.hovered(), 0.15);
    let radius = (crate::preset::active().chrome.radius * 0.4).clamp(4.0, 12.0) as u8;
    let p = ui.painter();
    p.rect_filled(rect, radius, theme::glass(0.08 + 0.06 * h));
    let [_, r, g, b] = m.color.to_be_bytes();
    let stripe = Rect::from_min_size(rect.min, vec2(6.0, rect.height()));
    p.rect_filled(stripe, egui::CornerRadius { nw: radius, sw: radius, ne: 0, se: 0 }, Color32::from_rgb(r, g, b));
    let font = theme::semibold(14.5);
    let y = rect.center().y - ui.fonts_mut(|f| f.row_height(&font)) / 2.0;
    widgets::text_at(ui, pos2(rect.left() + 20.0, y), &m.title, font, theme::text(), w - 28.0);
    resp
}

/// A Moods & genres category: its shelves of playlists.
fn mood(app: &mut App, ui: &mut Ui, params: &str, now: f64) {
    let Some(m) = app.mood.as_ref().filter(|m| m.params == params) else {
        placeholder(app, ui, now, 3, CARD);
        return;
    };
    page_title(ui, &m.title);
    let rows = Rows::new(app, now);
    for (i, s) in m.shelves.iter().enumerate() {
        let id = format!("mood-{i}-{}", s.title);
        match &s.kind {
            ShelfKind::Cards(cards) => {
                let cards: Vec<Card> = cards.iter().filter(|c| !matches!(&c.target, Target::Song(t) if !rows.shows(t))).cloned().collect();
                if !cards.is_empty() {
                    card_row(ui, &id, &s.title, &cards, &mut app.actions);
                }
            }
            ShelfKind::Songs { tracks, rows: n } => {
                song_grid(ui, &rows, &app.liked, &mut app.actions, &id, &s.title, s.strapline.as_deref(), tracks, *n);
            }
        }
    }
}

pub(super) fn page_title(ui: &mut Ui, title: &str) {
    // Left-aligned with the page's content, a little below the top edge.
    ui.add_space(18.0);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 42.0), Sense::hover());
    ui.painter().text(pos2(rect.left(), rect.center().y), Align2::LEFT_CENTER, title, theme::bold(34.0), theme::text());
}

// ------------------------------------------------------------------ search

fn search(app: &mut App, ui: &mut Ui, q: &str, now: f64) {
    let pick = super::transfer::pick_banner(app, ui);
    page_title(ui, "Search");
    ui.add_space(10.0);
    // Coming from a suggestion, the sidebar or a link, the field says what's shown.
    // Only when the route changed: clearing the box leaves the route alone until Enter.
    if app.search_synced != q {
        app.search_synced = q.to_owned();
        app.search_text = q.trim().to_owned();
    }
    super::shell::search_field(app, ui, ui.available_width().min(560.0));
    // Where the search looks, as on the website: everywhere, or your own music.
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        for (scope, label) in [(crate::ytm::Scope::Everywhere, "YT Music"), (crate::ytm::Scope::Library, "Library"), (crate::ytm::Scope::Uploads, "Uploads")] {
            if widgets::chip(ui, label, app.search_scope == scope).clicked() && app.search_scope != scope {
                app.search_scope = scope;
                app.search = None;
            }
        }
    });
    if q.trim().is_empty() {
        ui.add_space(26.0);
        recent_searches(ui, app);
        return;
    }
    if app.search.as_ref().is_none_or(|r| r.query != q) {
        ui.add_space(14.0);
        placeholder(app, ui, now, 2, CARD);
        return;
    }
    let rows = Rows::new(app, now);
    // A song played from here starts its radio, as in YouTube Music, rather than the other
    // results queued after it; the one playing pauses or resumes.
    let current = app.current_id().map(str::to_owned);
    let play = |t: &Track| {
        if pick {
            Action::TransferPick(t.clone())
        } else if current.as_deref() == Some(t.id.as_str()) {
            Action::TogglePlay
        } else {
            Action::Radio(t.clone())
        }
    };
    let Some(r) = &app.search else { return };
    let acts = &mut app.actions;
    let liked = &app.liked;
    // Songs here can be dragged onto a playlist in the sidebar, but not into another order.
    let drag = &mut app.row_drag;
    let ids = |section: &str| egui::Id::unique(("search", section));
    let tracks: Vec<&Track> = r.tracks.iter().filter(|t| rows.shows(t)).collect();
    let videos: Vec<&Track> = r.videos.iter().filter(|t| rows.shows(t)).collect();
    let episodes: Vec<&Track> = r.episodes.iter().filter(|t| rows.shows(t)).collect();
    if tracks.is_empty() && videos.is_empty() && r.artists.is_empty() && r.albums.is_empty() && r.playlists.is_empty() && r.podcasts.is_empty() && episodes.is_empty() {
        ui.add_space(18.0);
        ui.label(RichText::new("No results").font(theme::bold(20.0)).color(theme::text()));
        return;
    }
    ui.add_space(14.0);
    // Top result: the first artist when the query names one, otherwise the first song.
    ui.horizontal_top(|ui| {
        if let Some(top) = r.artists.first().filter(|a| a.title.eq_ignore_ascii_case(q.trim())) {
            top_result(ui, top, false, false, acts);
        } else if let Some(t) = tracks.first().copied() {
            let c = Card {
                title: t.title.clone(),
                subtitle: format!("{} • {}", if t.music_video { "Video" } else { "Song" }, t.artist_line()),
                thumbs: t.thumbs.clone(),
                target: Target::Song(t.clone()),
                round: false,
                links: t.artists.clone(),
            };
            let liked_it = liked.contains(&t.id);
            top_result(ui, &c, pick, liked_it, acts);
        }
        ui.add_space(24.0);
        ui.vertical(|ui| {
            ui.label(RichText::new("Songs").font(theme::bold(24.0)));
            ui.add_space(8.0);
            for (i, t) in tracks.iter().copied().take(4).enumerate() {
                // Its own scope, so a picked-up row dims alone.
                ui.push_id(i, |ui| {
                    row_drag::grip(ui, drag, ids("songs"), i, t);
                    // The album beside the artist, as every other song row shows it.
                    let o = RowOpts { pick, ..rows.opts(liked, t, None, true) };
                    if track_row(ui, t, &o, acts).clicked() {
                        acts.push(play(t));
                    }
                });
            }
            row_drag::carry(ui, drag, ids("songs"));
        });
    });
    card_row(ui, "s-artists", "Artists", &r.artists, acts);
    card_row(ui, "s-albums", "Albums", &r.albums, acts);
    if !videos.is_empty() && !app.hide_search_videos {
        ui.add_space(26.0);
        ui.label(RichText::new("Videos").font(theme::bold(24.0)));
        ui.add_space(10.0);
        for (i, t) in videos.iter().copied().take(6).enumerate() {
            ui.push_id(("videos", i), |ui| {
                row_drag::grip(ui, drag, ids("videos"), i, t);
                let o = RowOpts { pick, ..rows.opts(liked, t, None, false) };
                if track_row(ui, t, &o, acts).clicked() {
                    acts.push(play(t));
                }
            });
        }
        row_drag::carry(ui, drag, ids("videos"));
    }
    if tracks.len() > 4 {
        ui.add_space(26.0);
        ui.label(RichText::new("More songs").font(theme::bold(24.0)));
        ui.add_space(10.0);
        for (i, t) in tracks.iter().copied().skip(4).enumerate() {
            ui.push_id(("more", i), |ui| {
                row_drag::grip(ui, drag, ids("more"), i, t);
                let o = RowOpts { pick, ..rows.opts(liked, t, None, true) };
                if track_row(ui, t, &o, acts).clicked() {
                    acts.push(play(t));
                }
            });
        }
        row_drag::carry(ui, drag, ids("more"));
    }
    card_row(ui, "s-pl", "Playlists", &r.playlists, acts);
    card_row(ui, "s-podcasts", "Podcasts", &r.podcasts, acts);
    if !episodes.is_empty() {
        ui.add_space(26.0);
        ui.label(RichText::new("Episodes").font(theme::bold(24.0)));
        ui.add_space(10.0);
        for (i, t) in episodes.iter().copied().take(6).enumerate() {
            ui.push_id(("episodes", i), |ui| {
                row_drag::grip(ui, drag, ids("episodes"), i, t);
                let o = RowOpts { pick, ..rows.opts(liked, t, None, false) };
                if track_row(ui, t, &o, acts).clicked() {
                    // An episode plays alone: a radio from it would queue songs after it.
                    acts.push(match play(t) {
                        Action::Radio(t) => Action::Play(vec![t], 0),
                        other => other,
                    });
                }
            });
        }
        row_drag::carry(ui, drag, ids("episodes"));
    }
}

/// Apple's search landing: what was opened from the last few searches, with its artwork.
fn recent_searches(ui: &mut Ui, app: &mut App) {
    if app.recent_searches.is_empty() {
        ui.label(RichText::new("Search for artists, songs, albums and playlists.").font(theme::regular(15.0)).color(theme::text_dim()));
        return;
    }
    ui.label(RichText::new("Recently Searched").font(theme::bold(22.0)).color(theme::text()));
    ui.add_space(12.0);
    let w = ui.available_width().min(620.0);
    // The rows are cloned so a removal can be applied after the list is drawn.
    let recents = app.recent_searches.clone();
    let mut remove = None;
    for (i, r) in recents.iter().enumerate() {
        let (rect, resp) = ui.allocate_exact_size(vec2(w, 64.0), Sense::click());
        let resp = named(resp, &r.title).on_hover_cursor(egui::CursorIcon::PointingHand);
        let h = theme::anim_bool(ui.ctx(), resp.id.with("h"), resp.contains_pointer(), 0.12);
        ui.painter().rect_filled(rect, 10.0, theme::glass(0.05 + 0.06 * h));
        let art = Rect::from_min_size(rect.left_top() + vec2(8.0, 8.0), Vec2::splat(48.0));
        let radius = if r.round { 24.0 } else { 6.0 };
        match r.thumb.as_deref().filter(|u| !u.is_empty()) {
            Some(url) => {
                let thumbs = [crate::model::Thumb { url: url.to_owned(), width: 300, height: 300 }];
                paint_cover(ui, art, &thumbs, radius, 1.0);
            }
            None => {
                ui.painter().rect_filled(art, radius, theme::glass(0.1));
            }
        }
        let x = art.right() + 14.0;
        widgets::text_at(ui, pos2(x, rect.center().y - 17.0), &r.title, theme::semibold(15.0), theme::text(), rect.right() - x - 12.0);
        widgets::text_at(ui, pos2(x, rect.center().y + 4.0), &r.subtitle, theme::regular(13.0), theme::text_dim(), rect.right() - x - 12.0);
        egui::Popup::context_menu(&resp).show(|ui| {
            ui.set_min_width(200.0);
            if ui.button("Remove from Recently Searched").clicked() {
                remove = Some(i);
                ui.close();
            }
        });
        if resp.clicked() {
            app.actions.push(match &r.target {
                Some(t) => Action::Open(t.clone()),
                // An entry from an earlier version kept only the search.
                None => Action::Go(Page::Search(r.query.clone())),
            });
        }
        ui.add_space(6.0);
    }
    if let Some(i) = remove {
        app.recent_searches.remove(i);
    }
}

/// `pick`: choosing a transfer match, so clicking a song picks it.
fn top_result(ui: &mut Ui, c: &Card, pick: bool, liked: bool, acts: &mut Vec<Action>) {    ui.vertical(|ui| {
        ui.label(RichText::new("Top result").font(theme::bold(24.0)));
        ui.add_space(8.0);
        let (rect, resp) = ui.allocate_exact_size(vec2(420.0, 236.0), Sense::click());
        let resp = named(resp, &c.title).on_hover_cursor(egui::CursorIcon::PointingHand);
        let h = theme::anim_bool(ui.ctx(), resp.id.with("h"), resp.contains_pointer(), 0.2);
        ui.painter().rect_filled(rect, 14.0, theme::glass(0.07 + 0.05 * h));
        let img = Rect::from_min_size(rect.min + vec2(20.0, 20.0), Vec2::splat(110.0));
        paint_cover(ui, img, &c.thumbs, if c.round { 55.0 } else { theme::card_radius() }, 1.0);
        let x = rect.left() + 20.0;
        widgets::text_at(ui, pos2(x, img.bottom() + 16.0), &c.title, theme::bold(26.0), theme::text(), rect.width() - 40.0);
        widgets::linked_text(ui, pos2(x, img.bottom() + 52.0), &c.subtitle, &c.links, theme::regular(14.0), theme::text_dim(), rect.width() - 40.0, acts);
        // Floating play button
        let pc = rect.right_bottom() + vec2(-44.0, -44.0 + 6.0 * (1.0 - h));
        let pr = Rect::from_center_size(pc, Vec2::splat(52.0));
        let active = widgets::active_state(ui, &c.target);
        let playing = active == Some(true);
        let label = format!("{} {}", if playing { "Pause" } else { "Play" }, c.title);
        let p = named(ui.interact(pr, resp.id.with("play"), Sense::click()), &label).on_hover_cursor(egui::CursorIcon::PointingHand);
        ui.painter().circle_filled(pc, 26.0, theme::with_alpha(Color32::WHITE, 0.4 + 0.6 * h));
        let nudge = if playing { 0.0 } else { 1.5 };
        widgets::play_pause_glyph(ui, pc + vec2(nudge, 0.0), 10.0, Color32::from_gray(15), playing);
        if p.clicked() {
            acts.push(match &c.target {
                _ if active.is_some() => Action::TogglePlay,
                Target::Song(t) => Action::Radio(t.clone()),
                t => Action::PlayCollection(t.clone(), false),
            });
        } else if resp.clicked() {
            acts.push(match &c.target {
                Target::Song(t) if pick => Action::TransferPick(t.clone()),
                t => Action::Open(t.clone()),
            });
        }
        // Right-click the box for the same menu the tile's ⋮ carries.
        egui::Popup::context_menu(&resp).show(|ui| widgets::card_menu_contents(ui, c, liked, None, acts));
    });
}

// ------------------------------------------------------------------ album / playlist

fn collection(app: &mut App, ui: &mut Ui, id: &str, now: f64) {
    if app.opened(id).is_none() {
        placeholder(app, ui, now, 1, 232.0);
        return;
    }
    let rows = Rows::new(app, now);
    // Whether every song of it is on this computer, and how far a download of it is.
    let downloaded = all_downloaded(app, id);
    let run = app.download_runs.iter().find(|r| r.key == id).map(|r| (r.done, r.total, r.cancelled));
    // The header's animated cover: the release's first song stands in for the album —
    // Apple files the release's clip with its songs, and the canvas finds it there.
    let px = (232.0 * ui.ctx().pixels_per_point()).round().max(256.0) as u32;
    let proxy = app.opened(id).and_then(|o| o.c.tracks.first().cloned());
    let sprite = proxy.and_then(|t| app.animated_cover(ui.ctx(), &t, px));
    let Some(o) = app.lists.get(id) else { return };
    let more = app.receiving(id);
    // Sorted once all of it is here: until then it plays as it arrives.
    app.sorts.hold(id, more);
    // A sort of it is under way. None starts while it loads (what's ticked may be stale).
    let sorting = app.sorting.contains_key(id);
    let busy = sorting || app.lists.loading(id);
    let (c, more) = (&o.c, more);
    let acts = &mut app.actions;
    let tracks = &c.tracks;
    let play_order = super::sorts::list_id(&app.page).and_then(|l| app.sorts.sorted(&l, tracks));
    let is_album = c.is_album;
    let filter_on = app.coll_search.as_ref().is_some_and(|(fid, _)| fid == id);
    let selecting = app.selection.on(&app.page);
    let mut toggle_search = false;
    let mut start_select = false;
    let mut stop_select = false;
    ui.add_space(18.0);
    ui.horizontal_top(|ui| {
        let (img, _) = ui.allocate_exact_size(Vec2::splat(232.0), Sense::hover());
        widgets::soft_shadow(ui, img, 10.0, 40.0, 0.5);
        paint_cover(ui, img, &c.thumbs, theme::card_radius() + 2.0, 1.0);
        if let Some(s) = &sprite {
            widgets::paint_motion(ui, img, theme::card_radius() + 2.0, s);
        }
        widgets::download_veil(ui, img, theme::card_radius() + 2.0, id);
        ui.add_space(26.0);
        ui.vertical(|ui| {
            ui.set_max_width(ui.available_width().min(760.0));
            ui.add_space(24.0);
            ui.label(RichText::new(if is_album { "ALBUM" } else if c.is_show() { "PODCAST" } else { "PLAYLIST" }).font(theme::semibold(12.0)).color(theme::text_dim()));
            ui.label(RichText::new(&c.title).font(theme::bold(40.0)).color(theme::text()));
            ui.add_space(2.0);
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::hover());
            widgets::linked_text(ui, r.min, &c.byline(), &c.artists, theme::regular(15.0), theme::text_dim(), r.width(), acts);
            let total: u32 = tracks.iter().filter_map(|t| t.duration).sum();
            if total > 0 {
                ui.label(
                    RichText::new(format!("{} {} • {} min", tracks.len(), c.things(), total / 60)).font(theme::regular(13.5)).color(theme::text_faint()),
                );
            }
            if let Some(d) = c.description.as_deref().filter(|d| !d.trim().is_empty()) {
                ui.add_space(6.0);
                let g = widgets::galley_wrapped(ui, d.trim(), theme::regular(13.5), theme::text_dim(), ui.available_width(), 2);
                ui.label(g);
            }
            ui.add_space(16.0);
            let target = if is_album { Target::Album(id.to_owned()) } else { Target::Playlist(id.to_owned()) };
            // Liked Music and Episodes for later are the account's own lists: they can't be
            // saved, removed, edited or deleted.
            let auto = matches!(id, "LM" | "SE");
            let saved = app.library.as_ref().is_some_and(|l| {
                let cards = if is_album { &l.albums } else { &l.playlists };
                cards.iter().any(|card| card.target == target)
            });
            // Saving only applies to someone else's playlist or an album.
            let save_id = if is_album { c.save_id.clone() } else { (!c.owned).then(|| id.to_owned()) };
            let owned = !is_album && c.owned;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = vec2(10.0, 0.0);
                play_pill(ui, acts, &target, tracks, play_order.as_deref());
                if circle_action(ui, CircleGlyph::Char(icon::SHUFFLE), "Shuffle", false).clicked() && !tracks.is_empty() {
                    let mut t = tracks.clone();
                    super::shuffle(&mut t);
                    acts.push(Action::Play(t, 0));
                }
                if !c.sorts.is_empty() && !more {
                    saved_sort_pill(ui, &mut app.sorts, id, &c.sorts, busy, sorting, acts);
                } else if !more {
                    sort_pill(ui, &mut app.sorts, &app.page, false);
                }
                if let Some((.., true)) = run {
                    // Cancelled: the songs on their way finish first.
                    pill(ui, "Stopping\u{2026}", None, false);
                } else if let Some((done, total, _)) = run {
                    if pill(ui, &format!("Downloading {done} of {total}"), Some(icon::X), false).on_hover_text("Cancel download").clicked() {
                        acts.push(Action::CancelDownload(id.to_owned()));
                    }
                } else if downloaded {
                    pill(ui, "Downloaded", Some(icon::CHECK), false).on_hover_text(format!("All {} are on this computer", c.things()));
                } else if !tracks.is_empty() && pill(ui, "Download", Some(icon::DOWNLOAD), false).clicked() {
                    // A whole list goes as it is; a partial one is fetched in full by the backend.
                    let complete = !more && !app.lists.loading(id);
                    acts.push(Action::DownloadMany(Some(target.clone()), c.title.clone(), complete.then(|| tracks.clone())));
                }
                if circle_action(ui, CircleGlyph::Char(icon::SEARCH), "Search in this list", filter_on).clicked() {
                    toggle_search = true;
                }
                let more = circle_action(ui, CircleGlyph::More, "More", false);
                egui::Popup::menu(&more).show(|ui| {
                    ui.set_width(226.0);
                    ui.add_space(3.0);
                    if let Some(first) = tracks.first()
                        && widgets::menu_action(ui, Some(icon::RADIO), "Start radio")
                    {
                        acts.push(Action::Radio(first.clone()));
                    }
                    if let Some(save_id) = save_id.clone()
                        && widgets::menu_action(ui, Some(icon::LIBRARY), if saved { "Remove from library" } else { "Save to library" })
                    {
                        acts.push(Action::SaveCollection(save_id, !saved));
                    }
                    if app.logged_in && !auto && !tracks.is_empty() {
                        if widgets::menu_action(ui, Some(icon::LIST_CHECKS), if selecting { "Done selecting" } else { "Select" }) {
                            if selecting { stop_select = true } else { start_select = true }
                        }
                    }
                    if app.logged_in && !auto && (save_id.is_some() || owned) {
                        ui.separator();
                    }
                    if owned {
                        if widgets::menu_action(ui, Some(icon::MUSIC), "Edit playlist") {
                            acts.push(Action::EditPlaylist);
                        }
                        if widgets::menu_action(ui, Some(icon::USERS), "Collaborate") {
                            acts.push(Action::Collaborate);
                        }
                        if widgets::menu_action(ui, Some(icon::TRASH_2), "Delete playlist") {
                            acts.push(Action::DeletePlaylist);
                        }
                    }
                    if app.logged_in && !auto && !owned && save_id.is_none() {
                        ui.separator();
                    }
                    if widgets::menu_action(ui, Some(icon::LINK), "Share") {
                        acts.push(Action::Share(target.clone()));
                    }
                    ui.add_space(3.0);
                });
            });
            if toggle_search {
                let closing = app.coll_search.as_ref().is_some_and(|(fid, _)| fid == id);
                app.coll_search = (!closing).then(|| (id.to_owned(), String::new()));
            }
            if start_select {
                app.selection.start(&app.page);
            }
            if stop_select {
                app.selection.clear();
            }

        });
    });
    let mut dismiss_search = false;
    if let Some((fid, query)) = &mut app.coll_search
        && fid == id
    {
        ui.add_space(14.0);
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width().min(420.0), 40.0), Sense::hover());
        ui.painter().rect_filled(rect, 20.0, theme::glass(0.09));
        widgets::icon_at(ui, pos2(rect.left() + 20.0, rect.center().y), icon::SEARCH, 16.0, theme::text_dim());
        let clearable = !query.is_empty();
        let inner = Rect::from_min_max(pos2(rect.left() + 40.0, rect.top() + 4.0), pos2(rect.right() - if clearable { 40.0 } else { 16.0 }, rect.bottom() - 4.0));
        let edit = ui.put(
            inner,
            egui::TextEdit::singleline(query)
                .hint_text(egui::RichText::new("Search in this list").color(theme::text_faint()))
                .font(theme::regular(14.0))
                .align(egui::Align2::LEFT_CENTER)
                .frame(egui::Frame::NONE)
                .desired_width(inner.width()),
        );
        if toggle_search {
            edit.request_focus();
        } else if !edit.has_focus() && query.is_empty() {
            // Left alone empty, the field goes away instead of sitting over the list.
            dismiss_search = true;
        }
        if clearable {
            let x = Rect::from_center_size(pos2(rect.right() - 20.0, rect.center().y), Vec2::splat(26.0));
            let resp = named(ui.interact(x, ui.scope_id().with("coll-clear"), Sense::click()), "Clear search").on_hover_cursor(egui::CursorIcon::PointingHand);
            widgets::icon_at(ui, x.center(), icon::X, 14.0, theme::text_dim());
            if resp.clicked() {
                query.clear();
            }
        }
        ui.add_space(18.0);
    } else {
        ui.add_space(26.0);
    }
    if dismiss_search {
        app.coll_search = None;
    }
    let needle = app.coll_search.as_ref().filter(|(fid, _)| fid == id).map(|(_, q)| q.trim().to_lowercase()).unwrap_or_default();
    let filtered: Vec<Track> = if needle.is_empty() {
        Vec::new()
    } else {
        c.tracks
            .iter()
            .filter(|t| {
                t.title.to_lowercase().contains(&needle)
                    || t.artist_line().to_lowercase().contains(&needle)
                    || t.album.as_ref().is_some_and(|a| a.name.to_lowercase().contains(&needle))
            })
            .cloned()
            .collect()
    };
    let shown: &[Track] = if needle.is_empty() { tracks } else { &filtered };
    let order = super::sorts::list_id(&app.page).and_then(|l| app.sorts.order(&l, shown));
    let number = |i: usize, t: &Track| is_album.then(|| t.track_nr.map_or(i + 1, |n| n as usize));
    let own = app.logged_in && c.owned && !is_album;
    list(ui, &rows, &app.liked, shown, order.as_deref(), !is_album, number, ListCtx { sel: &mut app.selection, page: &app.page, own }, &mut app.row_drag, egui::Id::unique(("list", id)), acts);
    if more {
        widgets::skeleton_rows(ui, now, 3);
    }
}

/// Whether every song of a fully loaded album or playlist is downloaded; worked out once
/// per list (its songs and the downloads folder) and kept until a download changes it.
fn all_downloaded(app: &mut App, id: &str) -> bool {
    use std::hash::{Hash, Hasher};
    let Some(o) = app.lists.get(id) else { return false };
    // Not while it loads or downloads: its files land one by one, and the run says how far it is.
    if app.lists.receiving(id) || app.lists.loading(id) || app.download_runs.iter().any(|r| r.key == id) {
        return false;
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    crate::downloads::folder().hash(&mut h);
    crate::downloads::looks().hash(&mut h);
    o.c.tracks.iter().for_each(|t| t.id.hash(&mut h));
    let seen = h.finish();
    if let Some(&(s, all)) = app.all_downloaded.get(id)
        && s == seen
    {
        return all;
    }
    let all = !o.c.tracks.is_empty() && crate::downloads::all_listed(&o.c.tracks);
    app.all_downloaded.insert(id.to_owned(), (seen, all));
    all
}

/// The cover of an album or playlist being downloaded, from its kept copy or its card.
fn run_thumbs<'a>(app: &'a App, key: &str) -> &'a [crate::model::Thumb] {
    if let Some(o) = app.opened(key) {
        return &o.c.thumbs;
    }
    let lib = app.library.iter().flat_map(|l| l.playlists.iter().chain(&l.albums));
    lib.chain(app.sidebar_cache.iter().flatten())
        .find(|c| matches!(&c.target, Target::Album(id) | Target::Playlist(id) if id == key))
        .map_or(&[], |c| &c.thumbs)
}

/// A download of many songs in the Downloads page: its cover (veiled as it goes), its
/// name, how far it's got, and a button to cancel it. True when cancelled.
fn download_run(ui: &mut Ui, r: &crate::backend::DownloadRun, thumbs: &[crate::model::Thumb], width: f32) -> bool {
    let (rect, _) = ui.allocate_exact_size(vec2(width, 64.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 10.0, theme::glass(0.04));
    let img = Rect::from_min_size(rect.min + vec2(10.0, 10.0), Vec2::splat(44.0));
    if r.title.is_empty() {
        p.rect_filled(img, theme::thumb_radius(), theme::glass(0.07));
        widgets::icon_at(ui, img.center(), icon::DOWNLOAD, 20.0, theme::text_dim());
    } else {
        paint_cover(ui, img, thumbs, theme::thumb_radius(), 1.0);
        widgets::download_veil(ui, img, theme::thumb_radius(), &r.key);
    }
    let close = Rect::from_center_size(pos2(rect.right() - 26.0, rect.center().y), Vec2::splat(32.0));
    let x = img.right() + 14.0;
    let w = close.left() - 14.0 - x;
    let title = if r.title.is_empty() { "Selected songs" } else { &r.title };
    widgets::text_at(ui, pos2(x, rect.top() + 11.0), title, theme::semibold(14.0), theme::text(), w);
    widgets::text_at(ui, pos2(x, rect.top() + 30.0), &if r.cancelled { "Stopping\u{2026}".to_owned() } else { format!("{} of {} songs", r.done, r.total) }, theme::regular(12.5), theme::text_dim(), w);
    let bar = Rect::from_min_size(pos2(x, rect.bottom() - 12.0), vec2(w, 3.0));
    let f = theme::anim(ui.ctx(), ("download-run-bar", &r.key), r.done as f32 / r.total.max(1) as f32, 0.4);
    let accent = ui.ctx().data(|d| d.get_temp::<Color32>(egui::Id::unique("accent"))).unwrap_or(theme::RED);
    p.rect_filled(bar, 1.5, theme::glass(0.1));
    p.rect_filled(Rect::from_min_size(bar.min, vec2(w * f, bar.height())), 1.5, accent);
    if r.cancelled {
        return false;
    }
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(close));
    widgets::icon_button(&mut child, icon::X, 16.0, "Cancel download", false).clicked()
}

// ------------------------------------------------------------------ artist

fn artist(app: &mut App, ui: &mut Ui, id: &str, now: f64) {
    if app.artist.as_ref().is_none_or(|a| a.id != id) {
        placeholder(app, ui, now, 2, CARD);
        return;
    }
    let rows = Rows::new(app, now);
    let accent = app.accent_color();
    let subscribing = app.subscriptions.busy();
    let name = app.artist.as_ref().map(|a| a.name.clone()).unwrap_or_default();
    // The listener's signature service stands in for the name where it has one.
    let signature = app.signature_texture(ui.ctx(), &name);
    let Some(a) = &app.artist else { return };
    let top: Vec<Track> = a.top.iter().filter(|t| rows.shows(t)).cloned().collect();
    let top = &top;
    // The hero, full-bleed at the header aspect the artist pages use (about 1.93:1),
    // fading into the page at the bottom. YouTube's banner draws it — out to the
    // window's own edges: no gutter against the sidebar, none above or right.
    let w = ui.available_width();
    let screen = ui.ctx().input(|i| i.raw.screen_rect.unwrap_or(ui.clip_rect()));
    let screen_h = screen.height();
    let hero_h = (w / 1.93).clamp(240.0, (screen_h * 0.7).max(380.0));
    let (hero, _) = ui.allocate_exact_size(vec2(w, hero_h), Sense::hover());
    let clip = ui.clip_rect();
    // The scroll's clip already spans the panel — its left is the sidebar's own edge,
    // so crossing a page gutter again would paint over the sidebar — and on the
    // right the content's gutter (the set_max_width in `page`) sits inside it. The
    // title strip above is crossed.
    let bleed = Rect::from_min_max(pos2(clip.left(), hero.top() - (clip.top() - screen.top())), pos2(clip.right(), hero.bottom()));
    let outer = ui.clip_rect();
    ui.set_clip_rect(Rect::from_min_max(pos2(bleed.left(), screen.top()), pos2(bleed.right(), clip.bottom())));
    if let Some(t) = a.banner.iter().max_by_key(|t| t.width) {
        let img_ratio = if t.height > 0 { t.width as f32 / t.height as f32 } else { 16.0 / 9.0 };
        // Cover-fit: crop the image's top band to the hero's aspect.
        let vis_h = (img_ratio / (bleed.width() / bleed.height())).min(1.0);
        let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, vis_h));
        ui.painter().rect_filled(bleed, 0.0, theme::glass(0.07));
        widgets::paint_remote(ui, &t.url, bleed, 0.0, uv, Color32::WHITE);
    } else {
        ui.painter().rect_filled(bleed, 0.0, theme::with_alpha(accent, 0.25));
    }
    // The banner melts into the page on every side — a header wash, not a photograph
    // pasted over it — and the name sits in the foot ramp's dark, where the old shade
    // used to cut the image off hard. The ramp takes the page's own colour at the
    // banner's foot: the playing cover's, so the blend follows the background's art
    // instead of sitting as a black band over it.
    let fy = ((bleed.bottom() - screen.top()) / screen_h).clamp(0.0, 1.0);
    widgets::melt_edges(ui, bleed, super::shell::page_line_color(app, fy), 110.0, 64.0, bleed.height() * 0.55);
    // The melt's own ramp lands a shade short of the page, leaving the banner's foot to
    // read as a line. A tight second pass takes the last stretch the rest of the way,
    // and a wash of the same colour bleeds below the edge, so the header marries the
    // background under it instead of ending against it.
    let foot = super::shell::page_line_color(app, fy);
    super::shell::vgradient(ui.painter(), Rect::from_min_max(pos2(bleed.left(), bleed.bottom() - 72.0), pos2(bleed.right(), bleed.bottom())), theme::with_alpha(foot, 0.0), foot);
    super::shell::vgradient(ui.painter(), Rect::from_min_max(pos2(bleed.left(), bleed.bottom()), pos2(bleed.right(), bleed.bottom() + 110.0)), foot, theme::with_alpha(foot, 0.0));
    let x = bleed.left() + PAGE_INSET as f32;
    // The buttons ride the banner's foot, centred on the page and cut exactly in half
    // by its bottom edge — half over the art, half over the melt — which leaves the art
    // above them to the name. The website's play circle in the accent, with the shuffle
    // and the subscribe at its sides as the player's own transparent circles — the
    // subscribe wearing the songs' star.
    let acts = &mut app.actions;
    let play_r = 32.0;
    let side_r = 22.0;
    let cy = bleed.bottom();
    let gap = play_r + side_r + 12.0;
    let cx = bleed.center().x;
    // The transparent discs take their colour from the art they sit on; the glyphs stay
    // white on the dark pages, the theme's own ink on light ones.
    let ink = if theme::light() { theme::text() } else { Color32::WHITE };
    let active = widgets::active_state(ui, &Target::Artist(id.to_owned()));
    let (play_glyph, play_name) = if active == Some(true) { (icon::PAUSE, "Pause") } else { (icon::PLAY, "Play") };
    let prect = Rect::from_center_size(pos2(cx, cy), Vec2::splat(play_r * 2.0));
    let presp = ui.interact(prect, egui::Id::unique(("artist-play", id)), Sense::click());
    let presp = widgets::named(presp, play_name).on_hover_cursor(egui::CursorIcon::PointingHand);
    let h = theme::anim_bool(ui.ctx(), presp.id.with("h"), presp.hovered(), 0.15);
    let pr = prect.shrink(if presp.is_pointer_button_down_on() { 1.0 } else { 0.0 });
    ui.painter().circle_filled(pr.center(), pr.width() / 2.0, theme::lerp_color(accent, Color32::WHITE, 0.1 * h));
    widgets::icon_at(ui, pr.center(), play_glyph, 26.0, Color32::WHITE);
    if presp.clicked() {
        if active.is_some() {
            acts.push(Action::TogglePlay);
        } else if !top.is_empty() {
            // All of it from the top.
            acts.push(Action::Play(top.to_vec(), 0));
        }
    }
    let srect = Rect::from_center_size(pos2(cx - gap, cy), Vec2::splat(side_r * 2.0));
    let sresp = ui.interact(srect, egui::Id::unique(("artist-shuffle", id)), Sense::click());
    let sresp = widgets::named(sresp, "Shuffle").on_hover_cursor(egui::CursorIcon::PointingHand);
    let sh = theme::anim_bool(ui.ctx(), sresp.id.with("h"), sresp.hovered(), 0.15);
    let sr = srect.shrink(if sresp.is_pointer_button_down_on() { 1.0 } else { 0.0 });
    if sh > 0.0 {
        ui.painter().circle_filled(sr.center(), sr.width() / 2.0 + 1.5 * sh, theme::glass(0.1 * sh));
    }
    widgets::icon_at(ui, sr.center(), icon::SHUFFLE, 17.0, ink);
    if sresp.clicked() && !top.is_empty() {
        let mut t = top.clone();
        super::shuffle(&mut t);
        acts.push(Action::Play(t, 0));
    }
    // The subscribe, the songs' star over the transparent disc: filled in the cover's
    // accent while subscribed, with the little pop the player bar's star turns with.
    if a.channel_id.is_some() {
        let rect = Rect::from_center_size(pos2(cx + gap, cy), Vec2::splat(side_r * 2.0));
        ui.add_enabled_ui(!subscribing, |ui| {
            let resp = ui.interact(rect, egui::Id::unique(("artist-sub", id)), Sense::click());
            let resp = widgets::named(resp, if a.subscribed { "Subscribed" } else { "Subscribe" }).on_hover_cursor(egui::CursorIcon::PointingHand);
            let h = theme::anim_bool(ui.ctx(), resp.id.with("h"), resp.hovered(), 0.15);
            let pop = theme::anim_bool(ui.ctx(), egui::Id::unique(("artist-subbed", id)), a.subscribed, 0.3);
            let r = rect.shrink(if resp.is_pointer_button_down_on() { 1.0 } else { 0.0 });
            if h > 0.0 {
                ui.painter().circle_filled(r.center(), r.width() / 2.0 + 1.5 * h, theme::glass(0.1 * h));
            }
            let size = 16.0 * (1.0 + 0.25 * (pop * std::f32::consts::PI).sin());
            super::glyphs::draw(ui, r.center(), size, super::glyphs::STAR, ink);
            if pop > 0.0 {
                super::glyphs::draw(ui, r.center(), size, super::glyphs::STAR_FILLED, theme::with_alpha(accent, pop));
            }
            if resp.clicked() {
                acts.push(Action::Subscribe(!a.subscribed));
            }
        });
    }
    // With the buttons out of the art, the name and its subscriber count settle toward
    // the foot's own inset, clear of the circles riding the edge.
    let base = pos2(x, bleed.bottom() - 26.0);
    match &signature {
        // The artist's own hand, credited above it the way a page signs itself off.
        Some(tex) => {
            let tex_size = tex.size_vec2();
            let aspect = tex_size.x / tex_size.y.max(1.0);
            let mut size = vec2(96.0 * aspect, 96.0);
            let max_w = (w * 0.42).min(340.0);
            if size.x > max_w {
                size = vec2(max_w, max_w / aspect);
            }
            let rect = Rect::from_min_size(pos2(base.x, base.y - 40.0 - size.y), size);
            ui.painter().text(pos2(base.x, rect.top() - 8.0), Align2::LEFT_BOTTOM, format!("Signed by {name}"), theme::regular(12.0), Color32::from_white_alpha(199));
            let tex = egui::load::SizedTexture::from_handle(tex);
            egui::Image::from_texture(tex).uv(Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))).paint_at(ui, rect);
        }
        None => {
            ui.painter().text(base - vec2(0.0, 44.0), Align2::LEFT_BOTTOM, &name, theme::bold(58.0), Color32::WHITE);
        }
    }
    if let Some(s) = a.subscribers {
        ui.painter().text(base - vec2(0.0, 18.0), Align2::LEFT_BOTTOM, format!("{} subscribers", compact(s)), theme::regular(14.0), theme::text_dim());
    }
    // The radio, pinned to the image's bottom-right edge where the website keeps its
    // corner button. The About card stacks above it, ending short of it.
    let mut radio_top = None;
    if let Some(first) = top.first() {
        let text = ui.painter().layout_no_wrap("Radio".to_owned(), theme::semibold(14.0), Color32::PLACEHOLDER);
        let rw = text.size().x + 24.0 + 36.0;
        let rect = Rect::from_min_size(pos2(bleed.right() - PAGE_INSET as f32 - rw, bleed.bottom() - 24.0 - 38.0), vec2(rw, 38.0));
        if widgets::pill_at(ui, rect, "artist-radio", "Radio", Some(icon::RADIO), false).clicked() {
            acts.push(Action::Radio(first.clone()));
        }
        radio_top = Some(rect.top());
    }
    // The About, stacked over the radio in the banner's bottom-right corner: a few rows
    // ending in an ellipsis, the whole card — sized to its text, no pane above the rows
    // for the hover to light up — a click into the full text (with the channel's
    // picture) that used to close the page.
    if let Some(d) = a.description.as_deref().filter(|d| !d.trim().is_empty())
        && bleed.width() >= 640.0
    {
        let aw = (bleed.width() * 0.26).clamp(220.0, 340.0);
        let full = widgets::galley_wrapped(ui, d.trim(), theme::regular(14.0), theme::with_alpha(Color32::WHITE, 0.88), aw, usize::MAX);
        let g = widgets::galley_wrapped(ui, d.trim(), theme::regular(14.0), theme::with_alpha(Color32::WHITE, 0.88), aw, 4);
        let g_size = g.size().y;
        // "More" only says something when rows were actually cut.
        let cut = full.size().y > g_size + 1.0;
        let block_h = 24.0 + g_size + if cut { 30.0 } else { 0.0 };
        // The card sits on the radio's head (the foot ramp's own inset where the radio
        // isn't), right-flush with it, and stays clear of the name running the left
        // flank at the same height.
        let abottom = radio_top.map_or(bleed.bottom() - 24.0, |t| t - 12.0);
        let aright = bleed.right() - PAGE_INSET as f32;
        let aleft = aright - aw;
        let name_w = ui.painter().layout_no_wrap(name.clone(), theme::bold(58.0), Color32::PLACEHOLDER).size().x;
        if aleft > x + name_w + 30.0 {
            let arect = Rect::from_min_max(pos2(aleft, (abottom - block_h).max(bleed.top() + 56.0)), pos2(aright, abottom));
            let resp = widgets::named(ui.interact(arect, egui::Id::unique(("artist-about", id)), Sense::click()), "About this artist")
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            let h = theme::anim_bool(ui.ctx(), resp.id.with("h"), resp.hovered(), 0.15);
            if h > 0.0 {
                ui.painter().rect_filled(arect, 14.0, theme::glass(0.12 * h));
            }
            // The card is its text: the ABOUT header at the top, the rows under it, the
            // More that opens it all at the foot.
            let ty = (arect.bottom() - block_h).max(arect.top());
            ui.painter().text(pos2(arect.left(), ty), Align2::LEFT_TOP, "ABOUT", theme::semibold(11.5), theme::with_alpha(Color32::WHITE, 0.72));
            ui.painter().galley(pos2(arect.left(), ty + 24.0), g, Color32::PLACEHOLDER);
            if cut {
                ui.painter().text(pos2(arect.left(), ty + 24.0 + g_size + 12.0), Align2::LEFT_TOP, "More", theme::semibold(13.5), theme::with_alpha(Color32::WHITE, 0.85));
            }
            if resp.clicked() {
                // The picture rides along when the channel has one (the microformat's);
                // without it the card still opens — the description is the point.
                let avatar = crate::model::pick(&a.avatar, 120).map(str::to_owned);
                app.dialog = Some(Dialog::ArtistAbout { name: name.clone(), description: d.trim().to_owned(), avatar, subscribers: a.subscribers });
            }
        }
    }
    ui.set_clip_rect(outer);
    // An artist of your uploads: all your songs by them, as on the website.
    let uploaded = crate::ytm::uploaded(id);
    if !top.is_empty() {
        ui.add_space(26.0);
        ui.horizontal(|ui| {
            // The website's heading: the arrow after the title opens what the Show all
            // pill listed, and the title clicks with it. With a "Top songs" playlist
            // behind the shelf it opens; without one, the shelf page stands in and
            // lists everything the page holds.
            if widgets::heading_link(ui, if uploaded { "Songs" } else { "Top songs" }, theme::bold(24.0)) {
                match a.top_playlist.clone() {
                    Some(pl) => acts.push(Action::Open(Target::Playlist(pl))),
                    None => acts.push(Action::Go(Page::ArtistShelf { artist: id.to_owned(), title: super::SONGS_SHELF.to_owned() })),
                }
            }
        });
        ui.add_space(12.0);
        // They can be dragged onto a playlist in the sidebar.
        let drag = &mut app.row_drag;
        let list = egui::Id::unique(("artist-top", id));
        for (i, t) in top.iter().enumerate().take(if uploaded { top.len() } else { 5 }) {
            // Its own scope, so a picked-up row dims alone.
            ui.push_id(i, |ui| {
                row_drag::grip(ui, drag, list, i, t);
                let o = rows.opts(&app.liked, t, None, true);
                if track_row(ui, t, &o, acts).clicked() {
                    acts.push(Action::Play(top.clone(), i));
                }
            });
        }
        row_drag::carry(ui, drag, list);
    }
    // Albums and singles keep their own shelves now, each with its Show all into the
    // artist's full discography for that kind of release.
    card_row_all(ui, "a-albums", "Albums", &a.albums, a.albums_more.is_some(), acts, || Action::Go(Page::ArtistShelf { artist: id.to_owned(), title: "Albums".to_owned() }));
    card_row_all(ui, "a-singles", "Singles", &a.singles, a.singles_more.is_some(), acts, || Action::Go(Page::ArtistShelf { artist: id.to_owned(), title: super::SINGLES_SHELF.to_owned() }));
    // Their songs you've saved, less those hidden as explicit.
    let library: Vec<Card> = a.library.iter().filter(|c| !matches!(&c.target, Target::Song(t) if !rows.shows(t))).cloned().collect();
    if !library.is_empty() {
        card_row(ui, "a-lib", "From your library", &library, acts);
    }
    card_row(ui, "a-pl", "Featured on", &a.playlists, acts);
    card_row(ui, "a-sim", "Fans might also like", &a.similar, acts);
}

/// An artist shelf with everything on it (a Show all): the artist's releases for one
/// kind (`cards`), or every song the page listed (`tracks`). The cards open like any
/// shelf's; the songs play in order, like a playlist's rows do.
fn artist_shelf(app: &mut App, ui: &mut Ui, artist: &str, title: &str, now: f64) {
    let Some(s) = app.shelf.as_ref().filter(|s| s.key == (artist.to_owned(), title.to_owned())) else {
        placeholder(app, ui, now, 1, CARD);
        return;
    };
    if s.loading && s.cards.is_empty() && s.tracks.is_none() {
        placeholder(app, ui, now, 1, CARD);
        return;
    }
    let rows = Rows::new(app, now);
    let acts = &mut app.actions;
    ui.add_space(6.0);
    // The header: the shelf's name over the artist, whose name goes back to their page.
    ui.label(RichText::new(s.title.to_uppercase()).font(theme::semibold(12.0)).color(theme::text_dim()));
    let (name_rect, name_r) = ui.allocate_exact_size(vec2(ui.available_width().min(560.0), 46.0), Sense::click());
    ui.painter().text(name_rect.left_center(), Align2::LEFT_CENTER, &s.artist_name, theme::bold(40.0), theme::text());
    if name_r.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if name_r.clicked() {
        acts.push(Action::Go(Page::Artist(artist.to_owned())));
    }
    let (count, songs) = match &s.tracks {
        Some(t) => (format!("{} songs", t.len()), true),
        None => (format!("{} releases", s.cards.len()), false),
    };
    ui.label(RichText::new(count).font(theme::regular(13.5)).color(theme::text_faint()));
    ui.add_space(16.0);
    if songs {
        let Some(tracks) = &s.tracks else { return };
        if !tracks.is_empty() {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                if pill(ui, "Play", Some(icon::PLAY), true).clicked() {
                    acts.push(Action::Play(tracks.clone(), 0));
                }
                if pill(ui, "Shuffle", Some(icon::SHUFFLE), false).clicked() {
                    let mut t = tracks.clone();
                    super::shuffle(&mut t);
                    acts.push(Action::Play(t, 0));
                }
            });
            ui.add_space(14.0);
        }
        let liked = &app.liked;
        let plain = |_: usize, _: &Track| None;
        let mut sel = std::mem::take(&mut app.selection);
        let page = app.page.clone();
        let cx = ListCtx { sel: &mut sel, page: &page, own: false };
        let clicked = list(ui, &rows, liked, tracks, None, true, plain, cx, &mut app.row_drag, egui::Id::unique(("shelf", app.page.clone())), acts);
        app.selection = sel;
        let _ = clicked;
    } else if !s.cards.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(20.0, 22.0);
            for c in &s.cards {
                card(ui, c, CARD, false, None, acts);
            }
        });
    }
    if s.loading {
        ui.add_space(10.0);
        ui.spinner();
    }
}

// ------------------------------------------------------------------ library

fn library(app: &mut App, ui: &mut Ui, tab: LibTab, now: f64) {
    page_title(ui, "Library");
    // The Uploads tab holds this computer's songs too, below the account's: it opens
    // signed out, with its upload half asking for a sign-in.
    if !app.logged_in && tab != LibTab::Uploads {
        ui.add_space(10.0);
        ui.label(RichText::new("Sign in to see your playlists, liked songs and history.").color(theme::text_dim()));
        ui.add_space(12.0);
        if pill(ui, "Sign in with Google", Some(icon::CIRCLE_USER), true).clicked() {
            app.start_sign_in(ui.ctx());
        }
        return;
    }
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        let podcasts = app.show_podcasts;
        for (t, label) in [(LibTab::Playlists, "Playlists"), (LibTab::Songs, "Liked songs"), (LibTab::Albums, "Albums"), (LibTab::Artists, "Artists"), (LibTab::Podcasts, "Podcasts"), (LibTab::History, "History"), (LibTab::Uploads, "Uploads")].into_iter().filter(|(t, _)| podcasts || *t != LibTab::Podcasts) {
            if chip(ui, label, tab == t).clicked() {
                // Tabs swap in place (no history entry); the current one refreshes.
                if tab == t {
                    app.refresh();
                } else {
                    app.page = Page::Library(t);
                    app.page_loaded();
                }
            }
        }
    });
    ui.add_space(12.0);
    // `pinned`: on Library › Playlists, where playlists are pinned.
    let grid = |ui: &mut Ui, cards: &[Card], order: &[usize], pinned: Option<&[String]>, acts: &mut Vec<Action>| {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(20.0, 22.0);
            for &i in order {
                let pin = pinned.map(|p| matches!(&cards[i].target, Target::Playlist(id) if p.contains(id)));
                card(ui, &cards[i], CARD, false, pin, acts);
            }
        });
    };
    let rows = Rows::new(app, now);
    let plain = |_: usize, _: &Track| None;
    match tab {
        LibTab::History => {
            if app.history.is_none() {
                placeholder(app, ui, now, 1, 48.0);
                return;
            }
            let Some(h) = &app.history else { return };
            ui.horizontal(|ui| {
                sort_pill(ui, &mut app.sorts, &app.page, false);
            });
            ui.add_space(10.0);
            let order = app.sorts.order("history", h);
            list(ui, &rows, &app.liked, h, order.as_deref(), true, plain, ListCtx { sel: &mut app.selection, page: &app.page, own: false }, &mut app.row_drag, egui::Id::unique("history"), &mut app.actions);
        }
        LibTab::Uploads => uploads_tab(app, ui, now, &rows),
        // Liked Music, kept like any playlist.
        LibTab::Songs => {
            if app.opened(super::LIKED_MUSIC).is_none() {
                placeholder(app, ui, now, 1, 48.0);
                return;
            }
            let more = app.receiving(super::LIKED_MUSIC);
            let Some(o) = app.lists.get(super::LIKED_MUSIC) else { return };
            let liked = &o.c.tracks;
            if !liked.is_empty() {
                ui.horizontal(|ui| {
                    if pill(ui, "Play", Some(icon::PLAY), true).clicked() {
                        // All of it from the top, in the order it's sorted.
                        let plays = match app.sorts.sorted(super::LIKED_MUSIC, liked) {
                            Some(o) => o.iter().map(|&i| liked[i].clone()).collect(),
                            None => liked.clone(),
                        };
                        app.actions.push(Action::Play(plays, 0));
                    }
                    if pill(ui, "Shuffle", Some(icon::SHUFFLE), false).clicked() {
                        let mut t = liked.clone();
                        super::shuffle(&mut t);
                        app.actions.push(Action::Play(t, 0));
                    }
                    select_pill(ui, &mut app.selection, &app.page);
                    sort_pill(ui, &mut app.sorts, &app.page, more);
                });
                ui.add_space(10.0);
            }
            let order = app.sorts.order(super::LIKED_MUSIC, liked);
            list(ui, &rows, &app.liked, liked, order.as_deref(), true, plain, ListCtx { sel: &mut app.selection, page: &app.page, own: false }, &mut app.row_drag, egui::Id::unique("liked"), &mut app.actions);
            if more {
                widgets::skeleton_rows(ui, now, 3);
            }
        }
        _ => {
            if app.library.is_none() {
                placeholder(app, ui, now, 2, CARD);
                return;
            }
            let Some(lib) = &app.library else { return };
            if matches!(tab, LibTab::Playlists | LibTab::Albums | LibTab::Artists | LibTab::Podcasts) {
                ui.horizontal(|ui| {
                    sort_pill(ui, &mut app.sorts, &app.page, false);
                });
                ui.add_space(10.0);
            }
            match tab {
                LibTab::Playlists => {
                    if pill(ui, "New playlist", None, false).clicked() {
                        app.actions.push(Action::NewPlaylist(Vec::new()));
                    }
                    ui.add_space(14.0);
                    let pinned = app.pinned.as_slice();
                    let order = super::collections::pinned_first(pinned, &lib.playlists, app.sorts.cards("playlists", &lib.playlists));
                    grid(ui, &lib.playlists, &order, Some(pinned), &mut app.actions)
                }
                LibTab::Albums => grid(ui, &lib.albums, &app.sorts.cards("albums", &lib.albums), None, &mut app.actions),
                LibTab::Artists => grid(ui, &lib.artists, &app.sorts.cards("artists", &lib.artists), None, &mut app.actions),
                LibTab::Podcasts => grid(ui, &lib.podcasts, &app.sorts.cards("podcasts", &lib.podcasts), None, &mut app.actions),
                LibTab::Songs | LibTab::History | LibTab::Uploads => unreachable!(),
            }
        }
    }
}

// ------------------------------------------------------------------ downloads

/// Library › Uploads: two ways to have songs of your own. Uploading puts them in the
/// account's cloud library — playable on any device, but YouTube Music converts what it
/// takes, so a FLAC doesn't stay lossless. Local music plays straight from this
/// computer, each file as it is.
fn uploads_tab(app: &mut App, ui: &mut Ui, now: f64, rows: &Rows) {
    let plain = |_: usize, _: &Track| None;
    ui.label(RichText::new("Upload to YouTube Music").font(theme::bold(15.0)).color(theme::text()));
    ui.add_space(4.0);
    ui.label(RichText::new("Songs go into the account's cloud library, playable on any device. YouTube Music converts what it takes — MP3, M4A, FLAC, OGG and WMA — so a FLAC doesn't stay lossless.").font(theme::regular(13.0)).color(theme::text_dim()));
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        if pill(ui, "Upload to YouTube Music", Some(icon::UPLOAD), true).clicked() {
            app.pick_uploads(ui.ctx());
        }
        if app.uploads.as_ref().is_some_and(|u| !u.is_empty()) && pill(ui, "Shuffle", Some(icon::SHUFFLE), false).clicked() {
            let mut t = app.uploads.clone().unwrap_or_default();
            super::shuffle(&mut t);
            app.actions.push(Action::Play(t, 0));
        }
    });
    ui.add_space(12.0);
    if !app.logged_in {
        ui.label(RichText::new("Sign in to upload. Local music below works without an account.").font(theme::regular(13.0)).color(theme::text_dim()));
    } else if app.uploads.is_none() {
        placeholder(app, ui, now, 1, 48.0);
    } else if let Some(up) = &app.uploads {
        if up.is_empty() {
            ui.add_space(8.0);
            ui.label(RichText::new("No uploads yet").color(theme::text_dim()));
        } else {
            ui.horizontal(|ui| {
                sort_pill(ui, &mut app.sorts, &app.page, false);
            });
            ui.add_space(10.0);
        }
        let order = app.sorts.order("uploads", up);
        list(ui, rows, &app.liked, up, order.as_deref(), true, plain, ListCtx { sel: &mut app.selection, page: &app.page, own: false }, &mut app.row_drag, egui::Id::unique("uploads"), &mut app.actions);
    }
    ui.add_space(30.0);
    ui.label(RichText::new("Local music").font(theme::bold(15.0)).color(theme::text()));
    ui.add_space(4.0);
    ui.label(RichText::new("Songs from this computer, played straight from their files: nothing is uploaded, and a FLAC plays as the FLAC it is. Add files or a folder, or drop them anywhere on the window.").font(theme::regular(13.0)).color(theme::text_dim()));
    ui.add_space(10.0);
    let empty = app.local.is_empty();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        if pill(ui, "Add music", Some(icon::MUSIC), false).clicked() {
            app.pick_local(ui.ctx());
        }
        if pill(ui, "Add folder", Some(icon::FOLDER_OPEN), false).clicked() {
            app.pick_local_folder(ui.ctx());
        }
        if !empty {
            if pill(ui, "Play", Some(icon::PLAY), true).clicked() {
                // All of them from the top, in the order it's sorted.
                let plays = match app.sorts.sorted("local", &app.local) {
                    Some(o) => o.iter().map(|&i| app.local[i].clone()).collect(),
                    None => app.local.clone(),
                };
                app.actions.push(Action::Play(plays, 0));
            }
            if pill(ui, "Shuffle", Some(icon::SHUFFLE), false).clicked() {
                let mut t = app.local.clone();
                super::shuffle(&mut t);
                app.actions.push(Action::Play(t, 0));
            }
            sort_pill_list(ui, &mut app.sorts, &app.page, "local", false);
        }
    });
    ui.add_space(12.0);
    if empty {
        ui.label(RichText::new("No local music yet").color(theme::text_dim()));
        return;
    }
    let order = app.sorts.order("local", &app.local);
    // Its rows pick nothing: the Select bar belongs to the uploads above, whose rows
    // are a different list.
    let mut no_sel = Selection::default();
    list(ui, rows, &app.liked, &app.local, order.as_deref(), true, plain, ListCtx { sel: &mut no_sel, page: &app.page, own: false }, &mut app.row_drag, egui::Id::unique("local"), &mut app.actions);
}

fn downloads(app: &mut App, ui: &mut Ui, now: f64) {
    page_title(ui, "Downloads");
    let Some(songs) = &app.downloads else { return };
    ui.add_space(4.0);
    ui.label(
        RichText::new(format!("{} songs \u{2022} {}", songs.len(), crate::downloads::folder().display()))
            .font(theme::regular(13.0))
            .color(theme::text_dim()),
    );
    // Runs side by side, as many to a line as fit at 280 wide or more.
    let mut cancel = None;
    let (gap, full) = (8.0, ui.available_width());
    let cols = (((full + gap) / (280.0 + gap)).floor() as usize).max(1);
    let w = (full - gap * (cols - 1) as f32) / cols as f32;
    for line in app.download_runs.chunks(cols) {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for r in line {
                if download_run(ui, r, run_thumbs(app, &r.key), w) {
                    cancel = Some(r.key.clone());
                }
            }
        });
    }
    if let Some(key) = cancel {
        app.actions.push(Action::CancelDownload(key));
    }
    ui.add_space(12.0);
    if songs.is_empty() {
        ui.label(RichText::new("No downloads yet").font(theme::semibold(16.0)).color(theme::text()));
        ui.add_space(4.0);
        ui.label(
            RichText::new("Right-click a song and choose Download, or a whole album or playlist from its page, to keep it on this computer, playable offline.")
                .font(theme::regular(13.0))
                .color(theme::text_dim()),
        );
        ui.add_space(10.0);
        if pill(ui, "Open folder", Some(icon::FOLDER_OPEN), false).clicked() {
            crate::downloads::open_folder();
        }
        return;
    }
    ui.horizontal(|ui| {
        if pill(ui, "Play", Some(icon::PLAY), true).clicked() {
            // All of them from the top, in the order they're sorted.
            let plays = match app.sorts.sorted("downloads", songs) {
                Some(o) => o.iter().map(|&i| songs[i].clone()).collect(),
                None => songs.clone(),
            };
            app.actions.push(Action::Play(plays, 0));
        }
        if pill(ui, "Shuffle", Some(icon::SHUFFLE), false).clicked() {
            let mut t = songs.clone();
            super::shuffle(&mut t);
            app.actions.push(Action::Play(t, 0));
        }
        if pill(ui, "Open folder", Some(icon::FOLDER_OPEN), false).clicked() {
            crate::downloads::open_folder();
        }
        sort_pill(ui, &mut app.sorts, &app.page, false);
    });
    ui.add_space(12.0);
    let rows = Rows::new(app, now);
    let order = app.sorts.order("downloads", songs);
    list(ui, &rows, &app.liked, songs, order.as_deref(), true, |_, _| None, ListCtx { sel: &mut app.selection, page: &app.page, own: false }, &mut app.row_drag, egui::Id::unique("downloads"), &mut app.actions);
}

// ------------------------------------------------------------------ settings

// ── Settings ─────────────────────────────────────────────────────────────

/// The categories Settings splits into: most used first, which is how they're listed.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum SettingsTab {
    #[default]
    Account,
    Discord,
    Playback,
    Sources,
    Lyrics,
    Appearance,
    General,
    Shortcuts,
}

impl SettingsTab {
    const ALL: [Self; 8] = [Self::Account, Self::Discord, Self::Playback, Self::Sources, Self::Lyrics, Self::Appearance, Self::General, Self::Shortcuts];

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Account => "Account",
            Self::Discord => "Discord",
            Self::Playback => "Playback",
            Self::Sources => "Music sources",
            Self::Lyrics => "Lyrics",
            Self::Appearance => "Appearance",
            Self::General => "General",
            Self::Shortcuts => "Keyboard shortcuts",
        }
    }
}

/// What Settings' search box is looking for, for the frame being drawn. Only Settings
/// sets it, so the rows and cards other pages borrow stay as they are.
#[derive(Default)]
struct SettingsFilter {
    words: Vec<String>,
    /// The section and category being drawn, which count as part of each row's text.
    section: String,
    tab: &'static str,
    /// What's shown so far, in this section and on the whole page.
    in_section: usize,
    total: usize,
}

thread_local! {
    static FILTER: std::cell::RefCell<SettingsFilter> = std::cell::RefCell::default();
}

/// Every word is in `text` or the name of its section. `words` are lowercase.
fn words_match(words: &[String], section: &str, text: &str) -> bool {
    let (section, text) = (section.to_lowercase(), text.to_lowercase());
    words.iter().all(|w| section.contains(w.as_str()) || text.contains(w.as_str()))
}

pub(super) fn filtering() -> bool {
    FILTER.with_borrow(|f| !f.words.is_empty())
}

/// Whether what's looked for is in `text`, and it isn't counted as shown.
fn filter_hit(text: &str) -> bool {
    FILTER.with_borrow(|f| f.words.is_empty() || words_match(&f.words, &f.section, text))
}

/// Whether something described by `text` is shown. In a search it's counted, and set off
/// from what's above it, as the rows that would have gone between aren't there.
pub(super) fn filter_show(ui: &mut Ui, text: &str) -> bool {
    if !filtering() {
        return true;
    }
    if !filter_hit(text) {
        return false;
    }
    let first = FILTER.with_borrow_mut(|f| {
        f.in_section += 1;
        f.total += 1;
        f.in_section == 1
    });
    if !first {
        divider_line(ui);
    }
    true
}

/// A wide window has room for the categories beside what's in them; this is the page's
/// width below which they go above it.
const SETTINGS_SIDE_BY_SIDE: f32 = 680.0;
const SETTINGS_RAIL: f32 = 200.0;
const SETTINGS_GAP: f32 = 32.0;
const SETTINGS_COLUMN: f32 = 800.0;
const SETTINGS_SEARCH_H: f32 = 38.0;

fn settings(app: &mut App, ui: &mut Ui) {
    let words: Vec<String> = app.settings_search.to_lowercase().split_whitespace().map(str::to_owned).collect();
    FILTER.set(SettingsFilter { words, ..Default::default() });
    let full = ui.available_rect_before_wrap();
    // Its rail and heading don't scroll: clear of the window buttons above and the
    // floating player below, which pages otherwise run under.
    let bar_top = super::titlebar::buttons_rect(ui.ctx()).map_or(0.0, |b| b.height());
    let full = full.with_min_y(full.top() + bar_top).with_max_y(full.bottom() - app.bar_height());
    let side_by_side = full.width() >= SETTINGS_SIDE_BY_SIDE;
    let used = SETTINGS_RAIL + SETTINGS_GAP + SETTINGS_COLUMN;
    let left = full.left() + ((full.width() - used) / 2.0).max(0.0);
    let top_down = egui::Layout::top_down(egui::Align::Min);
    let body = if side_by_side {
        let rail = Rect::from_min_max(pos2(left, full.top()), pos2(left + SETTINGS_RAIL, full.bottom()));
        settings_rail(app, &mut ui.new_child(egui::UiBuilder::new().max_rect(rail).layout(top_down)));
        Rect::from_min_max(pos2(rail.right() + SETTINGS_GAP, full.top()), full.max)
    } else {
        let mut head = ui.new_child(egui::UiBuilder::new().max_rect(full.shrink2(vec2(16.0, 0.0))).layout(top_down));
        settings_head(app, &mut head);
        head.horizontal_wrapped(|ui| {
            let mut pick = None;
            for t in SettingsTab::ALL {
                if widgets::chip(ui, t.label(), !filtering() && t == app.settings_tab).clicked() {
                    pick = Some(t);
                }
            }
            if let Some(t) = pick {
                settings_open(app, t);
            }
        });
        Rect::from_min_max(pos2(full.left() + 16.0, head.min_rect().bottom()), full.max)
    };
    let mut area = egui::ScrollArea::vertical().id_salt(("settings", if filtering() { None } else { Some(app.settings_tab.label()) })).auto_shrink(false);
    // Dev hook: YTM_SCROLL=<px> pins the page scroll, for screenshots of lower sections.
    if let Some(y) = super::dev_var("YTM_SCROLL").and_then(|v| v.parse::<f32>().ok()) {
        area = area.vertical_scroll_offset(y);
    }
    let mut column = ui.new_child(egui::UiBuilder::new().max_rect(body).layout(top_down));
    area.show(&mut column, |ui| {
        ui.set_max_width((ui.available_width() - 20.0).clamp(0.0, SETTINGS_COLUMN));
        if filtering() {
            // A search looks in every category, and what each one finds is drawn
            // without the rows that don't match.
            for tab in SettingsTab::ALL {
                FILTER.with_borrow_mut(|f| f.tab = tab.label());
                settings_tab(app, ui, tab);
            }
            if FILTER.with_borrow(|f| f.total == 0) {
                ui.add_space(40.0);
                ui.label(RichText::new(format!("No settings match \u{201c}{}\u{201d}.", app.settings_search.trim())).font(theme::regular(14.0)).color(theme::text_dim()));
            }
        } else {
            settings_tab(app, ui, app.settings_tab);
        }
        // Room to scroll the last rows clear of the player's frosted strip.
        ui.add_space(40.0);
    });
    ui.advance_cursor_after_rect(full);
    FILTER.take();
}

/// Picking a category ends a search: it's where you were looking to go.
fn settings_open(app: &mut App, tab: SettingsTab) {
    app.settings_tab = tab;
    app.settings_search.clear();
}

fn settings_tab(app: &mut App, ui: &mut Ui, tab: SettingsTab) {
    match tab {
        SettingsTab::Account => {
            settings_account(app, ui);
            settings_connections(app, ui);
            // Spotify, with the accounts it signs into: its transfer card and its
            // account row belong with the Google account above them.
            if filter_show(ui, "Import Spotify transfer Liked Songs playlists account") {
                super::transfer::settings_section(app, ui);
            }
        }
        SettingsTab::Discord => super::presence::settings(app, ui),
        SettingsTab::Playback => settings_playback(app, ui),
        SettingsTab::Sources => {
            settings_sources(app, ui);
        }
        SettingsTab::Lyrics => {
            settings_lyrics(app, ui);
        }
        SettingsTab::Appearance => {
            settings_appearance(app, ui);
            settings_artists(app, ui);
        }
        SettingsTab::General => {
            settings_community(ui);
            settings_window(app, ui);
            settings_downloads(ui);
            settings_updates(app, ui);
            settings_section(ui, "Network", None, None, |ui| network(app, ui));
            super::storage::settings_section(app, ui);
            // Last, as most apps keep it.
            settings_about(ui);
        }
        SettingsTab::Shortcuts => {
            if filter_show(ui, "Keyboard shortcuts keys hotkeys shortcuts combo") {
                super::keybinds::settings_section(app, ui);
            }
        }
    }
}

/// The page's title and its search box.
fn settings_head(app: &mut App, ui: &mut Ui) {
    page_title(ui, "Settings");
    ui.add_space(12.0);
    settings_search(app, ui);
    ui.add_space(14.0);
}

/// The title, the search box and the categories, on a wide window.
fn settings_rail(app: &mut App, ui: &mut Ui) {
    settings_head(app, ui);
    let mut pick = None;
    for t in SettingsTab::ALL {
        if settings_rail_row(ui, t.label(), !filtering() && t == app.settings_tab).clicked() {
            pick = Some(t);
        }
    }
    if let Some(t) = pick {
        settings_open(app, t);
    }
}

fn settings_rail_row(ui: &mut Ui, label: &str, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click());
    let resp = widgets::named(resp, label).on_hover_cursor(egui::CursorIcon::PointingHand);
    let h = theme::anim_bool(ui.ctx(), resp.id.with("h"), resp.hovered(), 0.15);
    let s = theme::anim_bool(ui.ctx(), resp.id.with("s"), selected, 0.15);
    if h + s > 0.0 {
        ui.painter().rect_filled(rect, 10.0, theme::glass(0.05 * h + 0.10 * s));
    }
    let color = theme::lerp_color(theme::text_dim(), theme::text(), h.max(s));
    ui.painter().text(pos2(rect.left() + 14.0, rect.center().y), Align2::LEFT_CENTER, label, theme::semibold(14.0), color);
    resp
}

/// The search box under the page title: while it has words, only the rows they match
/// stay up.
fn settings_search(app: &mut App, ui: &mut Ui) {
    let (rect, field) = ui.allocate_exact_size(vec2(ui.available_width(), SETTINGS_SEARCH_H), Sense::click());
    let field = field.on_hover_cursor(egui::CursorIcon::Text);
    ui.painter().rect_filled(rect, SETTINGS_SEARCH_H / 2.0, theme::glass(0.09));
    widgets::icon_at(ui, pos2(rect.left() + 18.0, rect.center().y), icon::SEARCH, 15.0, theme::text_dim());
    let clearable = !app.settings_search.is_empty();
    let end = rect.right() - if clearable { 34.0 } else { 12.0 };
    let inner = Rect::from_min_max(pos2(rect.left() + 34.0, rect.top() + 3.0), pos2(end, rect.bottom() - 3.0));
    let edit = ui.put(
        inner,
        egui::TextEdit::singleline(&mut app.settings_search)
            .hint_text(RichText::new("Search settings").color(theme::text_faint()))
            .font(theme::regular(14.0))
            .frame(egui::Frame::NONE)
            .align(Align2::LEFT_CENTER)
            .desired_width(inner.width()),
    );
    if clearable {
        let x = Rect::from_center_size(pos2(rect.right() - 19.0, rect.center().y), Vec2::splat(26.0));
        let resp = widgets::named(ui.interact(x, ui.scope_id().with("settings-clear"), Sense::click()), "Clear").on_hover_cursor(egui::CursorIcon::PointingHand);
        let h = theme::anim_bool(ui.ctx(), resp.id.with("h"), resp.hovered(), 0.15);
        if h > 0.0 {
            ui.painter().circle_filled(x.center(), 13.0, theme::glass(0.10 * h));
        }
        widgets::icon_at(ui, x.center(), icon::X, 14.0, theme::lerp_color(theme::text_dim(), theme::text(), h));
        if resp.clicked() {
            app.settings_search.clear();
            edit.request_focus();
        }
    }
    if field.clicked() {
        edit.request_focus();
    }
    // Esc takes the focus from a text box: it clears this one too.
    if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.settings_search.clear();
    }
}

/// Settings › Account: signing in and out, and which channel to use.
fn settings_account(app: &mut App, ui: &mut Ui) {
    let busy = app.signing_in.load(Ordering::Relaxed);
    let (mut sign_in, mut sign_out) = (false, false);
    // Asked again now and then while Settings shows: a failed ask, or a channel made since.
    let now = ui.input(|i| i.time);
    if app.logged_in && app.channels_asked.is_none_or(|at| now - at > 60.0) {
        app.channels_asked = Some(now);
        app.backend.request(crate::backend::Req::Channels);
    }
    settings_section(ui, "Account", None, None, |ui| {
        if app.logged_in {
            let name = app.account_name.clone().unwrap_or_else(|| "YouTube Music".to_owned());
            setting_row(ui, &name, Some(&format!("Signed in with Google. Your home feed, library and likes come from this account. Saved encrypted in {}.", super::secret_store())), |ui| {
                sign_out = pill(ui, "Sign out", Some(icon::LOG_OUT), false).clicked();
            });
            // Also with one, when the one in use isn't it (a brand channel since deleted).
            if app.channels.as_ref().is_some_and(|c| c.list.len() > 1 || !c.list.iter().any(|l| l.page_id == c.current)) {
                divider(ui);
                setting_row(ui, "Channel", Some("Your Google account and each YouTube channel it manages have their own library, likes, history and recommendations."), |ui| {
                    channel_picker(app, ui);
                });
            }
        } else {
            setting_row(ui, "Not signed in", Some("Sign in to get your home feed, library and likes."), |ui| {
                if busy {
                    ui.spinner();
                } else {
                    sign_in = pill(ui, "Sign in with Google", Some(icon::CIRCLE_USER), true).clicked();
                }
            });
        }
        divider(ui);
        // Listen together, under the account it borrows its name from: the page holds
        // both room kinds, the relay jam and the web app's Listen Together.
        if open_row(ui, "Listen together", &format!("Play the same music at the same moment, on up to {} devices.", crate::jam::DEFAULT_MEMBERS)) {
            app.go(Page::Jam);
        }
    });
    if busy && filter_show(ui, "finish signing in in the window that opened") {
        ui.add_space(6.0);
        ui.label(RichText::new("Finish signing in in the window that opened.").font(theme::regular(12.5)).color(theme::text_dim()));
    }
    if sign_in {
        app.start_sign_in(ui.ctx());
    }
    if sign_out {
        app.settle_now();
        app.backend.request(crate::backend::Req::Logout);
    }
}

fn channel_picker(app: &mut App, ui: &mut Ui) {
    let Some(crate::backend::Channels { list, current, epoch }) = &app.channels else { return };
    // A brand channel often has the account's name: the handle, or "No channel" (as
    // YouTube's switcher says), tells them apart.
    let label = |c: &crate::ytm::Channel| match (&c.handle, c.has_channel) {
        (Some(h), _) => format!("{} ({h})", c.name),
        (None, false) => format!("{} (No channel)", c.name),
        (None, true) => c.name.clone(),
    };
    // The button is narrower: a handle alone is enough there.
    let shown = match list.iter().find(|c| c.page_id == *current) {
        Some(c) => c.handle.clone().unwrap_or_else(|| label(c)),
        None => "Another channel".to_owned(),
    };
    // A transfer would go on as the new channel.
    if app.transfer.running() {
        ui.label(RichText::new("Not while a transfer runs").font(theme::regular(13.0)).color(theme::text_dim()));
        return;
    }
    let switching = app.switching_to.is_some();
    let btn = dropdown_button(ui, if switching { "Switching…" } else { &shown }, 220.0, "Channel");
    let mut pick = None;
    egui::Popup::menu(&btn).show(|ui| {
        ui.set_width(300.0);
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
            ui.add_space(4.0);
            for c in list {
                if option_row(ui, &label(c), c.page_id == *current).clicked() {
                    pick = Some(c.clone());
                    ui.close();
                }
            }
            ui.add_space(4.0);
        });
    });
    if let Some(c) = pick.filter(|c| c.page_id != *current && !switching) {
        app.switching_to = Some(label(&c));
        let epoch = *epoch;
        app.settle_now();
        app.backend.request(crate::backend::Req::SwitchChannel { page_id: c.page_id, epoch });
    }
}

/// Settings › Playback, and below it the equalizer in its own section.
fn settings_playback(app: &mut App, ui: &mut Ui) {
    settings_section(ui, "Playback", None, None, |ui| {
        setting_row(ui, "Autoplay", Some("Keep playing similar songs when the queue ends."), |ui| {
            toggle(ui, &mut app.autoplay, "Autoplay");
        });
        divider(ui);
        let desc = normalization_desc(app);
        setting_row(ui, "Volume normalization", Some(&desc), |ui| {
            use crate::audio::Normalize;
            let btn = dropdown_button(ui, app.normalize.label(), 120.0, "Volume normalization");
            egui::Popup::menu(&btn).show(|ui| {
                ui.set_width(120.0);
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                ui.add_space(4.0);
                for level in Normalize::ALL {
                    if option_row(ui, level.label(), level == app.normalize).clicked() {
                        app.normalize = level;
                        app.audio.set_normalize(level);
                        ui.close();
                    }
                }
                ui.add_space(4.0);
            });
        });
        divider(ui);
        setting_row(ui, "Logarithmic volume", Some("Space the volume slider out the way ears hear loudness, so quiet levels aren't squeezed into its first few pixels."), |ui| {
            if toggle(ui, &mut app.log_volume, "Logarithmic volume").changed() {
                app.audio.set_log_volume(app.log_volume);
            }
        });
        divider(ui);
        setting_row(ui, "Fade when pausing", Some("Turn the music down when you pause, and back up when you play again, over a fifth of a second. Off, it stops and starts right away."), |ui| {
            if toggle(ui, &mut app.pause_fade, "Fade when pausing").changed() {
                app.audio.set_pause_fade(app.pause_fade);
            }
        });
        divider(ui);
        setting_row(ui, "Play songs, not music videos", Some("An official music video plays as its song, the album recording, when YouTube Music has one: no intros, skits or video-only edits. Lists still show what they hold."), |ui| {
            toggle(ui, &mut app.play_songs, "Play songs, not music videos");
        });
        divider(ui);
        setting_row(ui, "Output device", Some("Where the music plays. A picked device that isn't connected leaves it to the system's default until it's back."), |ui| {
            output_device_picker(app, ui);
        });
        divider(ui);
        setting_row(ui, "Show music videos", Some("Music videos play as moving video, with their own sound and no lyrics. Turned off while songs play in their place, songs have no Song and Video switch in Now playing; music videos keep theirs."), |ui| {
            let mut on = app.show_video;
            if toggle(ui, &mut on, "Show music videos").changed() {
                app.actions.push(Action::ToggleVideo);
            }
        });
        divider(ui);
        setting_row(ui, "Video quality", Some("Music videos play this sharp, or as close below it as the video comes. 1080p takes about three times the work of 720p, which a slower computer may not keep up with."), |ui| {
            let label = |h: u32| format!("{h}p");
            let btn = dropdown_button(ui, &label(app.video_quality), 120.0, "Video quality");
            egui::Popup::menu(&btn).show(|ui| {
                ui.set_width(120.0);
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                ui.add_space(4.0);
                for h in super::VIDEO_QUALITIES {
                    if option_row(ui, &label(h), h == app.video_quality).clicked() {
                        super::nowplaying::set_video_quality(app, h);
                        ui.close();
                    }
                }
            });
        });
        divider(ui);
        setting_row(ui, "Hide videos in search", Some("Only songs, albums, artists and playlists in search results."), |ui| {
            toggle(ui, &mut app.hide_search_videos, "Hide videos in search");
        });
        divider(ui);
        setting_row(ui, "Show comments", Some("A Comments tab in now playing, beside Up next and Lyrics: what people say about the song, Top or Newest, with replies. Signed in, you can comment, reply and like too."), |ui| {
            if toggle(ui, &mut app.show_comments, "Show comments").changed() && !app.show_comments && app.np_tab == super::NpTab::Comments {
                app.np_tab = super::NpTab::Art;
            }
        });
        divider(ui);
        setting_row(ui, "Automix", Some("Slide from one song into the next the way a DJ would: both hold their level for a moment, then swap in one smooth move, instead of a straight fade. The mix picks its own length, in phrases of 8 or 16 bars (16 on long songs), following the beat it hears."), |ui| {
            if toggle(ui, &mut app.automix, "Automix").changed() {
                app.set_crossfade();
            }
        });
        if !app.automix {
            divider(ui);
            setting_row(ui, "Crossfade songs", Some("Blend the end of each song into the start of the next."), |ui| {
                if toggle(ui, &mut app.crossfade, "Crossfade songs").changed() {
                    app.set_crossfade();
                }
            });
            if app.crossfade {
                divider(ui);
                setting_row(ui, "Crossfade length", None, |ui| {
                    if seconds_slider(ui, &mut app.crossfade_secs, 1..=super::CROSSFADE_MAX, "Crossfade length").changed() {
                        app.set_crossfade();
                    }
                });
            }
        }
        divider(ui);
        setting_row(ui, "Skip non-music parts", Some("Jump over talking, skits and sponsor breaks in music videos, as marked by the SponsorBlock community. Album songs don't have any."), |ui| {
            if toggle(ui, &mut app.sponsorblock, "Skip non-music parts").changed() {
                app.request_segments();
            }
        });
        divider(ui);
        setting_row(ui, "Hide explicit songs", Some("Leave songs marked E out of lists, search, the queue and autoplay. Ones already playing finish."), |ui| {
            if toggle(ui, &mut app.hide_explicit, "Hide explicit songs").changed() {
                app.set_hide_explicit();
            }
        });
        divider(ui);
        setting_row(ui, "Show podcasts", Some("Podcast shows and episodes: a Podcasts filter on Home, in search results, and the ones you've saved in your library."), |ui| {
            if toggle(ui, &mut app.show_podcasts, "Show podcasts").changed() {
                app.set_show_podcasts();
            }
        });
        divider(ui);
        // Only with hiding on: it's off (and shown off) until then.
        ui.add_enabled_ui(app.hide_explicit, |ui| {
            setting_row(ui, "Use YouTube's Restricted Mode", Some("Stricter: YouTube also leaves out whatever else it finds mature, not just explicit songs. Applies to pages loaded from now on."), |ui| {
                let mut on = app.hide_explicit && app.restricted_mode;
                if toggle(ui, &mut on, "Use YouTube's Restricted Mode").changed() {
                    app.restricted_mode = on;
                    app.set_hide_explicit();
                }
            });
        });
    });
    settings_equalizer(app, ui);
    settings_tempo(app, ui);
}

fn settings_tempo(app: &mut App, ui: &mut Ui) {
    let jam = app.jam.active();
    settings_section(ui, "Playback speed", None, jam.then_some(TEMPO_IN_JAM), |ui| {
        setting_row(ui, "Playback speed", Some("Play songs faster or slower, also from a button in the player bar. Faster sounds higher and slower lower, like a record. Lyrics stay in time. Back to 1× each time BitChord starts."), |ui| {
            toggle(ui, &mut app.tempo_on, "Playback speed");
        });
        ui.add_enabled_ui(app.tempo_on && !jam, |ui| {
            use crate::audio::tempo::{MAX_SPEED, MIN_SPEED};
            divider(ui);
            setting_row(ui, "Speed", None, |ui| {
                step_slider(ui, &mut app.speed, (MIN_SPEED, MAX_SPEED, 0.05), Some(1.0), times, "Speed");
            });
        });
    });
}

pub(super) const TEMPO_IN_JAM: &str = "Back to normal while you're in a jam, so everyone hears the same song.";

/// A speed as it's shown: 1×, 1.5×, 1.25×.
pub(super) fn times(v: f32) -> String {
    format!("{}×", (v * 100.0).round() / 100.0)
}

/// Settings › Playback › Equalizer: ten bands, presets, and the curve they make drawn
/// behind the sliders. Shapes only what you hear: not downloads, or what others in a jam
/// hear.
fn settings_equalizer(app: &mut App, ui: &mut Ui) {
    use crate::audio::eq;
    settings_section(ui, "Equalizer", None, None, |ui| {
        setting_row(ui, "Equalizer", Some("Shape the sound from deep bass to treble, on everything BitChord plays. Only what you hear changes: not downloads, or what others in a jam hear."), |ui| {
            if toggle(ui, &mut app.eq_on, "Equalizer").changed() {
                app.set_eq();
            }
        });
        ui.add_enabled_ui(app.eq_on, |ui| {
            divider(ui);
            setting_row(ui, "Preset", None, |ui| {
                let btn = dropdown_button(ui, eq::preset_name(&app.eq_gains).unwrap_or("Custom"), 180.0, "Equalizer preset");
                let mut pick = None;
                egui::Popup::menu(&btn).show(|ui| {
                    ui.set_width(200.0);
                    ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                    ui.add_space(4.0);
                    for (name, gains) in &eq::PRESETS {
                        if option_row(ui, name, app.eq_gains == *gains).clicked() {
                            pick = Some(*gains);
                            ui.close();
                        }
                    }
                });
                if let Some(g) = pick {
                    app.eq_gains = g;
                    app.set_eq();
                }
            });
            if filter_show(ui, "Equalizer bands bass mids treble frequency slider") {
                divider(ui);
                if eq_bands(ui, &mut app.eq_gains) {
                    app.set_eq();
                }
            }
            divider(ui);
            setting_row(ui, "Prevent clipping", Some("Turn everything down by as much as the bands boost, so loud songs are less likely to crackle."), |ui| {
                if toggle(ui, &mut app.eq_no_clip, "Prevent clipping").changed() {
                    app.set_eq();
                }
            });
        });
    });
}

/// The equalizer's bands as vertical sliders, over the curve they make together.
/// Drag or click to set one, or double-click for 0 dB. True if one changed.
fn eq_bands(ui: &mut Ui, gains: &mut crate::audio::eq::Gains) -> bool {
    use crate::audio::eq::{BANDS, MAX_DB, response};
    const LABELS: [&str; 10] = ["31", "62", "125", "250", "500", "1k", "2k", "4k", "8k", "16k"];
    const NAMES: [&str; 10] = ["31 Hz", "62 Hz", "125 Hz", "250 Hz", "500 Hz", "1 kHz", "2 kHz", "4 kHz", "8 kHz", "16 kHz"];
    const GUTTER: f32 = 34.0;
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 210.0), Sense::hover());
    let fade = if ui.is_enabled() { 1.0 } else { 0.45 };
    let accent = ui.ctx().data(|d| d.get_temp::<Color32>(egui::Id::unique("accent"))).unwrap_or(theme::RED);
    let p = ui.painter();
    let top = rect.top() + 26.0;
    let bottom = rect.bottom() - 30.0;
    let mid = (top + bottom) / 2.0;
    let y_of = |db: f32| mid - db / MAX_DB * (bottom - top) / 2.0;
    let col_w = (rect.width() - GUTTER) / BANDS.len() as f32;
    let x_of = |i: f32| rect.left() + GUTTER + col_w * (i + 0.5);
    for (db, text) in [(MAX_DB, "+12"), (0.0, "0"), (-MAX_DB, "\u{2212}12")] {
        p.text(pos2(rect.left(), y_of(db)), Align2::LEFT_CENTER, text, theme::regular(11.5), theme::with_alpha(theme::text_dim(), fade));
    }
    p.line_segment([pos2(x_of(-0.5), mid), pos2(x_of(9.5), mid)], egui::Stroke::new(1.0, theme::glass(0.10 * fade)));
    // Bands are an octave apart, so evenly spaced columns sit on a log frequency axis.
    // The curve can go a little past ±12 between bands; it runs off the top rather than
    // flattening there and hiding it. It's drawn from the first band to the last: past
    // them it only shows the filters' tails, which the sliders don't set.
    let db_at = response(gains);
    let curve: Vec<egui::Pos2> = (0..=180)
        .map(|k| {
            let i = 9.0 * k as f32 / 180.0;
            pos2(x_of(i), y_of(db_at(BANDS[0] * 2f32.powf(i))))
        })
        .collect();
    ui.painter_at(rect).add(egui::Shape::line(curve, egui::Stroke::new(2.0, theme::with_alpha(accent, 0.55 * fade))));
    let mut changed = false;
    for (i, g) in gains.iter_mut().enumerate() {
        let x = x_of(i as f32);
        // Just the track and a knob's reach past its ends, so clicking a label doesn't
        // send the band to the bottom.
        let col = Rect::from_min_max(pos2(x - col_w / 2.0, top - 8.0), pos2(x + col_w / 2.0, bottom + 8.0));
        let mut resp = ui.interact(col, ui.scope_id().with(("eq-band", i)), Sense::click_and_drag());
        let name = NAMES[i];
        let old = *g;
        if resp.double_clicked() {
            *g = 0.0;
        } else if let Some(pos) = resp.interact_pointer_pos()
            && (resp.dragged() || resp.clicked())
        {
            *g = (((mid - pos.y) / ((bottom - top) / 2.0) * MAX_DB * 2.0).round() / 2.0).clamp(-MAX_DB, MAX_DB);
        }
        if *g != old {
            changed = true;
            resp.mark_changed();
        }
        let value = *g;
        resp.widget_info(|| egui::WidgetInfo::slider(ui.is_enabled(), value as f64, name));
        let resp = resp.on_hover_cursor(egui::CursorIcon::ResizeVertical);
        let h = theme::anim_bool(ui.ctx(), resp.id.with("h"), resp.hovered() || resp.dragged() || resp.has_focus(), 0.12);
        let track = Rect::from_center_size(pos2(x, mid), vec2(4.0, bottom - top));
        p.rect_filled(track, 2.0, theme::glass(0.15 * fade));
        let knob = pos2(x, y_of(value));
        p.rect_filled(Rect::from_two_pos(pos2(x - 2.0, mid), pos2(x + 2.0, knob.y)), 2.0, theme::with_alpha(accent, fade));
        p.circle_filled(knob + vec2(0.0, 1.0), 7.0 + h, theme::shade(0.25 * fade));
        p.circle_filled(knob, 7.0 + h, theme::with_alpha(Color32::WHITE, fade));
        if h > 0.0 {
            let text = match value {
                0.0 => "0 dB".to_owned(),
                v if v > 0.0 => format!("+{v} dB"),
                v => format!("\u{2212}{} dB", -v),
            };
            p.text(pos2(x, rect.top() + 9.0), Align2::CENTER_CENTER, text, theme::semibold(12.0), theme::with_alpha(theme::text(), h * fade));
        }
        p.text(pos2(x, rect.bottom() - 12.0), Align2::CENTER_CENTER, LABELS[i], theme::regular(12.0), theme::with_alpha(theme::text_dim(), fade));
    }
    changed
}

/// The addon list: one line per addon with its on/off switch and its Test, and
/// "+ Add a source" below it that opens a fresh line for the next one. Test
/// health-checks the link (BitChord's source editor's Test, [`crate::hires::Client::probe`]),
/// whether or not it's saved. A switched-off addon stays in the list, but no song
/// is looked up on it.
fn bitchord_addons_editor(app: &mut App, ui: &mut Ui) {
    let mut edited = false;
    let mut remove = None;
    // ↑/↓ through the list: the song asks the addons in this order, so an addon moved
    // up is asked first (the same priority the sources list above orders by).
    let mut swap = None;
    for i in 0..app.hires_sources.len() {
        let link = app.hires_sources[i].trim().to_owned();
        // One addon: switch, field, Test and × are one unit in a search.
        if !filter_show(ui, &format!("Bitchord addon {link} test")) {
            continue;
        }
        ui.horizontal(|ui| {
            let mut on = app.hires_enabled.get(i).copied().unwrap_or(true);
            if toggle(ui, &mut on, "Addon on").changed() {
                if let Some(slot) = app.hires_enabled.get_mut(i) {
                    *slot = on;
                }
                app.backend.set_hires_source(&live_sources(app));
            }
            ui.add_space(6.0);
            let field_w = (ui.available_width() - 150.0).max(120.0);
            ui.allocate_ui_with_layout(vec2(field_w, 42.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.set_width(field_w);
                if super::text_field(ui, "", &mut app.hires_sources[i], false, "https://your-addon.example").changed() {
                    edited = true;
                }
            });
            hires_test_button(app, ui, &link);
            // Priority: up and down through the addon list.
            if widgets::icon_button(ui, icon::CHEVRON_UP, 11.0, "Move this addon up", false).clicked()
                && i > 0
            {
                swap = Some((i - 1, i));
            }
            if widgets::icon_button(ui, icon::CHEVRON_DOWN, 11.0, "Move this addon down", false).clicked()
                && i + 1 < app.hires_sources.len()
            {
                swap = Some((i, i + 1));
            }
            if widgets::icon_button(ui, icon::X, 14.0, "Remove source", false).clicked() {
                remove = Some(i);
            }
        });
        hires_test_line(app, ui, &link);
        ui.add_space(6.0);
    }
    if let Some((a, b)) = swap {
        // The tests' verdicts are keyed by the links themselves, so they follow their
        // link to its new place; only the list order and the switches move.
        app.hires_sources.swap(a, b);
        if a < app.hires_enabled.len() && b < app.hires_enabled.len() {
            app.hires_enabled.swap(a, b);
        }
        edited = true;
    }
    if let Some(i) = remove {
        let gone = app.hires_sources.remove(i);
        if i < app.hires_enabled.len() {
            app.hires_enabled.remove(i);
        }
        app.hires_tests.remove(gone.trim());
        edited = true;
    }
    if edited {
        // One row per distinct link: an exact repeat of one already listed is dropped
        // (it would fire duplicate searches and streams at one server, which is how a
        // working source starts answering 502). A different link on the same host is a
        // separate source the user asked for and stays.
        let mut seen = std::collections::HashSet::new();
        let mut i = 0;
        while i < app.hires_sources.len() {
            let key = app.hires_sources[i].trim().to_owned();
            if !key.is_empty() && !seen.insert(key) {
                app.hires_sources.remove(i);
                if i < app.hires_enabled.len() {
                    app.hires_enabled.remove(i);
                }
            } else {
                i += 1;
            }
        }
        app.backend.set_hires_source(&live_sources(app));
    }
    if app.hires_adding {
        if filter_show(ui, "Bitchord addon add a source link test") {
            let resp = super::text_field(ui, "", &mut app.hires_draft, false, "https://your-addon.example");
            if app.hires_focus {
                resp.request_focus();
                app.hires_focus = false;
            }
            // Test before the link is even saved: finding out here beats finding out the
            // first time a song would have used it (the editor's Test beside Save).
            let typed = app.hires_draft.trim().to_owned();
            ui.horizontal(|ui| {
                hires_test_button(app, ui, &typed);
                if typed.is_empty() {
                    ui.label(RichText::new("Enter a link, then Test it or press Enter to add it.").font(theme::regular(12.5)).color(theme::text_faint()));
                }
            });
            hires_test_line(app, ui, &typed);
            // Enter commits without needing to click away; clicking elsewhere does too.
            let enter = resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if resp.lost_focus() || enter {
                let link = app.hires_draft.trim().to_owned();
                if !link.is_empty() && !app.hires_sources.iter().any(|s| s.trim() == link) {
                    app.hires_sources.push(link);
                    app.hires_enabled.push(true);
                    app.backend.set_hires_source(&live_sources(app));
                }
                app.hires_draft.clear();
                app.hires_adding = false;
            }
            ui.add_space(6.0);
        }
    } else if filter_show(ui, "Bitchord addon add a source") && pill(ui, "+  Add a source", None, false).clicked() {
        app.hires_adding = true;
        app.hires_focus = true;
        app.hires_draft.clear();
    }
}

/// The Test pill for one source link (saved or still being typed): empty links can't be
/// probed, a running probe spins, and a re-Test asks again.
fn hires_test_button(app: &mut App, ui: &mut Ui, link: &str) {
    let empty = link.is_empty();
    if empty {
        ui.add_space(ui.available_width().min(76.0));
        return;
    }
    let slot = app.hires_tests.entry(link.to_owned()).or_default().clone();
    let state = slot.lock().unwrap().clone();
    if matches!(state, Some(None)) {
        ui.add_space(ui.available_width().min(76.0));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spinner();
        });
        return;
    }
    if pill(ui, "Test", None, false).clicked() {
        *slot.lock().unwrap() = Some(None);
        let (slot, http, link) = (slot, app.backend.http(), link.to_owned());
        app.backend.spawn(async move {
            let health = crate::hires::probe_link(&link, &http).await;
            *slot.lock().unwrap() = Some(Some((health.good(), health.message().to_owned())));
        });
    }
}

/// The verdict line under one source's row, colored like the sources list's status line:
/// green when the probe reached a working addon, red when the addon (or the link) is
/// wrong, dimmed gray while it asks.
fn hires_test_line(app: &App, ui: &mut Ui, link: &str) {
    let Some(state) = app.hires_tests.get(link).and_then(|s| s.lock().unwrap().clone()) else { return };
    let Some((good, msg)) = state else { return };
    let color = if good { theme::GREEN } else { theme::RED };
    let label = if good { format!("{msg}") } else { format!("Didn't work: {msg}") };
    ui.label(RichText::new(label).font(theme::regular(12.5)).color(color));
}

/// The non-empty, switched-on links as one newline-separated list, the way the backend
/// reads them. A row's link edits are saved whether its switch is on or not; only the
/// on ones reach the backend.
fn live_sources(app: &App) -> String {
    app.hires_sources.iter().enumerate().filter(|(i, s)| {
        let on = app.hires_enabled.get(*i).copied().unwrap_or(true);
        on && !s.trim().is_empty()
    }).map(|(_, s)| s.trim()).collect::<Vec<_>>().join("\n")
}

/// Settings › Music sources: the providers ported from Mochy-Music. When one has the
/// song, its copy plays instead of YouTube's, and its quality shows in the player.
/// The page reads one way: how a song plays, then the sources in the order they're
/// asked — each with its own settings under its row, like every other — then the
/// servers themselves.
fn settings_sources(app: &mut App, ui: &mut Ui) {
    settings_section(ui, "How songs play", None, Some("Every source on this page plays by these, whichever one a song comes from."), |ui| {
        setting_row(ui, "Prefer Dolby Atmos", Some("Ask the addons for a Dolby Atmos copy when they offer one. Falls back to the best stereo copy when the stream's codec can't be decoded."), |ui| {
            if toggle(ui, &mut app.prefer_atmos, "Prefer Dolby Atmos").changed() {
                app.backend.set_prefer_atmos(app.prefer_atmos);
            }
        });
        divider(ui);
        setting_row(ui, "Play YouTube first, upgrade after", Some("Start every song from YouTube at once and swap in the hi-res copy in place when an addon answers, instead of waiting for the addons before the song starts."), |ui| {
            if toggle(ui, &mut app.disable_direct_stream, "Play YouTube first, upgrade after").changed() {
                app.backend.set_disable_direct_stream(app.disable_direct_stream);
            }
        });
        divider(ui);
        setting_row(ui, "Stats for nerds", Some("Show the codec, sample rate and bitrate of what's playing at the foot of the cover in Now playing, whatever the source."), |ui| {
            toggle(ui, &mut app.show_nerd_stats, "Stats for nerds");
        });
    });
    settings_section(ui, "Source priority", None, Some("The first source in this list that has the song plays its own copy; when none does, YouTube Music plays as usual. The arrows beside a source's switch move it up and down."), |ui| {
        for i in 0..app.source_order.len() {
            let kind = app.source_order[i];
            if i > 0 {
                divider(ui);
            }
            ui.horizontal(|ui| {
                setting_row(ui, kind.label(), Some(source_blurb(kind)), |ui| {
                    let mut on = match kind {
                        crate::backend::MusicSource::Addons => app.addons_on,
                        crate::backend::MusicSource::MonoServers => app.mono_servers_on,
                        crate::backend::MusicSource::Monochrome => app.monochrome,
                        crate::backend::MusicSource::JioSaavn => app.jiosaavn,
                        crate::backend::MusicSource::Youtube => app.youtube_on,
                    };
                    if toggle(ui, &mut on, kind.label()).changed() {
                        match kind {
                            crate::backend::MusicSource::Addons => app.addons_on = on,
                            crate::backend::MusicSource::MonoServers => app.mono_servers_on = on,
                            crate::backend::MusicSource::Monochrome => app.monochrome = on,
                            crate::backend::MusicSource::JioSaavn => app.jiosaavn = on,
                            crate::backend::MusicSource::Youtube => app.youtube_on = on,
                        }
                        app.apply_sources();
                    }
                    ui.add_space(8.0);
                    // The arrows live beside the switch: up and down through the order.
                    if widgets::icon_button(ui, icon::CHEVRON_UP, 11.0, "Move this source up", false).clicked()
                        && i > 0
                    {
                        app.source_order.swap(i - 1, i);
                        app.apply_sources();
                    }
                    if widgets::icon_button(ui, icon::CHEVRON_DOWN, 11.0, "Move this source down", false).clicked()
                        && i + 1 < app.source_order.len()
                    {
                        app.source_order.swap(i, i + 1);
                        app.apply_sources();
                    }
                });
            });
            // The built-in streamer's check, under its own row: its own unit in a search.
            if kind == crate::backend::MusicSource::Monochrome && filter_show(ui, "Monochrome test connection") {
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    let slot = app.monochrome_test.clone();
                    let state = slot.lock().unwrap().clone();
                    if matches!(state, Some(None)) {
                        ui.add_space(ui.available_width().min(76.0));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.spinner();
                        });
                    } else if pill(ui, "Test", None, false).clicked() {
                        *slot.lock().unwrap() = Some(None);
                        let (slot, http) = (slot, app.backend.http());
                        app.backend.spawn(async move {
                            let (good, msg) = crate::providers::probe_monochrome(&http, crate::providers::MONO_STREAMER).await;
                            *slot.lock().unwrap() = Some(Some((good, msg)));
                        });
                    }
                });
                test_line(ui, &app.monochrome_test.lock().unwrap().clone());
                ui.add_space(10.0);
            }
            // JioSaavn's own settings, under its row the same way: its stream quality and
            // its check, one unit in a search. Which quality plays is the row's say above.
            if kind == crate::backend::MusicSource::JioSaavn && filter_show(ui, "JioSaavn stream quality test connection") {
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    let btn = dropdown_button(ui, app.jiosaavn_quality.label(), 200.0, "JioSaavn stream");
                    egui::Popup::menu(&btn).show(|ui| {
                        ui.set_width(200.0);
                        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                        ui.add_space(4.0);
                        for quality in crate::providers::SaavnQuality::ALL {
                            if option_row(ui, quality.label(), app.jiosaavn_quality == quality).clicked() {
                                app.jiosaavn_quality = quality;
                                app.backend.set_saavn_quality(quality);
                                ui.close();
                            }
                        }
                        ui.add_space(4.0);
                    });
                    let slot = app.saavn_test.clone();
                    let state = slot.lock().unwrap().clone();
                    if matches!(state, Some(None)) {
                        ui.spinner();
                    } else if pill(ui, "Test", None, false).clicked() {
                        *slot.lock().unwrap() = Some(None);
                        let (slot, http) = (slot, app.backend.http());
                        app.backend.spawn(async move {
                            let (good, msg) = crate::providers::probe_saavn(&http).await;
                            *slot.lock().unwrap() = Some(Some((good, msg)));
                        });
                    }
                });
                test_line(ui, &app.saavn_test.lock().unwrap().clone());
                ui.add_space(10.0);
            }
        }
    });
    settings_provider_add(app, ui);
    settings_sources_lists(app, ui);
}

/// One source's row: what it is and what turning it off means.
fn source_blurb(kind: crate::backend::MusicSource) -> &'static str {
    match kind {
        crate::backend::MusicSource::Addons => "The Bitchord addon servers below: links that serve hi-res copies of songs, the same ones the BitChord Android app takes.",
        crate::backend::MusicSource::MonoServers => "Monochrome-format servers you've added below. A link's kind is found when its first search runs.",
        crate::backend::MusicSource::Monochrome => "Monochrome's own track streamer: lossless FLAC, no account needed, no setup.",
        crate::backend::MusicSource::JioSaavn => "JioSaavn's catalog in AAC up to 320 kbps, no account needed. Strongest on Indian music; it misses many big international labels.",
        crate::backend::MusicSource::Youtube => "YouTube Music itself, where every song ends up when the sources above it have no copy. Switching it off leaves only the sources above.",
    }
}

/// Adding a custom provider, one button for both formats: whatever is pasted is asked in
/// the Bitchord addon protocol and in the Monochrome protocol at the same time, and the
/// link is added wherever it answered — both, if both speak it — with each format's
/// verdict (its Test, which asks for real search results) on its own line. Which of them
/// wins for a song is the priority list's say.
fn settings_provider_add(app: &mut App, ui: &mut Ui) {
    settings_section(ui, "Add a custom provider", None, None, |ui| {
        let desc = "Paste an addon server's link and BitChord tries it as a Bitchord addon and as a Monochrome server at the same time, adding it to the formats that answer. Each verdict tells you whether search really works there.";
        let adding_text = format!("One link, both formats {desc}");
        setting_row(ui, "One link, both formats", Some(desc), |ui| {
            if !app.provider_adding && pill(ui, "+  Add a custom provider", None, false).clicked() {
                app.provider_adding = true;
                app.provider_focus = true;
                app.provider_draft.clear();
            }
        });
        // The field and its Add are that row's unit: they show exactly when it does.
        if app.provider_adding && filter_hit(&adding_text) {
            ui.add_space(6.0);
            let resp = super::text_field(ui, "", &mut app.provider_draft, false, "https://your-provider.example");
            if app.provider_focus {
                resp.request_focus();
                app.provider_focus = false;
            }
            let typed = app.provider_draft.trim().to_owned();
            let slot = app.provider_tests.entry(typed.clone()).or_default().clone();
            let state = slot.lock().unwrap().clone();
            ui.horizontal(|ui| {
                let running = matches!(state, Some(None));
                if !typed.is_empty() && !running && pill(ui, "Add", None, true).clicked() {
                    *slot.lock().unwrap() = Some(None);
                    let (slot, http, link) = (slot.clone(), app.backend.http(), typed.clone());
                    app.backend.spawn(async move {
                        let (bitchord, mono) = tokio::join!(crate::hires::probe_link(&link, &http), crate::providers::probe_monochrome(&http, &link));
                        *slot.lock().unwrap() = Some(Some(ProviderVerdicts { bitchord: Some((bitchord.good(), bitchord.message().to_owned())), mono: Some(mono) }));
                    });
                }
                if typed.is_empty() {
                    ui.label(RichText::new("Paste a link, then Add. Enter works too.").font(theme::regular(12.5)).color(theme::text_faint()));
                } else if running {
                    ui.label(RichText::new("Asking both formats…").font(theme::regular(12.5)).color(theme::text_faint()));
                }
            });
            // Enter in the field adds, the same as the button.
            if resp.lost_focus() {
                let link = app.provider_draft.trim().to_owned();
                if !link.is_empty() && !matches!(slot.lock().unwrap().as_ref(), Some(None)) {
                    let running = matches!(slot.lock().unwrap().as_ref(), Some(None));
                    if !running {
                        *slot.lock().unwrap() = Some(None);
                        let (slot, http, link) = (slot, app.backend.http(), link);
                        app.backend.spawn(async move {
                            let (bitchord, mono) = tokio::join!(crate::hires::probe_link(&link, &http), crate::providers::probe_monochrome(&http, &link));
                            *slot.lock().unwrap() = Some(Some(ProviderVerdicts { bitchord: Some((bitchord.good(), bitchord.message().to_owned())), mono: Some(mono) }));
                        });
                    }
                }
            }
            provider_verdicts(app, ui, &typed, &state);
            ui.add_space(6.0);
        }
        if !app.provider_adding && let Some(v) = &app.provider_last
            && filter_hit("Add a custom provider verdict")
            && FILTER.with_borrow(|f| f.in_section > 0)
        {
            ui.add_space(6.0);
            if let Some((good, msg)) = &v.bitchord {
                ui.label(RichText::new(verdict_line("Bitchord addon", *good, msg)).font(theme::regular(12.5)).color(if *good { theme::GREEN } else { theme::RED }));
            }
            if let Some((good, msg)) = &v.mono {
                ui.label(RichText::new(verdict_line("Monochrome", *good, msg)).font(theme::regular(12.5)).color(if *good { theme::GREEN } else { theme::RED }));
            }
        }
    });
}

/// A format's verdict, or how the ask went.
fn verdict_line(format: &str, good: bool, msg: &str) -> String {
    if good {
        format!("{format}: {msg}")
    } else {
        format!("{format}: didn't work ({msg})")
    }
}

/// The verdicts of the add-probe while the row is open — and the moment they're in, the
/// link joins every format that answered, the row closes and the verdicts stay below.
fn provider_verdicts(app: &mut App, ui: &mut Ui, typed: &str, state: &Option<Option<ProviderVerdicts>>) {
    let Some(Some(v)) = state else { return };
    let link = typed.to_owned();
    let mut added = false;
    if let Some((good, _)) = &v.bitchord
        && *good
        && !app.hires_sources.iter().any(|s| s.trim() == link)
    {
        app.hires_sources.push(link.clone());
        app.hires_enabled.push(true);
        added = true;
    }
    if let Some((good, _)) = &v.mono
        && *good
        && !app.mono_servers.iter().any(|s| s.trim() == link)
    {
        app.mono_servers.push(link.clone());
        added = true;
    }
    if added {
        app.backend.set_hires_source(&live_sources(app));
        app.backend.set_monochrome_servers(&live_mono_servers(app));
        app.apply_sources();
    }
    app.provider_last = Some(v.clone());
    app.provider_adding = false;
    app.provider_draft.clear();
    if let Some((good, msg)) = &v.bitchord {
        ui.label(RichText::new(verdict_line("Bitchord addon", *good, msg)).font(theme::regular(12.5)).color(if *good { theme::GREEN } else { theme::RED }));
    }
    if let Some((good, msg)) = &v.mono {
        ui.label(RichText::new(verdict_line("Monochrome", *good, msg)).font(theme::regular(12.5)).color(if *good { theme::GREEN } else { theme::RED }));
    }
}

/// The providers already added: the Bitchord addons, then the Monochrome servers, each
/// row with its Test and its switch; removing is the ×. New ones come from the one
/// button above, which asks both formats at once.
fn settings_sources_lists(app: &mut App, ui: &mut Ui) {
    settings_section(ui, "Bitchord addons", None, Some("Addon servers that serve hi-res copies of songs. A switched-off addon keeps its place in the list, but no song is looked up on it."), |ui| {
        bitchord_addons_editor(app, ui);
    });
    settings_section(ui, "Monochrome servers", None, Some("Monochrome-format servers you've added, tried before Monochrome's own streamer — they were added for a reason. A site's link is read for the server it plays from. New servers come from \"Add a custom provider\" above, which asks both formats at once."), |ui| {
        mono_servers_editor(app, ui);
    });
}

/// The Monochrome server list: one line per server with its Test. The built-in track
/// streamer has its own row in the priority list and isn't listed here. Test asks the
/// link whether a server answers there — or, for a Monochrome site's link, reads the
/// site for the server it plays from ([`crate::providers::probe_monochrome`]). New
/// servers come from "Add a custom provider" above, which asks both formats at once.
fn mono_servers_editor(app: &mut App, ui: &mut Ui) {
    let mut edited = false;
    let mut remove = None;
    for i in 0..app.mono_servers.len() {
        let link = app.mono_servers[i].trim().to_owned();
        // One server: field, Test and × are one unit in a search.
        if !filter_show(ui, &format!("Monochrome server {link} test")) {
            continue;
        }
        ui.horizontal(|ui| {
            let field_w = (ui.available_width() - 110.0).max(120.0);
            ui.allocate_ui_with_layout(vec2(field_w, 42.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.set_width(field_w);
                if super::text_field(ui, "", &mut app.mono_servers[i], false, "https://monochrome.example").changed() {
                    edited = true;
                }
            });
            mono_test_button(app, ui, &link);
            if widgets::icon_button(ui, icon::X, 14.0, "Remove server", false).clicked() {
                remove = Some(i);
            }
        });
        let state = app.mono_tests.get(&link).and_then(|s| s.lock().unwrap().clone());
        test_line(ui, &state);
        ui.add_space(6.0);
    }
    if let Some(i) = remove {
        let gone = app.mono_servers.remove(i);
        app.mono_tests.remove(gone.trim());
        edited = true;
    }
    if edited {
        app.backend.set_monochrome_servers(&live_mono_servers(app));
    }
}

/// The Test pill for one Monochrome server link (saved or still being typed).
fn mono_test_button(app: &mut App, ui: &mut Ui, link: &str) {
    if link.is_empty() {
        ui.add_space(ui.available_width().min(76.0));
        return;
    }
    let slot = app.mono_tests.entry(link.to_owned()).or_default().clone();
    let state = slot.lock().unwrap().clone();
    if matches!(state, Some(None)) {
        ui.add_space(ui.available_width().min(76.0));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spinner();
        });
        return;
    }
    if pill(ui, "Test", None, false).clicked() {
        *slot.lock().unwrap() = Some(None);
        let (slot, http, link) = (slot, app.backend.http(), link.to_owned());
        app.backend.spawn(async move {
            let (good, msg) = crate::providers::probe_monochrome(&http, &link).await;
            *slot.lock().unwrap() = Some(Some((good, msg)));
        });
    }
}

/// The verdict line under a Test: green when the probe reached a working server, red
/// when the link (or the server) is wrong, nothing while it asks.
fn test_line(ui: &mut Ui, state: &Option<Option<(bool, String)>>) {
    let Some(Some((good, msg))) = state else { return };
    let color = if *good { theme::GREEN } else { theme::RED };
    let label = if *good { format!("{msg}") } else { format!("Didn't work: {msg}") };
    ui.label(RichText::new(label).font(theme::regular(12.5)).color(color));
}

/// The Monochrome servers as one newline-separated list, the way the backend reads them.
fn live_mono_servers(app: &App) -> String {
    app.mono_servers.iter().map(|s| s.trim()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n")
}

/// Settings › General › Updates: the channel to take them from, and a check on demand.
/// A nightly build of the newest changes is published every night there are some; it can
/// break at any time. Stable follows the finished releases only.
fn settings_updates(app: &mut App, ui: &mut Ui) {
    use crate::update::State;
    let mut channel = None;
    settings_section(ui, "Updates", None, None, |ui| {
        let desc = "Stable: finished releases, ready for everyday listening.\nNightly: experimental. It can break at any time.";
        setting_row(ui, "Update channel", Some(desc), |ui| {
            let btn = dropdown_button(ui, if app.update_nightly { "Nightly" } else { "Stable" }, 140.0, "Update channel");
            egui::Popup::menu(&btn).show(|ui| {
                ui.set_width(140.0);
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                ui.add_space(4.0);
                for (label, nightly) in [("Stable", false), ("Nightly", true)] {
                    if option_row(ui, label, nightly == app.update_nightly).clicked() {
                        channel = Some(nightly);
                        ui.close();
                    }
                }
                ui.add_space(4.0);
            });
        });
        divider(ui);
        let status = match app.updater.as_ref().map(|u| u.state()) {
            Some(State::Checking) => "Checking for updates…".to_owned(),
            Some(State::Downloading { percent, .. }) => format!("Downloading… {percent}%"),
            Some(State::Ready { version, .. }) => format!("{version} is downloaded. It installs when you close the app"),
            Some(State::Available { version, .. }) => format!("{version} is out"),
            Some(State::Failed(e)) => format!("The check failed: {e}"),
            _ => format!("You're on {}. Checks run at launch and every six hours.", crate::update::current_version()),
        };
        let mut check = false;
        setting_row(ui, "Check now", Some(&status), |ui| {
            ui.add_enabled_ui(super::updatable(), |ui| {
                check = pill(ui, "Check for updates", None, false).clicked();
            });
        });
        if check {
            if let Some(u) = &app.updater {
                u.set_nightly(app.update_nightly);
                u.check();
            }
        }
    });
    if let Some(nightly) = channel {
        app.set_update_nightly(nightly);
    }
}

/// Settings › General › About: where this app comes from. Most of it — the playback
/// engine, the UI, the jam sync, the installers — is ytm-rs's code, and the credit
/// for that belongs with ytm-rs. The README's Acknowledgements has the whole list.
fn settings_about(ui: &mut Ui) {
    settings_section(ui, "About", None, None, |ui| {
        about_row(ui, "A fork of ytm-rs", "By grapedjuice — most of the code this app runs, from playback to the installers, comes from ytm-rs.", "github.com/grapedjuice/ytm-rs", "https://github.com/grapedjuice/ytm-rs");
        divider(ui);
        about_row(ui, "It continues BitChord-PC-Continued", "By jackychan4891 — in the style of the BitChord Android app by kushagrasinghx, which made the icon and hosts the Listen Together relay.", "github.com/jackychan4891/BitChord-PC-Continued", "https://github.com/jackychan4891/BitChord-PC-Continued");
        divider(ui);
        about_row(ui, "Credits", "Changes and additions by LoserDenji and nxfu, with lyrics providers following ForkChord and ideas from Sonora and Mochy-Music. The full list is in the README's Acknowledgements.", "github.com/LoserDenji/Bitchord-Desktop-Revived", "https://github.com/LoserDenji/Bitchord-Desktop-Revived");
        divider(ui);
        about_row(ui, "License", "BitChord Desktop Revived is free software under the GPL-3.0 license.", "gnu.org/licenses/gpl-3.0", "https://www.gnu.org/licenses/gpl-3.0");
    });
}

/// One About row: like `setting_row`, but the link rides on its own line under the
/// description instead of in the row's control column — a URL long enough to read
/// won't fit in there beside the text without running over it.
fn about_row(ui: &mut Ui, title: &str, desc: &str, label: &str, url: &str) {
    if !filter_show(ui, &format!("{title} {desc}")) {
        return;
    }
    let w = ui.available_width();
    let title_g = widgets::galley(ui, title, theme::semibold(14.5), theme::text(), w);
    let desc_g = widgets::galley_wrapped(ui, desc, theme::regular(12.5), theme::text_dim(), w, 3);
    let (title_h, desc_h) = (title_g.size().y, desc_g.size().y);
    let color = if theme::light() { Color32::from_rgb(0, 102, 204) } else { Color32::from_rgb(62, 166, 255) };
    let link_g = egui::WidgetText::from(RichText::new(label).font(theme::regular(13.5)).color(color))
        .into_galley(ui, Some(egui::TextWrapMode::Wrap), w, egui::FontSelection::Default);
    let text_h = title_h + 3.0 + desc_h + 6.0 + link_g.size().y;
    let h = (text_h + 26.0).max(58.0);
    let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let top = rect.center().y - text_h / 2.0;
    ui.painter().galley(pos2(rect.left(), top), title_g, theme::text());
    ui.painter().galley(pos2(rect.left(), top + title_h + 3.0), desc_g, theme::text_dim());
    let lrect = Rect::from_min_size(pos2(rect.left(), top + title_h + 3.0 + desc_h + 6.0), vec2(w, link_g.size().y));
    let mut lui = ui.new_child(egui::UiBuilder::new().max_rect(lrect).layout(egui::Layout::top_down(egui::Align::Min)));
    lui.hyperlink_to(link_g, url);
}

/// Settings › Downloads.
fn settings_downloads(ui: &mut Ui) {
    settings_section(ui, "Downloads", None, Some("Download songs from their menu, or whole albums and playlists from their page. They're saved at the best quality your account gets, tagged with their cover art, and play from the file, even offline."), |ui| {
        let folder = crate::downloads::folder().display().to_string();
        setting_row(ui, "Download folder", Some(&folder), |ui| {
            if pill(ui, "Change", None, false).clicked() {
                crate::downloads::pick_folder(ui.ctx());
            }
            ui.add_space(6.0);
            if pill(ui, "Open", Some(icon::FOLDER_OPEN), false).clicked() {
                crate::downloads::open_folder();
            }
        });
    });
}

/// Settings › Lyrics: the better-lyrics extension's options.
fn settings_lyrics(app: &mut App, ui: &mut Ui) {
    settings_section(ui, "Lyrics", None, None, |ui| {
        setting_row(ui, "Romanization", Some("Japanese, Korean, Chinese, Cyrillic and other scripts in Latin letters. Timed with the lyrics when the source has it."), |ui| {
            toggle(ui, &mut app.show_roman, "Romanization");
        });
        divider(ui);
        setting_row(ui, "Translation", Some("Translated by Unison, or Google when it can't. Lyrics that ship their own translation use it."), |ui| {
            toggle(ui, &mut app.show_trans, "Translation");
        });
        divider(ui);
        setting_row(ui, "Translate into", None, |ui| {
            let current = super::lyric_prefs::LANGUAGES.iter().find(|(c, _)| *c == app.lyric_prefs.target).map_or(app.lyric_prefs.target.as_str(), |(_, n)| n);
            let btn = dropdown_button(ui, current, 200.0, "Translate into");
            let just_opened = btn.clicked();
            egui::Popup::menu(&btn).show(|ui| {
                ui.set_width(200.0);
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                    ui.add_space(4.0);
                    for (code, name) in super::lyric_prefs::LANGUAGES {
                        let on = app.lyric_prefs.target == *code;
                        let row = option_row(ui, name, on);
                        if on && just_opened {
                            row.scroll_to_me(Some(egui::Align::Center));
                        }
                        if row.clicked() {
                            app.lyric_prefs.target = (*code).to_owned();
                            ui.close();
                        }
                    }
                    ui.add_space(4.0);
                });
            });
        });
        divider(ui);
        setting_row(ui, "Text size", Some("Make the lyrics larger or smaller. Works on top of the theme's own size."), |ui| {
            let (min, max, step) = super::lyric_prefs::TEXT_SCALE;
            let v = &mut app.lyric_prefs.text_scale;
            if ui.add_enabled(*v < max - 0.01, egui::Button::new("+").min_size(vec2(28.0, 26.0))).on_hover_text("Larger").clicked() {
                *v = ((*v + step) * 10.0).round() / 10.0;
            }
            ui.label(egui::RichText::new(format!("{:.0}%", *v * 100.0)).font(theme::semibold(13.0)));
            if ui.add_enabled(*v > min + 0.01, egui::Button::new("−").min_size(vec2(28.0, 26.0))).on_hover_text("Smaller").clicked() {
                *v = ((*v - step) * 10.0).round() / 10.0;
            }
        });
        divider(ui);
        setting_row(ui, "Lyrics at the screen's refresh rate", Some("Synced lyrics animate as fast as your screen refreshes, such as 144 fps on a 144 Hz monitor, instead of 60. Smoother on fast screens, for more CPU and graphics work."), |ui| {
            if toggle(ui, &mut app.lyrics_full_rate, "Lyrics at the screen's refresh rate").changed() {
                // A bundle (Settings › Appearance) set this too; a hand change isn't one.
                app.visual = theme::VisualPreset::Custom;
            }
        });
        divider(ui);
        // Offsets: seconds taken off the lyrics clock, as the extension's dock sets them.
        for (title, desc, which) in [
            ("Timing offset", "Shift all lyrics later (+) or earlier (−).", 0),
            ("Word-synced trim", "Extra shift for syllable and word-synced lyrics.", 1),
            ("Line-synced trim", "Extra shift for line-synced lyrics.", 2),
        ] {
            setting_row(ui, title, Some(desc), |ui| {
                let v = match which {
                    0 => &mut app.lyric_prefs.global_offset,
                    1 => &mut app.lyric_prefs.word_trim,
                    _ => &mut app.lyric_prefs.line_trim,
                };
                if ui.add(egui::Button::new("+").min_size(vec2(28.0, 26.0))).on_hover_text("Later").clicked() {
                    *v = ((*v + 0.1) * 10.0).round() / 10.0;
                }
                ui.label(egui::RichText::new(format!("{:+.1} s", *v).replace("+0.0", "0.0")).font(theme::semibold(13.0)));
                if ui.add(egui::Button::new("−").min_size(vec2(28.0, 26.0))).on_hover_text("Earlier").clicked() {
                    *v = ((*v - 0.1) * 10.0).round() / 10.0;
                }
            });
            divider(ui);
        }
        // Source order: the first enabled source with lyrics for a song is shown; the
        // lyrics toolbar can pick another for that song. The row and the order rows
        // with their arrows are one unit in a search.
        let desc = "Tried in this order. Switch one off to skip it; the toolbar under the lyrics picks a different one for a single song.";
        let names = app.lyric_prefs.order.iter().map(|(p, _)| p.name()).collect::<Vec<_>>().join(" ");
        if filter_show(ui, &format!("Lyrics sources {desc} {names} order arrows")) {
            setting_row_shown(ui, "Lyrics sources", Some(desc), 210.0, |_| {});
            let n = app.lyric_prefs.order.len();
            let mut swap: Option<(usize, usize)> = None;
            for (i, entry) in app.lyric_prefs.order.iter_mut().enumerate() {
                let (p, on) = *entry;
                let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::hover());
                let mut row = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::left_to_right(egui::Align::Center)));
                let mut enabled = on;
                if toggle(&mut row, &mut enabled, p.name()).changed() {
                    entry.1 = enabled;
                }
                row.add_space(10.0);
                let kind = match p.sync() {
                    crate::lyrics::SyncKind::Syllable => "syllable",
                    crate::lyrics::SyncKind::Word => "word",
                    crate::lyrics::SyncKind::Line => "line",
                    crate::lyrics::SyncKind::Unsynced => "plain",
                };
                row.label(egui::RichText::new(p.name()).font(theme::semibold(13.5)).color(if on { theme::text() } else { theme::text_dim() }));
                row.label(egui::RichText::new(kind).font(theme::regular(12.0)).color(theme::text_dim()));
                row.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add_enabled(i + 1 < n, egui::Button::new("↓").min_size(vec2(28.0, 24.0))).on_hover_text("Lower").clicked() {
                        swap = Some((i, i + 1));
                    }
                    if ui.add_enabled(i > 0, egui::Button::new("↑").min_size(vec2(28.0, 24.0))).on_hover_text("Higher").clicked() {
                        swap = Some((i, i - 1));
                    }
                });
            }
            if let Some((a, b)) = swap {
                app.lyric_prefs.order.swap(a, b);
            }
            ui.add_space(6.0);
        }
    });
}

/// Settings › Appearance › Artist pages: the listener's own signature service.
fn settings_artists(app: &mut App, ui: &mut Ui) {
    settings_section(ui, "Artist pages", None, None, |ui| {
        // The row and its link field are one unit in a search.
        let desc = "A link to your own signature API. Artists get their handwritten signature, with “Signed by …” above it, standing in for their name in the page header; without a signature, their name is set in type.";
        if filter_show(ui, &format!("Signature service {desc} link")) {
            setting_row_shown(ui, "Signature service", Some(desc), 210.0, |_| {});
            divider(ui);
            ui.add_space(12.0);
            if super::text_field(ui, "Link", &mut app.signature_api, false, "https://your-signature-service.example").changed() {
                app.signature_reset();
            }
            ui.add_space(12.0);
        }
    });
}

/// Settings › Appearance.
fn settings_appearance(app: &mut App, ui: &mut Ui) {
    settings_section(ui, "Appearance", None, None, |ui| {
        let desc = if cfg!(windows) {
            "Light or dark pages. System follows Windows' app mode. Now playing stays dark, over the album art."
        } else {
            "Light or dark pages. System follows your system's setting. Now playing stays dark, over the album art."
        };
        let mut pick = None;
        setting_row(ui, "Mode", Some(desc), |ui| {
            let btn = dropdown_button(ui, app.appearance.label(), 160.0, "Mode");
            egui::Popup::menu(&btn).show(|ui| {
                ui.set_width(160.0);
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                ui.add_space(4.0);
                for a in theme::Appearance::ALL {
                    if option_row(ui, a.label(), a == app.appearance).clicked() {
                        pick = Some(a);
                        ui.close();
                    }
                }
                ui.add_space(4.0);
            });
        });
        if let Some(a) = pick {
            app.appearance = a;
        }
        divider(ui);
        // The row and its bundle: picking one sets every setting it owns, and changing
        // any of those by hand (here or in Lyrics, for the refresh rate) returns to
        // Custom, so the label never claims a bundle that isn't in force.
        setting_row(ui, "Visual preset", Some("A bundle of the visual settings below. Barebones turns every animation and glow off for older computers — Now playing keeps the theme's own backdrop, held still, and the lyrics take the Light renderer; nothing about playback changes. High visual turns them all on. Changing any of these settings afterwards returns to Custom."), |ui| {
            let mut pick = None;
            let btn = dropdown_button(ui, app.visual.label(), 160.0, "Visual preset");
            egui::Popup::menu(&btn).show(|ui| {
                ui.set_width(160.0);
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                ui.add_space(4.0);
                for v in theme::VisualPreset::ALL {
                    if option_row(ui, v.label(), v == app.visual).clicked() {
                        pick = Some(v);
                        ui.close();
                    }
                }
                ui.add_space(4.0);
            });
            if let Some(v) = pick {
                app.visual = v;
                app.apply_visual(Some(ui.ctx()));
            }
        });
        divider(ui);
        setting_row(ui, "Animated background", Some("The album art slowly drifts behind everything."), |ui| {
            if toggle(ui, &mut app.anim_bg, "Animated background").changed() {
                app.visual = theme::VisualPreset::Custom;
            }
        });
        divider(ui);
        setting_row(ui, "Pulse with the music", Some("The background swells with the beat."), |ui| {
            if toggle(ui, &mut app.reactive_bg, "Pulse with the music").changed() {
                app.visual = theme::VisualPreset::Custom;
            }
        });
        divider(ui);
        setting_row(ui, "Disable heavy background blur", Some("Replaces intensive GPU shader blur effects with a clean static ambient tint. Drastically reduces GPU usage, heat and battery consumption on integrated GPUs."), |ui| {
            if toggle(ui, &mut app.disable_blur, "Disable heavy background blur").changed() {
                app.visual = theme::VisualPreset::Custom;
            }
        });
        divider(ui);
        setting_row(ui, "Ambient light", Some("Now playing glows with the video's colours around it."), |ui| {
            if toggle(ui, &mut app.ambient_light, "Ambient light").changed() {
                app.visual = theme::VisualPreset::Custom;
            }
        });
        divider(ui);
        setting_row(ui, "Animated covers", Some("Where Apple Music has a clip for a song or its album, the cover moves in Now playing and on the album page, looping while the music plays and holding still when it's paused. Finding them sends each song's title and artist to Apple Music, remembered on disk so each song asks once."), |ui| {
            if toggle(ui, &mut app.animated_covers, "Animated covers").changed() {
                app.visual = theme::VisualPreset::Custom;
            }
        });
        divider(ui);
        setting_row(ui, "Now playing backdrop", Some("What plays behind the full-screen player: the theme's own, or one of the alternatives without editing the theme."), |ui| {
            use crate::preset::BgMode;
            let label = |m: Option<BgMode>| match m {
                None => "Theme default".to_owned(),
                Some(m) => match m {
                    BgMode::Kawarp => "Animated warp".to_owned(),
                    BgMode::CoverStatic => "Blurred cover".to_owned(),
                    BgMode::AccentGradient => "Flat accent".to_owned(),
                    BgMode::Solid => "Solid colour".to_owned(),
                    BgMode::CoverFullbleed => "Full-bleed cover".to_owned(),
                },
            };
            let mut pick: Option<Option<BgMode>> = None;
            let btn = dropdown_button(ui, &label(app.np_bg), 220.0, "Now playing backdrop");
            egui::Popup::menu(&btn).show(|ui| {
                ui.set_width(220.0);
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                ui.add_space(4.0);
                for option in [None, Some(BgMode::Kawarp), Some(BgMode::CoverStatic), Some(BgMode::AccentGradient), Some(BgMode::Solid), Some(BgMode::CoverFullbleed)] {
                    if option_row(ui, &label(option), option == app.np_bg).clicked() {
                        pick = Some(option);
                        ui.close();
                    }
                }
                ui.add_space(4.0);
            });
            if let Some(p) = pick {
                app.np_bg = p;
                app.visual = theme::VisualPreset::Custom;
            }
        });
        divider(ui);
        setting_row(ui, "Reduce motion", Some("Every transition lands at once — panels, hover states, the now-playing slide — and a record cover stops spinning. The lyrics' own sweep keeps its timing; how fast it redraws is set in Lyrics."), |ui| {
            if toggle(ui, &mut app.reduce_motion, "Reduce motion").changed() {
                theme::set_motion(!app.reduce_motion);
                theme::install_style(ui.ctx());
                app.visual = theme::VisualPreset::Custom;
            }
        });
        divider(ui);
        setting_row(ui, "Cover shadow and glow", Some("The soft shadow and the glow under the cover in Now playing."), |ui| {
            if toggle(ui, &mut app.cover_fx, "Cover shadow and glow").changed() {
                app.visual = theme::VisualPreset::Custom;
            }
        });
        divider(ui);
        setting_row(ui, "Lyric blur and glow", Some("The theme's blur on lines still to come and glow on the word being sung, in Now playing. Off leaves every line sharp and plain — much less work per frame."), |ui| {
            if toggle(ui, &mut app.lyric_fx, "Lyric blur and glow").changed() {
                app.visual = theme::VisualPreset::Custom;
            }
        });
        divider(ui);
        // Moved in from Lyrics: a visual effect among the others, so the presets reach it
        // (Barebones leaves it off with every other glow).
        setting_row(ui, "Intensive glow", Some("Light the word being sung with a far stronger glow than the theme's own, the way am-lyrics does it. Strength sets how far it goes."), |ui| {
            if toggle(ui, &mut app.lyric_prefs.glow_boost, "Intensive glow").changed() {
                app.visual = theme::VisualPreset::Custom;
            }
        });
        if app.lyric_prefs.glow_boost {
            divider(ui);
            setting_row(ui, "Glow strength", Some("How far the intensive glow goes, from a hint to all the way in."), |ui| {
                let (min, max, step) = super::lyric_prefs::GLOW_STRENGTH;
                let v = &mut app.lyric_prefs.glow_strength;
                if ui.add_enabled(*v < max - 0.01, egui::Button::new("+").min_size(vec2(28.0, 26.0))).on_hover_text("Stronger").clicked() {
                    *v = ((*v + step) * 10.0).round() / 10.0;
                }
                ui.label(egui::RichText::new(format!("{:.0}%", *v * 100.0)).font(theme::semibold(13.0)));
                if ui.add_enabled(*v > min + 0.01, egui::Button::new("−").min_size(vec2(28.0, 26.0))).on_hover_text("Softer").clicked() {
                    *v = ((*v - step) * 10.0).round() / 10.0;
                }
            });
        }
        divider(ui);
        setting_row(ui, "Lyrics renderer", Some("How the synced lyrics are drawn. Full is every per-letter animation; Light, ported from Sonora, draws whole words with a plain clipped sweep and one eased scroll — much the same look for a fraction of the work each frame. It gives up the letter waves, the words' sideways pop and the line's swell; Barebones uses it."), |ui| {
            let mut pick = None;
            let btn = dropdown_button(ui, if app.lyric_light { "Light" } else { "Full" }, 160.0, "Lyrics renderer");
            egui::Popup::menu(&btn).show(|ui| {
                ui.set_width(160.0);
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                ui.add_space(4.0);
                for (label, light) in [("Full", false), ("Light", true)] {
                    if option_row(ui, label, light == app.lyric_light).clicked() {
                        pick = Some(light);
                        ui.close();
                    }
                }
                ui.add_space(4.0);
            });
            if let Some(light) = pick {
                app.lyric_light = light;
                app.visual = theme::VisualPreset::Custom;
            }
        });
        divider(ui);
        setting_row(ui, "Accent-coloured controls", Some("The play button and volume slider in the accent colour, like the progress line."), |ui| {
            toggle(ui, &mut app.accent_controls, "Accent-coloured controls");
        });
        divider(ui);
        setting_row(ui, "Volume control", Some("How the volume shows at Now playing's left edge. Bar is the slim slider that grows out of the margin; Edge dial is the half disc pressed on the window's border, its level read round the rim. Bar is the app's own look."), |ui| {
            let mut pick = None;
            let btn = dropdown_button(ui, if app.np_vol_dial { "Edge dial" } else { "Bar" }, 160.0, "Volume control");
            egui::Popup::menu(&btn).show(|ui| {
                ui.set_width(160.0);
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                ui.add_space(4.0);
                for (label, dial) in [("Bar", false), ("Edge dial", true)] {
                    if option_row(ui, label, dial == app.np_vol_dial).clicked() {
                        pick = Some(dial);
                        ui.close();
                    }
                }
                ui.add_space(4.0);
            });
            if let Some(dial) = pick {
                app.np_vol_dial = dial;
            }
        });
        divider(ui);
        setting_row(ui, "Font", Some("For the whole app and the lyrics. Any font installed on this computer works, e.g. one made for your language."), |ui| {
            if let Some(name) = font_picker(ui) {
                theme::set_font_choice(&name);
                app.chrome_applied = None;
            }
        });
        divider(ui);
        let hidden = app.hidden_playlists.len();
        let desc = match hidden {
            0 => "Nothing hidden. Right-click a playlist in the sidebar to hide it from there.".to_owned(),
            1 => "1 playlist is hidden from the sidebar. Right-click a playlist there to hide or show it.".to_owned(),
            n => format!("{} playlists are hidden from the sidebar. Right-click a playlist there to hide or show it.", thousands(n as u64)),
        };
        setting_row(ui, "Hidden playlists", Some(&desc), |ui| {
            if hidden > 0 && pill(ui, "Show all", None, false).clicked() {
                app.hidden_playlists.clear();
            }
        });
        divider(ui);
        settings_zoom(ui);
    });
}

/// Settings › Appearance › Zoom: the size of everything, as Ctrl + plus / minus sets it.
fn settings_zoom(ui: &mut Ui) {
    const STEPS: [f32; 9] = [0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0];
    let pct = |z: f32| format!("{}%", (z * 100.0).round() as i32);
    let zoom = ui.ctx().zoom_factor();
    let mut pick = None;
    setting_row(ui, "Zoom", Some("Makes everything bigger or smaller. Ctrl + plus and Ctrl + minus do the same, and Ctrl + 0 goes back to 100%."), |ui| {
        let btn = dropdown_button(ui, &pct(zoom), 120.0, "Zoom");
        egui::Popup::menu(&btn).show(|ui| {
            ui.set_width(120.0);
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            ui.add_space(4.0);
            for z in STEPS {
                if option_row(ui, &pct(z), (z - zoom).abs() < 0.005).clicked() {
                    pick = Some(z);
                    ui.close();
                }
            }
            ui.add_space(4.0);
        });
    });
    if let Some(z) = pick {
        ui.ctx().set_zoom_factor(z);
    }
}

/// Settings › General's first card: the project's Discord server — the same place the
/// update catalogue's news lands first.
fn settings_community(ui: &mut Ui) {
    settings_section(ui, "Community", None, None, |ui| {
        setting_row(ui, "Join the Discord server", Some("Release news as it happens, help with the app, and the talk the project runs on."), |ui| {
            if pill(ui, "Join server", Some(icon::USERS), true).clicked() {
                ui.ctx().open_url(egui::OpenUrl::new_tab(crate::discord::DISCORD_INVITE));
            }
        });
    });
}

/// Settings › Window.
fn settings_window(app: &mut App, ui: &mut Ui) {
    settings_section(ui, "Window", None, None, |ui| {
        setting_row(ui, "Close to tray", Some("Clicking × hides the window and keeps the music playing. The tray icon is always up: left-click brings the window back, and its menu has the transport buttons, Launch at login and Quit. If the icon never shows up, launching BitChord again brings the window back; --no-tray starts a session without an icon, where × quits for real. On Linux, clicking the song in the system's media controls brings it back too, and Ctrl+Q quits."), |ui| {
            toggle(ui, &mut app.close_to_tray, "Close to tray");
        });
        if let Some(e) = &app.tray_error {
            ui.label(RichText::new(e).font(theme::regular(12.5)).color(theme::RED));
            ui.add_space(8.0);
        }
        if crate::notify::SUPPORTED {
            divider(ui);
            let ok = crate::notify::available();
            let desc = if ok {
                "When a new song starts while the window is minimized, in the tray or behind other windows, show its title, artist and cover in a notification."
            } else {
                "Needs the installed app: Windows only shows notifications from apps in the Start menu."
            };
            ui.add_enabled_ui(ok, |ui| {
                setting_row(ui, "Song notifications", Some(desc), |ui| {
                    toggle(ui, &mut app.song_notifications, "Song notifications");
                });
            });
        }
        divider(ui);
        setting_row(ui, "Launch at login", Some("Start BitChord when the computer does. The OS keeps this: the Run key on Windows, an autostart entry on Linux, a launch agent on macOS — and the tray menu's own Launch at login item flips the same switch."), |ui| {
            if toggle(ui, &mut app.launch_at_login, "Launch at login").changed() {
                crate::autostart::set(app.launch_at_login);
            }
        });
        divider(ui);
        if cfg!(target_os = "linux") {
            use super::titlebar::Choice;
            let dir = app.backend.data_dir().to_owned();
            let saved = Choice::load(&dir);
            setting_row(ui, "Title bar", Some("Who draws the window's title bar and buttons. Automatic uses Bitchord's where the desktop draws none (GNOME on Wayland)."), |ui| {
                let btn = dropdown_button(ui, saved.label(), 120.0, "Title bar");
                egui::Popup::menu(&btn).show(|ui| {
                    ui.set_width(120.0);
                    ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                    ui.add_space(4.0);
                    for c in Choice::ALL {
                        if option_row(ui, c.label(), c == saved).clicked() {
                            if let Err(e) = c.save(&dir) {
                                log::warn!("title bar: couldn't save the setting: {e}");
                            }
                            ui.close();
                        }
                    }
                    ui.add_space(4.0);
                });
            });
            divider(ui);
            if saved.own_here() != super::titlebar::custom() && std::env::var_os("YTM_NATIVE_TITLEBAR").is_none() {
                let mut restart = false;
                setting_row(ui, "Restart to apply", Some("The title bar changes from the next start."), |ui| {
                    restart = pill(ui, "Restart now", None, true).clicked();
                });
                if restart {
                    app.restart(ui.ctx());
                }
                divider(ui);
            }
        }
        setting_row(ui, "Collapse sidebar", Some("Show only icons in the sidebar, for narrow windows. The menu button at its top does the same."), |ui| {
            toggle(ui, &mut app.sidebar_collapsed, "Collapse sidebar");
        });
        divider(ui);
        let api = crate::renderer::api_name();
        let title = format!("Draw with {api}");
        let desc = format!(
            "Use {api} (wgpu) instead of OpenGL. It isn't faster for this app and takes about three times the memory; try it if OpenGL draws wrong on this computer. If {api} won't start, the app goes back to OpenGL."
        );
        setting_row(ui, &title, Some(&desc), |ui| {
            if toggle(ui, &mut app.wgpu_renderer, &title).changed() {
                use crate::renderer::Renderer;
                let r = if app.wgpu_renderer { Renderer::Wgpu } else { Renderer::OpenGl };
                if let Err(e) = crate::renderer::save(app.backend.data_dir(), r) {
                    log::warn!("renderer: couldn't save the setting: {e}");
                }
            }
        });
        if app.wgpu_renderer != (crate::renderer::active() == crate::renderer::Renderer::Wgpu) {
            divider(ui);
            let mut restart = false;
            setting_row(ui, "Restart to apply", Some("The app draws with the new renderer from the next start."), |ui| {
                restart = pill(ui, "Restart now", None, true).clicked();
            });
            if restart {
                app.restart(ui.ctx());
            }
        }
    });
}

/// Settings › Account › Connections: Last.fm scrobbling.
fn settings_connections(app: &mut App, ui: &mut Ui) {
    settings_section(ui, "Connections", None, None, |ui| {
        let busy = app.lastfm_busy.load(Ordering::Relaxed);
        let desc = match &app.lastfm_user {
            Some(name) => format!("Scrobbling what you play to {name}'s profile."),
            None if busy => "Allow BitChord on the Last.fm page that opened in your browser.".to_owned(),
            None => "Scrobble what you play to your Last.fm profile.".to_owned(),
        };
        let (mut lf_connect, mut lf_disconnect) = (false, false);
        setting_row(ui, "Last.fm", Some(&desc), |ui| {
            if app.lastfm_user.is_some() {
                lf_disconnect = pill(ui, "Disconnect", None, false).clicked();
            } else if busy {
                ui.spinner();
            } else {
                lf_connect = pill(ui, "Connect", None, false).clicked();
            }
        });
        if lf_connect {
            app.lastfm_sign_in();
        }
        if lf_disconnect {
            app.lastfm_sign_out();
        }
    });
}

/// Settings › Network: the proxy everything goes through, a connection test, and a
/// restart when the setting changed.
fn network(app: &mut App, ui: &mut Ui) {
    use crate::proxy::{self, Setting};
    const HIGHLIGHT_SECS: f64 = 3.0;
    setting_row(ui, "Backup stream", Some("Play songs through Safari's HLS when YouTube refuses the usual stream partway, as VPNs make it. Starts each song a little slower."), |ui| {
        let response = toggle(ui, &mut app.backup_stream, "Backup stream");
        let now = ui.input(|input| input.time);
        if std::mem::take(&mut app.reveal_backup_setting) {
            response.scroll_to_me(Some(egui::Align::Center));
            response.request_focus();
            app.backup_setting_highlight_until = now + HIGHLIGHT_SECS;
        }
        let remaining = app.backup_setting_highlight_until - now;
        if remaining > 0.0 {
            let elapsed = (HIGHLIGHT_SECS - remaining) as f32;
            let pulse = 0.5 + 0.5 * (elapsed * std::f32::consts::TAU).cos();
            let fade = (remaining / 0.25).min(1.0) as f32;
            let color = theme::with_alpha(app.accent_color(), (0.2 + 0.8 * pulse) * fade);
            let stroke = egui::Stroke::new(2.0, color);
            ui.painter().rect_stroke(response.rect.expand(4.0), response.rect.height() / 2.0 + 4.0, stroke, egui::StrokeKind::Outside);
            ui.ctx().request_repaint();
        }
        if response.changed() {
            app.backend.set_backup_stream(app.backup_stream);
        }
    });
    divider(ui);
    let same = |a: &Setting, b: &Setting| std::mem::discriminant(a) == std::mem::discriminant(b);
    let mut pick = None;
    setting_row(ui, "Proxy", Some("Send everything BitChord fetches through a proxy: songs, lyrics, sign-in and Listen together."), |ui| {
        let btn = dropdown_button(ui, app.proxy_mode.label(), 220.0, "Proxy");
        egui::Popup::menu(&btn).show(|ui| {
            ui.set_width(220.0);
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            ui.add_space(4.0);
            for s in [Setting::System, Setting::Off, Setting::Custom(String::new())] {
                if option_row(ui, s.label(), same(&s, &app.proxy_mode)).clicked() {
                    pick = Some(s);
                    ui.close();
                }
            }
            ui.add_space(4.0);
        });
    });
    if let Some(s) = pick {
        app.proxy_error = None;
        *app.proxy_test.lock().unwrap() = None;
        app.proxy_mode = s.clone();
        // System and Off take effect as picked; a custom address once it's saved.
        if !matches!(s, Setting::Custom(_)) {
            match proxy::save(app.backend.data_dir(), &s) {
                Ok(()) => app.proxy_saved = s,
                Err(e) => app.proxy_error = Some(format!("{e:#}")),
            }
        }
    }
    match &app.proxy_mode {
        Setting::System => {
            divider(ui);
            match (proxy::system(), cfg!(windows)) {
                (Some(p), true) => {
                    setting_row(ui, "System proxy", Some(&format!("Windows is set to use {}.", proxy::redact(&p))), |_| {});
                }
                (None, true) => {
                    setting_row(ui, "System proxy", Some("Windows isn't set to use a proxy, so BitChord connects directly."), |_| {});
                }
                (Some(p), false) => {
                    setting_row(ui, "System proxy", Some(&format!("The environment sets {}.", proxy::redact(&p))), |_| {});
                }
                (None, false) => {
                    setting_row(ui, "System proxy", Some("No HTTPS_PROXY or ALL_PROXY is set, so BitChord connects directly."), |_| {});
                }
            }
        }
        Setting::Custom(_) => {
            divider(ui);
            // The address, its Save and the note are the Proxy row's unit in a search.
            if filter_show(ui, "Proxy custom address save password secret store") {
                ui.add_space(12.0);
                super::text_field(ui, "Address", &mut app.proxy_url, false, "http://127.0.0.1:8080 or socks5://user:pass@host:1080");
                ui.add_space(10.0);
                let custom = Setting::Custom(app.proxy_url.trim().to_owned());
                let saved = app.proxy_saved == custom;
                let mut save = false;
                ui.horizontal(|ui| {
                    save = pill(ui, if saved { "Saved" } else { "Save" }, None, !saved).clicked() && !saved;
                    ui.add_space(8.0);
                    ui.label(RichText::new(format!("A password in the address is kept in {}.", super::secret_store())).font(theme::regular(12.5)).color(theme::text_dim()));
                });
                if save {
                    match proxy::save(app.backend.data_dir(), &custom) {
                        Ok(()) => {
                            app.proxy_saved = custom;
                            app.proxy_error = None;
                        }
                        Err(e) => app.proxy_error = Some(format!("{e:#}")),
                    }
                }
                ui.add_space(12.0);
            }
        }
        Setting::Off => {}
    }
    if let Some(e) = &app.proxy_error
        && filter_show(ui, "proxy error didn't connect save")
    {
        ui.label(RichText::new(e).font(theme::regular(12.5)).color(theme::RED));
        ui.add_space(8.0);
    }
    divider(ui);
    let result = app.proxy_test.lock().unwrap().clone();
    let desc = match &result {
        Some(Some((_, msg))) => msg.clone(),
        Some(None) => "Checking…".to_owned(),
        None => "Checks that YouTube Music answers through this setting.".to_owned(),
    };
    setting_row(ui, "Test connection", Some(&desc), |ui| {
        if matches!(result, Some(None)) {
            ui.spinner();
        } else if pill(ui, "Test", None, false).clicked() {
            let s = match &app.proxy_mode {
                Setting::Custom(_) => Setting::Custom(app.proxy_url.trim().to_owned()),
                other => other.clone(),
            };
            let (slot, b) = (app.proxy_test.clone(), app.backend.clone());
            *slot.lock().unwrap() = Some(None);
            app.backend.spawn(async move {
                let out = match proxy::test(&s).await {
                    Ok(t) => (true, format!("Working: YouTube Music answered in {} ms.", t.as_millis())),
                    Err(e) => (false, format!("Didn't connect: {e:#}")),
                };
                *slot.lock().unwrap() = Some(Some(out));
                b.wake();
            });
        }
    });
    if app.proxy_saved != *proxy::active() {
        divider(ui);
        let mut restart = false;
        setting_row(ui, "Restart to apply", Some("The new proxy setting is used from the next start."), |ui| {
            restart = pill(ui, "Restart now", None, true).clicked();
        });
        if restart {
            app.restart(ui.ctx());
        }
    }
}

/// The font setting: the theme's own, a bundled face, or one installed on this PC.
/// Returns the new choice ("" for the theme's).
fn font_picker(ui: &mut Ui) -> Option<String> {
    let current = theme::font_choice();
    let btn = dropdown_button(ui, if current.is_empty() { "Theme's font" } else { &current }, 220.0, "Font");
    let just_opened = btn.clicked();
    let mut picked = None;
    egui::Popup::menu(&btn).show(|ui| {
        ui.set_width(220.0);
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
            ui.add_space(4.0);
            let mut row = |ui: &mut Ui, label: &str, value: &str| {
                let on = current.eq_ignore_ascii_case(value);
                let r = option_row(ui, label, on);
                if on && just_opened {
                    r.scroll_to_me(Some(egui::Align::Center));
                }
                if r.clicked() {
                    picked = Some(value.to_owned());
                    ui.close();
                }
            };
            row(ui, "Theme's font", "");
            for name in theme::BUNDLED_FONTS {
                row(ui, name, name);
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                ui.label(RichText::new("Installed on this computer").font(theme::semibold(12.0)).color(theme::text_faint()));
            });
            ui.add_space(4.0);
            let ctx = ui.ctx().clone();
            match theme::installed_fonts(move || ctx.request_repaint()) {
                Some(fonts) => {
                    for f in fonts {
                        row(ui, &f.name, &f.name);
                    }
                }
                None => {
                    ui.horizontal(|ui| {
                        ui.add_space(12.0);
                        ui.label(RichText::new("Looking for fonts…").font(theme::regular(13.0)).color(theme::text_dim()));
                    });
                }
            }
            ui.add_space(4.0);
        });
    });
    picked.filter(|p| *p != current)
}

fn output_device_picker(app: &mut App, ui: &mut Ui) {
    let default_label = super::system_device_label();
    let current = match &app.output_device {
        None => default_label.to_owned(),
        Some((id, name)) => match app.output_devices.iter().find(|d| d.id == *id) {
            Some(d) => d.name.clone(),
            None if app.output_devices.is_empty() => name.clone(),
            None => format!("{name} (not connected)"),
        },
    };
    let btn = dropdown_button(ui, &current, 210.0, "Output device");
    // Listed when opened, so a device that just connected shows up.
    if btn.clicked() || app.output_devices.is_empty() {
        app.output_devices = crate::audio::output_devices();
    }
    let mut pick: Option<Option<(String, String)>> = None;
    egui::Popup::menu(&btn).show(|ui| {
        ui.set_width(340.0);
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
            ui.add_space(4.0);
            if option_row(ui, default_label, app.output_device.is_none()).clicked() {
                pick = Some(None);
                ui.close();
            }
            for d in &app.output_devices {
                if option_row(ui, &d.name, app.output_device.as_ref().is_some_and(|(id, _)| *id == d.id)).clicked() {
                    pick = Some(Some((d.id.clone(), d.name.clone())));
                    ui.close();
                }
            }
            ui.add_space(4.0);
        });
    });
    if let Some(pick) = pick {
        app.audio.send(crate::audio::AudioCmd::SetDevice(pick.as_ref().map(|(id, _)| id.clone())));
        app.output_device = pick;
    }
}

/// Volume normalization's description: what the levels do, and what it does to the song playing.
fn normalization_desc(app: &App) -> String {
    use crate::audio::Normalize;
    let what = match app.normalize {
        Normalize::Off => return "Songs play as loud as they were made. Normal brings them all to the same level, and Quiet to the same, lower one.".to_owned(),
        Normalize::Normal => "Songs play at the same level: loud ones turned down, as on YouTube Music, and quiet ones up by as much as 6 dB.",
        Normalize::Quiet => "Songs play at the same, lower level: loud ones turned down further, so quiet ones rarely need turning up.",
    };
    match (app.current().is_some(), super::shell::normalization_note(app)) {
        (true, Some(note)) => format!("{what} This song: {note}."),
        _ => what.to_owned(),
    }
}

/// A dropdown's closed state: the chosen value and a chevron on a glass pill.
pub(super) fn dropdown_button(ui: &mut Ui, value: &str, width: f32, name: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 36.0), Sense::click());
    let resp = widgets::named(resp, name).on_hover_cursor(egui::CursorIcon::PointingHand);
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&resp));
    let h = theme::anim_bool(ui.ctx(), resp.id.with("h"), resp.hovered() || open, 0.12);
    ui.painter().rect_filled(rect, 10.0, theme::glass(0.08 + 0.05 * h));
    widgets::text_at(ui, pos2(rect.left() + 14.0, rect.center().y - 9.0), value, theme::regular(14.0), theme::text(), rect.width() - 44.0);
    let glyph = if open { theme::icon::CHEVRON_UP } else { theme::icon::CHEVRON_DOWN };
    widgets::icon_at(ui, pos2(rect.right() - 18.0, rect.center().y), glyph, 15.0, theme::lerp_color(theme::text_dim(), theme::text(), h));
    resp
}

/// A dropdown option: hover glass, and a check on the chosen one.
pub(super) fn option_row(ui: &mut Ui, label: &str, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click());
    let resp = widgets::named(resp, label).on_hover_cursor(egui::CursorIcon::PointingHand);
    let h = theme::anim_bool(ui.ctx(), resp.id.with("h"), resp.hovered(), 0.12);
    if h > 0.0 || selected {
        ui.painter().rect_filled(rect, 8.0, theme::glass(0.05 * selected as u8 as f32 + 0.08 * h));
    }
    let font = if selected { theme::semibold(14.0) } else { theme::regular(14.0) };
    widgets::text_at(ui, pos2(rect.left() + 12.0, rect.center().y - 9.0), label, font, theme::text(), rect.width() - 44.0);
    if selected {
        widgets::icon_at(ui, pos2(rect.right() - 16.0, rect.center().y), theme::icon::CHECK, 15.0, theme::text());
    }
    resp
}

/// 12345 -> "12,345".
pub(super) fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Small heading above a settings card, with an optional value on the right.
pub(super) fn section_label(ui: &mut Ui, title: &str, value: Option<&str>) {
    ui.add_space(30.0);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::hover());
    ui.painter().text(pos2(rect.left() + 2.0, rect.center().y), Align2::LEFT_CENTER, title, theme::bold(17.0), theme::text());
    if let Some(v) = value {
        ui.painter().text(pos2(rect.right() - 2.0, rect.center().y), Align2::RIGHT_CENTER, v, theme::semibold(13.0), theme::text_dim());
    }
    ui.add_space(10.0);
}

pub(super) fn settings_card(ui: &mut Ui, body: impl FnOnce(&mut Ui)) {
    settings_frame().show(ui, |ui| {
        ui.set_width(ui.available_width());
        body(ui);
    });
}

fn settings_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(theme::glass(0.06))
        .stroke(egui::Stroke::new(1.0, theme::glass(0.05)))
        .corner_radius(theme::panel_radius())
        .inner_margin(egui::Margin::symmetric(20, 6))
}

/// The value beside a section's heading: in a search, the category it's in.
fn settings_value(value: Option<&str>) -> Option<String> {
    FILTER.with_borrow(|f| if f.words.is_empty() { value.map(str::to_owned) } else { Some(f.tab.to_owned()) })
}

/// A section of Settings: its heading, a line about it, and a card of rows. In a search
/// it shows only the rows that match, and nothing at all if none do.
pub(super) fn settings_section(ui: &mut Ui, title: &str, value: Option<&str>, note: Option<&str>, body: impl FnOnce(&mut Ui)) {
    if !filtering() {
        section_label(ui, title, value);
        if let Some(n) = note {
            ui.label(RichText::new(n).font(theme::regular(13.0)).color(theme::text_dim()));
            ui.add_space(12.0);
        }
        settings_card(ui, body);
        return;
    }
    FILTER.with_borrow_mut(|f| {
        f.section = title.to_owned();
        f.in_section = 0;
    });
    // In a child, for whether it had anything to show to decide if it takes up room.
    const HEADING: f32 = 60.0;
    let area = ui.available_rect_before_wrap();
    let mut child = ui.new_child(egui::UiBuilder::new().id_salt(("settings-section", title)).max_rect(area).layout(egui::Layout::top_down(egui::Align::Min)));
    child.add_space(HEADING);
    let mut card = settings_frame().begin(&mut child);
    let w = card.content_ui.available_width();
    card.content_ui.set_width(w);
    body(&mut card.content_ui);
    if FILTER.with_borrow(|f| f.in_section > 0) {
        card.end(&mut child);
        let heading = Rect::from_min_size(pos2(area.left(), area.top() + 30.0), vec2(area.width(), 20.0));
        ui.painter().text(pos2(heading.left() + 2.0, heading.center().y), Align2::LEFT_CENTER, title, theme::bold(17.0), theme::text());
        if let Some(v) = settings_value(value).filter(|v| *v != title) {
            ui.painter().text(pos2(heading.right() - 2.0, heading.center().y), Align2::RIGHT_CENTER, v, theme::semibold(13.0), theme::text_dim());
        }
        ui.advance_cursor_after_rect(child.min_rect());
    }
}

/// A settings row: title and description on the left, the control on the right. A search
/// that doesn't find it leaves it out.
pub(super) fn setting_row(ui: &mut Ui, title: &str, desc: Option<&str>, control: impl FnOnce(&mut Ui)) {
    if !filter_show(ui, &format!("{title} {}", desc.unwrap_or_default())) {
        return;
    }
    setting_row_shown(ui, title, desc, 210.0, control);
}

/// A settings row whose controls take `control_w` rather than one control's 230, for a
/// dropdown with a switch beside it (Discord's card lines).
pub(super) fn setting_row_wide(ui: &mut Ui, title: &str, desc: Option<&str>, control_w: f32, control: impl FnOnce(&mut Ui)) {
    if !filter_show(ui, &format!("{title} {}", desc.unwrap_or_default())) {
        return;
    }
    setting_row_shown(ui, title, desc, control_w, control);
}

/// The row itself, without the filter check: for a block that's gated as one unit — a row
/// and the widgets that belong to it show together or not at all.
fn setting_row_shown(ui: &mut Ui, title: &str, desc: Option<&str>, control_w: f32, control: impl FnOnce(&mut Ui)) {
    let w = ui.available_width();
    let text_w = (w - control_w - 20.0).max(200.0);
    let title_g = widgets::galley(ui, title, theme::semibold(14.5), theme::text(), text_w);
    // Up to three lines, and as many again for each line the description starts itself.
    let desc_g = desc.map(|d| widgets::galley_wrapped(ui, d, theme::regular(12.5), theme::text_dim(), text_w, 3 * (1 + d.matches('\n').count())));
    let text_h = title_g.size().y + desc_g.as_ref().map_or(0.0, |g| g.size().y + 3.0);
    let h = (text_h + 26.0).max(58.0);
    let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let fade = if ui.is_enabled() { 1.0 } else { 0.45 };
    let top = rect.center().y - text_h / 2.0;
    ui.painter().galley(pos2(rect.left(), top), title_g.clone(), theme::with_alpha(theme::text(), fade));
    if let Some(g) = desc_g {
        ui.painter().galley(pos2(rect.left(), top + title_g.size().y + 3.0), g, theme::with_alpha(theme::text_dim(), fade));
    }
    let ctl = Rect::from_min_max(pos2(rect.left() + text_w + 20.0, rect.top()), rect.max);
    let mut cui = ui.new_child(egui::UiBuilder::new().max_rect(ctl).layout(egui::Layout::right_to_left(egui::Align::Center)));
    control(&mut cui);
}

pub(super) fn divider(ui: &mut Ui) {
    // A search has rows left out: each one it shows draws its own.
    if !filtering() {
        divider_line(ui);
    }
}

/// A settings row that opens somewhere else: the whole row is the button, with a
/// chevron on the right. A search that doesn't find it leaves it out.
pub(super) fn open_row(ui: &mut Ui, title: &str, desc: &str) -> bool {
    if !filter_show(ui, &format!("{title} {desc}")) {
        return false;
    }
    let w = ui.available_width();
    let text_w = (w - 230.0).max(200.0);
    let title_g = widgets::galley(ui, title, theme::semibold(14.5), theme::text(), text_w);
    let desc_g = widgets::galley_wrapped(ui, desc, theme::regular(12.5), theme::text_dim(), text_w, 3);
    let text_h = title_g.size().y + desc_g.size().y + 3.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(w, (text_h + 26.0).max(58.0)), Sense::click());
    let resp = named(resp, title).on_hover_cursor(egui::CursorIcon::PointingHand);
    let top = rect.center().y - text_h / 2.0;
    let title_h = title_g.size().y;
    ui.painter().galley(pos2(rect.left(), top), title_g, theme::text());
    ui.painter().galley(pos2(rect.left(), top + title_h + 3.0), desc_g, theme::text_dim());
    widgets::icon_at(ui, pos2(rect.right() - 8.0, rect.center().y), icon::CHEVRON_RIGHT, 16.0, theme::text_faint());
    resp.clicked()
}

fn divider_line(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, theme::glass(0.07));
}

/// On/off switch tinted with the accent.
pub(super) fn toggle(ui: &mut Ui, on: &mut bool, name: &str) -> egui::Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(44.0, 26.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let value = *on;
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, ui.is_enabled(), value, name));
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let t = theme::anim_bool(ui.ctx(), resp.id.with("t"), value, 0.16);
    let h = theme::anim_bool(ui.ctx(), resp.id.with("h"), resp.hovered(), 0.12);
    let accent = ui.ctx().data(|d| d.get_temp::<Color32>(egui::Id::unique("accent"))).unwrap_or(theme::RED);
    let fade = if ui.is_enabled() { 1.0 } else { 0.4 };
    let off = theme::glass(0.16 + 0.06 * h);
    ui.painter().rect_filled(rect, 13.0, theme::with_alpha(theme::lerp_color(off, accent, t), fade));
    let knob = pos2(egui::lerp(rect.left() + 13.0..=rect.right() - 13.0, t), rect.center().y);
    ui.painter().circle_filled(knob + vec2(0.0, 1.0), 10.0, theme::shade(0.25 * fade));
    ui.painter().circle_filled(knob, 10.0, theme::with_alpha(Color32::WHITE, fade));
    resp
}

/// Whole seconds on a track like the volume slider's, with the ends labelled and the
/// value beside it: click or drag to set (scrolling over it scrolls the page).
fn seconds_slider(ui: &mut Ui, v: &mut u32, range: std::ops::RangeInclusive<u32>, name: &str) -> egui::Response {
    let mut f = *v as f32;
    let resp = step_slider(ui, &mut f, (*range.start() as f32, *range.end() as f32, 1.0), None, |v| format!("{v} s"), name);
    *v = f as u32;
    resp
}

/// A slider over `lo..=hi` in steps of `step`, for a settings row: the value, then the
/// track between its ends. It fills from `from` (`lo` when none) to the knob.
fn step_slider(ui: &mut Ui, v: &mut f32, (lo, hi, step): (f32, f32, f32), from: Option<f32>, text: impl Fn(f32) -> String, name: &str) -> egui::Response {
    let label = |ui: &mut Ui, text: String, w: Option<f32>, font: egui::FontId, color: Color32| {
        let g = widgets::galley(ui, &text, font, color, 60.0);
        let (rect, _) = ui.allocate_exact_size(vec2(w.unwrap_or(g.size().x), 26.0), Sense::hover());
        ui.painter().galley(pos2(rect.right() - g.size().x, rect.center().y - g.size().y / 2.0), g, color);
    };
    label(ui, text(*v), Some(40.0), theme::semibold(13.0), theme::text());
    ui.add_space(6.0);
    label(ui, text(hi), None, theme::regular(12.0), theme::text_dim());
    let resp = slider_track(ui, v, (lo, hi, step), from, 170.0, name);
    label(ui, text(lo), None, theme::regular(12.0), theme::text_dim());
    resp
}

/// Just the track and knob of a step slider, `width` wide.
pub(super) fn slider_track(ui: &mut Ui, v: &mut f32, (lo, hi, step): (f32, f32, f32), from: Option<f32>, width: f32, name: &str) -> egui::Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(width, 26.0), Sense::click_and_drag());
    let track = Rect::from_center_size(rect.center(), vec2(rect.width() - 16.0, 4.0));
    let old = *v;
    if let Some(p) = resp.interact_pointer_pos()
        && (resp.dragged() || resp.clicked())
    {
        let t = ((p.x - track.left()) / track.width()).clamp(0.0, 1.0);
        *v = (((lo + t * (hi - lo)) / step).round() * step).clamp(lo, hi);
    }
    if *v != old {
        resp.mark_changed();
    }
    let value = *v;
    resp.widget_info(|| egui::WidgetInfo::slider(ui.is_enabled(), value as f64, name));
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let h = theme::anim_bool(ui.ctx(), resp.id.with("h"), resp.hovered() || resp.dragged(), 0.12);
    let accent = ui.ctx().data(|d| d.get_temp::<Color32>(egui::Id::unique("accent"))).unwrap_or(theme::RED);
    let x = |v: f32| egui::lerp(track.left()..=track.right(), (v - lo) / (hi - lo));
    let knob = pos2(x(value), track.center().y);
    let from = x(from.unwrap_or(lo));
    ui.painter().rect_filled(track, 2.0, theme::glass(0.15));
    ui.painter().rect_filled(Rect::from_min_max(pos2(from.min(knob.x), track.min.y), pos2(from.max(knob.x), track.max.y)), 2.0, accent);
    ui.painter().circle_filled(knob + vec2(0.0, 1.0), 7.0 + h, theme::shade(0.25));
    ui.painter().circle_filled(knob, 7.0 + h, Color32::WHITE);
    resp
}

/// Draw a key cap whose right edge is at `right`; returns its left edge.
pub(super) fn keycap(ui: &Ui, right: egui::Pos2, key: &str) -> f32 {
    let g = widgets::galley(ui, key, theme::semibold(12.5), theme::text(), 200.0);
    let size = vec2((g.size().x + 18.0).max(30.0), 26.0);
    let rect = Rect::from_min_size(pos2(right.x - size.x, right.y - size.y / 2.0), size);
    ui.painter().rect_filled(rect, 7.0, theme::glass(0.08));
    ui.painter().rect_stroke(rect, 7.0, egui::Stroke::new(1.0, theme::glass(0.12)), egui::StrokeKind::Inside);
    ui.painter().rect_filled(Rect::from_min_max(pos2(rect.left() + 2.0, rect.bottom() - 2.0), pos2(rect.right() - 2.0, rect.bottom())), 2.0, theme::shade(0.25));
    ui.painter().galley(rect.center() - g.size() / 2.0, g, theme::text());
    rect.left()
}

