//! Page bodies. Every page fades and slides in when navigated to.

use std::collections::HashSet;
use std::sync::atomic::Ordering;

use egui::{Align2, Color32, Rect, RichText, Sense, Ui, Vec2, pos2, vec2};

use super::widgets::{self, RowOpts, card, chip, h_scroll, named, paint_cover, pill, shelf_header, track_row};
use super::{Action, App, LibTab, Page, ProviderVerdicts, Selection};
use crate::backend::compact;
use crate::model::{Card, ShelfKind, Target, Track};
use crate::theme::{self, icon};

const CARD: f32 = 180.0;

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

/// One tile of an Apple feed row: a share of the content on wide windows, the site's
/// own narrow width below its breakpoint.
fn feed_tile(ui: &Ui, square: bool) -> f32 {
    let width = ui.available_width();
    let min = if square { 144.0 } else { 270.0 };
    if width < 1024.0 {
        return min;
    }
    let cols = feed_columns(width, square);
    ((width - 20.0 * (cols - 1) as f32) / cols as f32).max(min)
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
}

impl Rows {
    pub(super) fn new(app: &App, now: f64) -> Self {
        Self {
            current: app.current_id().map(str::to_owned),
            playing: app.audio.status.playing.load(Ordering::Relaxed),
            accent: app.accent_color(),
            now,
        }
    }

    pub(super) fn opts(&self, liked: &HashSet<String>, t: &Track, number: Option<usize>, show_album: bool) -> RowOpts {
        RowOpts {
            number,
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
fn play_pill(ui: &mut Ui, acts: &mut Vec<Action>, target: &Target, tracks: &[Track]) {
    let active = widgets::active_state(ui, target);
    let (label, glyph) = if active == Some(true) { ("Pause", icon::PAUSE) } else { ("Play", icon::PLAY) };
    if pill(ui, label, Some(glyph), true).clicked() {
        if active.is_some() {
            acts.push(Action::TogglePlay);
        } else if !tracks.is_empty() {
            acts.push(Action::Play(tracks.to_vec(), 0));
        }
    }
}

fn card_row(ui: &mut Ui, id: &str, title: &str, cards: &[Card], acts: &mut Vec<Action>) {
    if cards.is_empty() {
        return;
    }
    let (l, r) = shelf_header(ui, title, None, None, true);
    let nudge = r as i32 - l as i32;
    let tile = feed_tile(ui, true);
    h_scroll(ui, id, nudge, cards.len(), tile, 20.0, |ui| {
        ui.spacing_mut().item_spacing.x = 20.0;
        for c in cards {
            card(ui, c, tile, false, acts);
        }
    });
}

/// A card shelf with a Show all on its header (`all`: there is more where this came
/// from, and `open` asks for it). The arrows stay: the shelf still scrolls sideways.
fn card_row_all(ui: &mut Ui, id: &str, title: &str, cards: &[Card], all: bool, acts: &mut Vec<Action>, open: impl FnOnce() -> Action) {
    if cards.is_empty() {
        return;
    }
    let (l, r, show_all) = widgets::shelf_header_ext(ui, title, None, None, true, all);
    if show_all {
        acts.push(open());
    }
    let nudge = r as i32 - l as i32;
    let tile = feed_tile(ui, true);
    h_scroll(ui, id, nudge, cards.len(), tile, 20.0, |ui| {
        ui.spacing_mut().item_spacing.x = 20.0;
        for c in cards {
            card(ui, c, tile, false, acts);
        }
    });
}

/// Songs laid out in columns of `rows`, scrolling sideways (Quick picks). Clicking a
/// song starts its radio. Returns whether the arrows reached the end.
#[allow(clippy::too_many_arguments)]
fn song_grid(ui: &mut Ui, rows_ctx: &Rows, liked: &HashSet<String>, acts: &mut Vec<Action>, id: &str, title: &str, strap: Option<&str>, tracks: &[Track], rows: usize) -> bool {
    let (l, r) = shelf_header(ui, title, strap, None, true);
    let nudge = r as i32 - l as i32;
    let col_w = feed_tile(ui, false);
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
        acts.push(Action::Radio(tracks[i].clone()));
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
fn list(ui: &mut Ui, rows: &Rows, liked: &HashSet<String>, tracks: &[Track], show_album: bool, number: impl Fn(usize, &Track) -> Option<usize>, cx: ListCtx, acts: &mut Vec<Action>) {
    let ListCtx { sel, page, own } = cx;
    let selecting = sel.on(page);
    let mods = ui.input(|i| i.modifiers);
    widgets::visible_rows(ui, tracks.len(), |ui, i| {
        let t = &tracks[i];
        let o = RowOpts {
            selected: selecting.then(|| sel.rows.contains(&i)),
            remove: own.then_some(i),
            ..rows.opts(liked, t, number(i, t), show_album)
        };
        if track_row(ui, t, &o, acts).clicked() {
            if selecting || mods.command || mods.shift {
                sel.pick(page, i, mods.shift);
            } else {
                acts.push(Action::Play(tracks.to_vec(), i));
            }
        }
    });
}

/// A list page's "Select" button: turns select mode on and off.
fn select_pill(ui: &mut Ui, sel: &mut Selection, page: &Page) {
    let on = sel.on(page);
    if pill(ui, if on { "Done" } else { "Select" }, Some(icon::LIST_CHECKS), false).clicked() {
        if on { sel.clear() } else { sel.start(page) }
    }
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
    let picked: Vec<Track> = app.selection.rows.iter().filter_map(|&i| tracks.get(i).cloned()).collect();
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
    page_title(ui, "Home");
    // A refresh keeps showing the current feed; the skeleton is only for an empty one.
    if app.home.as_ref().is_none_or(|h| app.home_loading && h.shelves.is_empty()) {
        placeholder(app, ui, now, 3, CARD);
        return;
    }
    let rows = Rows::new(app, now);
    let Some(h) = &app.home else { return };
    if h.shelves.is_empty() {
        ui.add_space(40.0);
        ui.label(RichText::new("Nothing here yet").color(theme::text_dim()));
    }
    let mut more_picks = None;
    for (i, s) in h.shelves.iter().enumerate() {
        let id = format!("home-{i}-{}", s.title);
        match &s.kind {
            ShelfKind::Cards(cards) => {
                let (strap, thumb) = if s.title == "Listen again" { (None, None) } else { (s.strapline.as_deref(), s.strap_thumb.as_deref()) };
                let (l, r) = shelf_header(ui, &s.title, strap, thumb, true);
                let nudge = r as i32 - l as i32;
                let tile = feed_tile(ui, true);
                let acts = &mut app.actions;
                let liked = &app.liked;
                h_scroll(ui, &id, nudge, cards.len(), tile, 20.0, |ui| {
                    ui.spacing_mut().item_spacing.x = 20.0;
                    for c in cards {
                        let is_liked = matches!(&c.target, Target::Song(t) if liked.contains(&t.id));
                        card(ui, c, tile, is_liked, acts);
                    }
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
    let Some(r) = &app.search else { return };
    let acts = &mut app.actions;
    let liked = &app.liked;
    if r.tracks.is_empty() && r.videos.is_empty() && r.artists.is_empty() && r.albums.is_empty() && r.playlists.is_empty() {
        ui.add_space(18.0);
        ui.label(RichText::new("No results").font(theme::bold(20.0)).color(theme::text()));
        return;
    }
    ui.add_space(14.0);
    // Top result: the first artist when the query names one, otherwise the first song.
    ui.horizontal_top(|ui| {
        if let Some(top) = r.artists.first().filter(|a| a.title.eq_ignore_ascii_case(q.trim())) {
            top_result(ui, top, false, acts);
        } else if let Some(t) = r.tracks.first() {
            let c = Card {
                title: t.title.clone(),
                subtitle: format!("Song • {}", t.artist_line()),
                thumbs: t.thumbs.clone(),
                target: Target::Song(t.clone()),
                round: false,
                links: t.artists.clone(),
            };
            top_result(ui, &c, pick, acts);
        }
        ui.add_space(24.0);
        ui.vertical(|ui| {
            ui.label(RichText::new("Songs").font(theme::bold(24.0)));
            ui.add_space(8.0);
            for (i, t) in r.tracks.iter().take(4).enumerate() {
                let o = RowOpts { pick, ..rows.opts(liked, t, None, false) };
                if track_row(ui, t, &o, acts).clicked() {
                    acts.push(if pick { Action::TransferPick(t.clone()) } else { Action::Play(r.tracks.clone(), i) });
                }
            }
        });
    });
    card_row(ui, "s-artists", "Artists", &r.artists, acts);
    card_row(ui, "s-albums", "Albums", &r.albums, acts);
    if !r.videos.is_empty() && !app.hide_search_videos {
        ui.add_space(26.0);
        ui.label(RichText::new("Videos").font(theme::bold(24.0)));
        ui.add_space(10.0);
        for (i, t) in r.videos.iter().take(6).enumerate() {
            let o = RowOpts { pick, ..rows.opts(liked, t, None, false) };
            if track_row(ui, t, &o, acts).clicked() {
                acts.push(if pick { Action::TransferPick(t.clone()) } else { Action::Play(r.videos.clone(), i) });
            }
        }
    }
    if r.tracks.len() > 4 {
        ui.add_space(26.0);
        ui.label(RichText::new("More songs").font(theme::bold(24.0)));
        ui.add_space(10.0);
        for (i, t) in r.tracks.iter().enumerate().skip(4) {
            let o = RowOpts { pick, ..rows.opts(liked, t, None, true) };
            if track_row(ui, t, &o, acts).clicked() {
                acts.push(if pick { Action::TransferPick(t.clone()) } else { Action::Play(r.tracks.clone(), i) });
            }
        }
    }
    card_row(ui, "s-pl", "Playlists", &r.playlists, acts);
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
fn top_result(ui: &mut Ui, c: &Card, pick: bool, acts: &mut Vec<Action>) {    ui.vertical(|ui| {
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
    });
}

// ------------------------------------------------------------------ album / playlist

fn collection(app: &mut App, ui: &mut Ui, id: &str, now: f64) {
    if app.opened(id).is_none() {
        placeholder(app, ui, now, 1, 232.0);
        return;
    }
    let rows = Rows::new(app, now);
    // The header's animated cover: the release's first song stands in for the album —
    // Apple files the release's clip with its songs, and the canvas finds it there.
    let px = (232.0 * ui.ctx().pixels_per_point()).round().max(256.0) as u32;
    let proxy = app.opened(id).and_then(|o| o.c.tracks.first().cloned());
    let sprite = proxy.and_then(|t| app.animated_cover(ui.ctx(), &t, px));
    let Some(o) = app.lists.get(id) else { return };
    let (c, more) = (&o.c, app.receiving(id));
    let acts = &mut app.actions;
    let tracks = &c.tracks;
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
        ui.add_space(26.0);
        ui.vertical(|ui| {
            ui.set_max_width(ui.available_width().min(760.0));
            ui.add_space(24.0);
            ui.label(RichText::new(if is_album { "ALBUM" } else { "PLAYLIST" }).font(theme::semibold(12.0)).color(theme::text_dim()));
            ui.label(RichText::new(&c.title).font(theme::bold(40.0)).color(theme::text()));
            ui.add_space(2.0);
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::hover());
            widgets::linked_text(ui, r.min, &c.byline(), &c.artists, theme::regular(15.0), theme::text_dim(), r.width(), acts);
            let total: u32 = tracks.iter().filter_map(|t| t.duration).sum();
            if total > 0 {
                ui.label(
                    RichText::new(format!("{} songs • {} min", tracks.len(), total / 60)).font(theme::regular(13.5)).color(theme::text_faint()),
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
                play_pill(ui, acts, &target, tracks);
                if circle_action(ui, CircleGlyph::Char(icon::SHUFFLE), "Shuffle", false).clicked() && !tracks.is_empty() {
                    let mut t = tracks.clone();
                    super::shuffle(&mut t);
                    acts.push(Action::Play(t, 0));
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
    let number = |i: usize, t: &Track| is_album.then(|| t.track_nr.map_or(i + 1, |n| n as usize));
    let own = app.logged_in && c.owned && !is_album;
    list(ui, &rows, &app.liked, shown, !is_album, number, ListCtx { sel: &mut app.selection, page: &app.page, own }, acts);
    if more {
        widgets::skeleton_rows(ui, now, 3);
    }
}

// ------------------------------------------------------------------ artist

fn artist(app: &mut App, ui: &mut Ui, id: &str, now: f64) {
    if app.artist.as_ref().is_none_or(|a| a.id != id) {
        placeholder(app, ui, now, 2, CARD);
        return;
    }
    let rows = Rows::new(app, now);
    let accent = app.accent_color();
    let name = app.artist.as_ref().map(|a| a.name.clone()).unwrap_or_default();
    // The listener's signature service stands in for the name where it has one.
    let signature = app.signature_texture(ui.ctx(), &name);
    let Some(a) = &app.artist else { return };
    let acts = &mut app.actions;
    let top = &a.top;
    // The hero, full-bleed at the header aspect the artist pages use (about 1.93:1),
    // fading into the page at the bottom. YouTube's banner draws it.
    let w = ui.available_width();
    let screen_h = ui.ctx().input(|i| i.raw.screen_rect.map_or(0.0, |r| r.height()));
    let hero_h = (w / 1.93).clamp(240.0, (screen_h * 0.7).max(380.0));
    let (hero, _) = ui.allocate_exact_size(vec2(w, hero_h), Sense::hover());
    if let Some(t) = a.banner.iter().max_by_key(|t| t.width) {
        let img_ratio = if t.height > 0 { t.width as f32 / t.height as f32 } else { 16.0 / 9.0 };
        // Cover-fit: crop the image's top band to the hero's aspect.
        let vis_h = (img_ratio / (w / hero_h)).min(1.0);
        let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, vis_h));
        ui.painter().rect_filled(hero, 14.0, theme::glass(0.07));
        widgets::paint_remote(ui, &t.url, hero, 14.0, uv, Color32::WHITE);
    } else {
        ui.painter().rect_filled(hero, 14.0, theme::with_alpha(accent, 0.25));
    }
    super::shell::vgradient(ui.painter(), Rect::from_min_max(pos2(hero.left(), hero.center().y - 40.0), hero.max), Color32::TRANSPARENT, theme::shade(0.85));
    let base = hero.left_bottom() + vec2(28.0, -30.0);
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
    ui.add_space(18.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        play_pill(ui, acts, &Target::Artist(id.to_owned()), top);
        if pill(ui, "Shuffle", Some(icon::SHUFFLE), false).clicked() && !top.is_empty() {
            let mut t = top.clone();
            super::shuffle(&mut t);
            acts.push(Action::Play(t, 0));
        }
        if let Some(first) = top.first()
            && pill(ui, "Radio", Some(icon::RADIO), false).clicked() {
                acts.push(Action::Radio(first.clone()));
            }
    });
    if !top.is_empty() {
        ui.add_space(26.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Top songs").font(theme::bold(24.0)).color(theme::text()));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // With a "Top songs" playlist behind the shelf, Show all opens it; without
                // one, the shelf page stands in and lists everything the page holds.
                if pill(ui, "Show all", None, false).clicked() {
                    match a.top_playlist.clone() {
                        Some(pl) => acts.push(Action::Open(Target::Playlist(pl))),
                        None => acts.push(Action::Go(Page::ArtistShelf { artist: id.to_owned(), title: super::SONGS_SHELF.to_owned() })),
                    }
                }
            });
        });
        ui.add_space(12.0);
        for (i, t) in top.iter().enumerate().take(5) {
            let o = rows.opts(&app.liked, t, None, true);
            if track_row(ui, t, &o, acts).clicked() {
                acts.push(Action::Play(top.clone(), i));
            }
        }
    }
    // Albums and singles keep their own shelves now, each with its Show all into the
    // artist's full discography for that kind of release.
    card_row_all(ui, "a-albums", "Albums", &a.albums, a.albums_more.is_some(), acts, || Action::Go(Page::ArtistShelf { artist: id.to_owned(), title: "Albums".to_owned() }));
    card_row_all(ui, "a-singles", "Singles", &a.singles, a.singles_more.is_some(), acts, || Action::Go(Page::ArtistShelf { artist: id.to_owned(), title: super::SINGLES_SHELF.to_owned() }));
    card_row(ui, "a-pl", "Featured on", &a.playlists, acts);
    card_row(ui, "a-sim", "Fans might also like", &a.similar, acts);
    if let Some(d) = a.description.as_deref().filter(|d| !d.trim().is_empty()) {
        shelf_header(ui, "About", None, None, false);
        let full = ui.available_width().min(820.0);
        let g = widgets::galley_wrapped(ui, d.trim(), theme::regular(14.5), theme::text_dim(), full, 8);
        ui.label(g);
    }
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
        let clicked = list(ui, &rows, liked, tracks, true, plain, cx, acts);
        app.selection = sel;
        let _ = clicked;
    } else if !s.cards.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(20.0, 22.0);
            for c in &s.cards {
                card(ui, c, CARD, false, acts);
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
    if !app.logged_in {
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
        for (t, label) in [(LibTab::Playlists, "Playlists"), (LibTab::Songs, "Liked songs"), (LibTab::Albums, "Albums"), (LibTab::History, "History"), (LibTab::Uploads, "Uploads")] {
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
    let grid = |ui: &mut Ui, cards: &[Card], acts: &mut Vec<Action>| {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(20.0, 22.0);
            for c in cards {
                card(ui, c, CARD, false, acts);
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
            list(ui, &rows, &app.liked, h, true, plain, ListCtx { sel: &mut app.selection, page: &app.page, own: false }, &mut app.actions);
        }
        LibTab::Uploads => {
            ui.horizontal(|ui| {
                if pill(ui, "Upload music", Some(icon::UPLOAD), true).clicked() {
                    app.pick_uploads(ui.ctx());
                }
                if app.uploads.as_ref().is_some_and(|u| !u.is_empty()) && pill(ui, "Shuffle", Some(icon::SHUFFLE), false).clicked() {
                    let mut t = app.uploads.clone().unwrap_or_default();
                    super::shuffle(&mut t);
                    app.actions.push(Action::Play(t, 0));
                }
            });
            ui.add_space(6.0);
            ui.label(RichText::new("Or drop audio files anywhere on the window. MP3, M4A, FLAC, OGG and WMA.").font(theme::regular(13.0)).color(theme::text_dim()));
            ui.add_space(12.0);
            if app.uploads.is_none() {
                placeholder(app, ui, now, 1, 48.0);
                return;
            }
            let Some(up) = &app.uploads else { return };
            if up.is_empty() {
                ui.add_space(20.0);
                ui.label(RichText::new("No uploads yet").color(theme::text_dim()));
            }
            list(ui, &rows, &app.liked, up, true, plain, ListCtx { sel: &mut app.selection, page: &app.page, own: false }, &mut app.actions);
        }
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
                        app.actions.push(Action::Play(liked.clone(), 0));
                    }
                    if pill(ui, "Shuffle", Some(icon::SHUFFLE), false).clicked() {
                        let mut t = liked.clone();
                        super::shuffle(&mut t);
                        app.actions.push(Action::Play(t, 0));
                    }
                    select_pill(ui, &mut app.selection, &app.page);
                });
                ui.add_space(10.0);
            }
            list(ui, &rows, &app.liked, liked, true, plain, ListCtx { sel: &mut app.selection, page: &app.page, own: false }, &mut app.actions);
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
            match tab {
                LibTab::Playlists => {
                    if pill(ui, "New playlist", None, false).clicked() {
                        app.actions.push(Action::NewPlaylist(Vec::new()));
                    }
                    ui.add_space(14.0);
                    grid(ui, &lib.playlists, &mut app.actions)
                }
                LibTab::Albums => grid(ui, &lib.albums, &mut app.actions),
                LibTab::Songs | LibTab::History | LibTab::Uploads => unreachable!(),
            }
        }
    }
}

// ------------------------------------------------------------------ downloads

fn downloads(app: &mut App, ui: &mut Ui, now: f64) {
    page_title(ui, "Downloads");
    let songs = crate::downloads::list();
    ui.add_space(4.0);
    ui.label(
        RichText::new(format!("{} songs \u{2022} {}", songs.len(), crate::downloads::folder().display()))
            .font(theme::regular(13.0))
            .color(theme::text_dim()),
    );
    ui.add_space(12.0);
    if songs.is_empty() {
        ui.label(RichText::new("No downloads yet").font(theme::semibold(16.0)).color(theme::text()));
        ui.add_space(4.0);
        ui.label(
            RichText::new("Right-click a song and choose Download to keep it on this computer, playable offline.")
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
            app.actions.push(Action::Play(songs.clone(), 0));
        }
        if pill(ui, "Shuffle", Some(icon::SHUFFLE), false).clicked() {
            let mut t = songs.clone();
            super::shuffle(&mut t);
            app.actions.push(Action::Play(t, 0));
        }
        if pill(ui, "Open folder", Some(icon::FOLDER_OPEN), false).clicked() {
            crate::downloads::open_folder();
        }
    });
    ui.add_space(12.0);
    let rows = Rows::new(app, now);
    list(ui, &rows, &app.liked, &songs, true, |_, _| None, ListCtx { sel: &mut app.selection, page: &app.page, own: false }, &mut app.actions);
}

// ------------------------------------------------------------------ settings

// ── Settings ─────────────────────────────────────────────────────────────

/// The categories Settings splits into: most used first, which is how they're listed.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum SettingsTab {
    #[default]
    Account,
    Playback,
    Sources,
    Lyrics,
    Appearance,
    General,
    Shortcuts,
}

impl SettingsTab {
    const ALL: [Self; 7] = [Self::Account, Self::Playback, Self::Sources, Self::Lyrics, Self::Appearance, Self::General, Self::Shortcuts];

    fn label(self) -> &'static str {
        match self {
            Self::Account => "Account",
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

fn filtering() -> bool {
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
        }
        SettingsTab::Playback => settings_playback(app, ui),
        SettingsTab::Sources => {
            settings_sources(app, ui);
            settings_saavn(app, ui);
        }
        SettingsTab::Lyrics => {
            settings_lyrics(app, ui);
            settings_artists(app, ui);
        }
        SettingsTab::Appearance => settings_appearance(app, ui),
        SettingsTab::General => {
            settings_window(app, ui);
            settings_downloads(ui);
            settings_section(ui, "Network", None, None, |ui| network(app, ui));
            if filter_show(ui, "Import Spotify transfer Liked Songs playlists account") {
                super::transfer::settings_section(app, ui);
            }
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
    });
    if busy && filter_show(ui, "finish signing in in the window that opened") {
        ui.add_space(6.0);
        ui.label(RichText::new("Finish signing in in the window that opened.").font(theme::regular(12.5)).color(theme::text_dim()));
    }
    if sign_in {
        app.start_sign_in(ui.ctx());
    }
    if sign_out {
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
        app.backend.request(crate::backend::Req::SwitchChannel { page_id: c.page_id, epoch: *epoch });
    }
}

/// Settings › Playback, and below it the equalizer in its own section.
fn settings_playback(app: &mut App, ui: &mut Ui) {
    settings_section(ui, "Playback", None, None, |ui| {
        setting_row(ui, "Autoplay", Some("Keep playing similar songs when the queue ends."), |ui| {
            toggle(ui, &mut app.autoplay, "Autoplay");
        });
        divider(ui);
        setting_row(ui, "Volume normalization", Some("Turn loud songs down to the same level, like YouTube Music does."), |ui| {
            if toggle(ui, &mut app.normalize, "Volume normalization").changed() {
                app.audio.set_normalize(app.normalize);
            }
        });
        divider(ui);
        setting_row(ui, "Logarithmic volume", Some("Space the volume slider out the way ears hear loudness, so quiet levels aren't squeezed into its first few pixels."), |ui| {
            if toggle(ui, &mut app.log_volume, "Logarithmic volume").changed() {
                app.audio.set_log_volume(app.log_volume);
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
        setting_row(ui, "Show music videos", Some("Music videos play as moving video, with their own sound and no lyrics. The Song and Video switch in Now playing swaps between them either way."), |ui| {
            let mut on = app.show_video;
            if toggle(ui, &mut on, "Show music videos").changed() {
                app.actions.push(Action::ToggleVideo);
            }
        });
        divider(ui);
        setting_row(ui, "Ambient light", Some("Now playing glows with the video's colours around it."), |ui| {
            toggle(ui, &mut app.ambient_light, "Ambient light");
        });
        divider(ui);
        setting_row(ui, "Hide videos in search", Some("Only songs, albums, artists and playlists in search results."), |ui| {
            toggle(ui, &mut app.hide_search_videos, "Hide videos in search");
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
    });
    settings_equalizer(app, ui);
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
            if resp.lost_focus() {
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
fn settings_sources(app: &mut App, ui: &mut Ui) {
    settings_section(ui, "Source priority", None, Some("Other catalogs that can stand in for YouTube's stream when they have the song: the first source in the priority list that holds the song plays its own copy, and when none does, YouTube plays as usual."), |ui| {
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
        }
    });
    settings_sources_lists(app, ui);
    settings_provider_add(app, ui);
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
    settings_section(ui, "Monochrome servers", None, Some("Monochrome-format servers you've added, tried before Monochrome's own streamer — they were added for a reason. A site's link is read for the server it plays from. New servers come from \"Add a custom provider\" below, which asks both formats at once."), |ui| {
        mono_servers_editor(app, ui);
    });
}

/// JioSaavn's stream quality, its Test, and the addons' Dolby Atmos switch.
fn settings_saavn(app: &mut App, ui: &mut Ui) {
    settings_section(ui, "Streams", None, None, |ui| {
        setting_row(ui, "JioSaavn stream", Some("How much data a JioSaavn copy uses. 320 is its best; songs that don't have it play 160."), |ui| {
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
            });
        });
        divider(ui);
        // JioSaavn's own check is its own unit: searching "test" finds it alone.
        if filter_show(ui, "JioSaavn test connection") {
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let slot = app.saavn_test.clone();
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
                        let (good, msg) = crate::providers::probe_saavn(&http).await;
                        *slot.lock().unwrap() = Some(Some((good, msg)));
                    });
                }
            });
            test_line(ui, &app.saavn_test.lock().unwrap().clone());
            ui.add_space(10.0);
        }
        divider(ui);
        setting_row(ui, "Prefer Dolby Atmos", Some("Ask the addons for a Dolby Atmos copy when they offer one. Falls back to the best stereo copy when the stream's codec can't be decoded."), |ui| {
            if toggle(ui, &mut app.prefer_atmos, "Prefer Dolby Atmos").changed() {
                app.backend.set_prefer_atmos(app.prefer_atmos);
            }
        });
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

/// Settings › Downloads.
fn settings_downloads(ui: &mut Ui) {
    settings_section(ui, "Downloads", None, Some("Download songs from their menu. They're saved at the best quality your account gets, tagged with their cover art, and play from the file, even offline."), |ui| {
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
        // Intensive glow: the sung words' drop-shadow lit much harder than the look
        // asks, with the listener's own strength on top.
        setting_row(ui, "Intensive glow", Some("Light the word being sung with a far stronger glow than the theme's own, the way am-lyrics does it. Strength sets how far it goes."), |ui| {
            toggle(ui, &mut app.lyric_prefs.glow_boost, "Intensive glow");
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
            setting_row_shown(ui, "Lyrics sources", Some(desc), |_| {});
            let n = app.lyric_prefs.order.len();
            let mut swap: Option<(usize, usize)> = None;
            for i in 0..n {
                let (p, on) = app.lyric_prefs.order[i];
                let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::hover());
                let mut row = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::left_to_right(egui::Align::Center)));
                let mut enabled = on;
                if toggle(&mut row, &mut enabled, p.name()).changed() {
                    app.lyric_prefs.order[i].1 = enabled;
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

/// Settings › Artist pages: the listener's own signature service.
fn settings_artists(app: &mut App, ui: &mut Ui) {
    settings_section(ui, "Artist pages", None, None, |ui| {
        // The row and its link field are one unit in a search.
        let desc = "A link to your own signature API. Artists get their handwritten signature, with “Signed by …” above it, standing in for their name in the page header; without a signature, their name is set in type.";
        if filter_show(ui, &format!("Signature service {desc} link")) {
            setting_row_shown(ui, "Signature service", Some(desc), |_| {});
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
        setting_row(ui, "Animated background", Some("The album art slowly drifts behind everything."), |ui| {
            toggle(ui, &mut app.anim_bg, "Animated background");
        });
        divider(ui);
        setting_row(ui, "Pulse with the music", Some("The background swells with the beat."), |ui| {
            toggle(ui, &mut app.reactive_bg, "Pulse with the music");
        });
        divider(ui);
        setting_row(ui, "Animated covers", Some("Where Apple Music has a clip for a song or its album, the cover moves in Now playing and on the album page, looping while the music plays and holding still when it's paused. Finding them sends each song's title and artist to Apple Music, remembered on disk so each song asks once."), |ui| {
            toggle(ui, &mut app.animated_covers, "Animated covers");
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
    });
}

/// Settings › Window.
fn settings_window(app: &mut App, ui: &mut Ui) {
    settings_section(ui, "Window", None, None, |ui| {
        setting_row(ui, "Close to tray", Some("Clicking × hides the window and keeps the music playing. The tray icon is always up: left-click brings the window back, and its menu has the transport buttons, Launch at login and Quit. If the icon never shows up, launching BitChord again brings the window back; --no-tray starts a session without an icon, where × quits for real."), |ui| {
            toggle(ui, &mut app.close_to_tray, "Close to tray");
        });
        divider(ui);
        setting_row(ui, "Launch at login", Some("Start BitChord when the computer does. The OS keeps this: the Run key on Windows, an autostart entry on Linux, a launch agent on macOS — and the tray menu's own Launch at login item flips the same switch."), |ui| {
            if toggle(ui, &mut app.launch_at_login, "Launch at login").changed() {
                crate::autostart::set(app.launch_at_login);
            }
        });
        divider(ui);
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

/// Settings › Connections.
fn settings_connections(app: &mut App, ui: &mut Ui) {
    let mut discord_enabled = app.discord_enabled;
    let linked = app.discord_link_token.is_some();
    let (mut connect, mut disconnect) = (false, false);
    settings_section(ui, "Connections", None, None, |ui| {
        let detail = match (&app.discord_link_user, linked) {
            (Some(u), _) => format!("Showing what you play on {}'s Discord profile.", u.name),
            (None, true) => "Discord is linked; presence shows on the linked account.".to_owned(),
            (None, false) => "Show what you're playing on your Discord profile. Link an account for a card with buttons and cover art on your own profile.".to_owned(),
        };
        setting_row(ui, "Discord", Some(&detail), |ui| {
            toggle(ui, &mut discord_enabled, "Discord");
        });
        divider(ui);
        let status = app.discord_link.as_ref().map(|s| s.status()).unwrap_or(("off", None));
        let link_detail = match (&status.1, app.discord_link_busy) {
            (Some(p), _) => p.as_str(),
            (None, true) => "Waiting for the Discord page…",
            (None, false) if linked => match status.0 {
                "ready" => "Linked account's presence is live.",
                "connecting" => "Bringing the linked account's gateway up…",
                "error" => "The linked account's gateway is down.",
                _ => "Linked. Presence starts with the switch above.",
            },
            (None, false) => "Open discord.com and sign in; the app takes it from there.",
        };
        setting_row(ui, "Link account", Some(link_detail), |ui| {
            if linked {
                disconnect = pill(ui, "Disconnect", None, false).clicked();
            } else if app.discord_link_busy {
                ui.spinner();
            } else {
                connect = pill(ui, "Connect", None, false).clicked();
            }
        });
        if linked {
            divider(ui);
            setting_row(ui, "Lead with the song", Some("Name the song, not the artist, in member lists."), |ui| {
                toggle(ui, &mut app.discord_options.use_details, "song-first");
            });
            divider(ui);
            text_option(ui, &mut app.discord_options.activity_name, "Listening to :", "BitChord");
            divider(ui);
            setting_row(ui, "Buttons on the card", Some("Two taps on the profile card, fixed: one opens the song on YouTube Music, one opens this project's page. The labels can't be changed, only switched off."), |ui| {
                toggle(ui, &mut app.discord_options.button1_visible, crate::discord_link::DEFAULT_BUTTON_1);
                ui.add_space(8.0);
                toggle(ui, &mut app.discord_options.button2_visible, crate::discord_link::DEFAULT_BUTTON_2);
            });
        }
        divider(ui);
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
    if connect {
        app.link_discord();
    }
    if disconnect {
        app.unlink_discord();
    }
    if discord_enabled != app.discord_enabled {
        app.set_discord_enabled(discord_enabled);
    }
}

/// A one-line option the listener types over (the card's labels).
fn text_option(ui: &mut Ui, value: &mut String, label: &str, placeholder: &str) {
    setting_row(ui, label, Some(&format!("Empty: \"{placeholder}\".")), |ui| {
        let width = ui.available_width() - 140.0;
        ui.add(
            egui::TextEdit::singleline(value)
                .hint_text(egui::RichText::new(placeholder).color(theme::text_faint()))
                .font(theme::regular(13.0))
                .desired_width(width.max(120.0)),
        );
    });
}

/// Settings › Network: the proxy everything goes through, a connection test, and a
/// restart when the setting changed.
fn network(app: &mut App, ui: &mut Ui) {
    use crate::proxy::{self, Setting};
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
            match proxy::system() {
                Some(p) => {
                    let os = if cfg!(target_os = "macos") { "macOS" } else { "Windows" };
                    setting_row(ui, "System proxy", Some(&format!("{os} is set to use {}.", proxy::redact(&p))), |_| {});
                }
                None if cfg!(windows) => {
                    setting_row(ui, "System proxy", Some("Windows isn't set to use a proxy, so BitChord connects directly."), |_| {});
                }
                None => {
                    setting_row(ui, "System proxy", Some("BitChord doesn't read a system proxy on this platform, so it connects directly."), |_| {});
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

/// Paint `v` into `rect`, from the ICO entry that fits its size in physical pixels.
pub(super) fn paint_app_icon(ui: &Ui, v: &crate::appicon::Variant, rect: Rect) {
    let px = (rect.width() * ui.ctx().pixels_per_point()).ceil() as u32;
    let png = v.png(px);
    egui::Image::from_bytes(format!("bytes://app-icon/{}-{}.png", v.id, png.len()), png).paint_at(ui, rect);
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

/// A dropdown's closed state: the chosen value and a chevron on a glass pill.
fn dropdown_button(ui: &mut Ui, value: &str, width: f32, name: &str) -> egui::Response {
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
fn option_row(ui: &mut Ui, label: &str, selected: bool) -> egui::Response {
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
    setting_row_shown(ui, title, desc, control);
}

/// The row itself, without the filter check: for a block that's gated as one unit — a row
/// and the widgets that belong to it show together or not at all.
fn setting_row_shown(ui: &mut Ui, title: &str, desc: Option<&str>, control: impl FnOnce(&mut Ui)) {
    let w = ui.available_width();
    let text_w = (w - 230.0).max(200.0);
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
    let (lo, hi) = (*range.start(), *range.end());
    let label = |ui: &mut Ui, text: String, w: f32, font: egui::FontId, color: Color32| {
        let (rect, _) = ui.allocate_exact_size(vec2(w, 26.0), Sense::hover());
        let g = widgets::galley(ui, &text, font, color, w);
        ui.painter().galley(pos2(rect.right() - g.size().x, rect.center().y - g.size().y / 2.0), g, color);
    };
    label(ui, format!("{v} s"), 34.0, theme::semibold(13.0), theme::text());
    ui.add_space(6.0);
    label(ui, format!("{hi} s"), 26.0, theme::regular(12.0), theme::text_dim());
    let (rect, mut resp) = ui.allocate_exact_size(vec2(170.0, 26.0), Sense::click_and_drag());
    label(ui, format!("{lo} s"), 22.0, theme::regular(12.0), theme::text_dim());
    let track = Rect::from_center_size(rect.center(), vec2(rect.width() - 16.0, 4.0));
    let old = *v;
    if let Some(p) = resp.interact_pointer_pos()
        && (resp.dragged() || resp.clicked())
    {
        let t = ((p.x - track.left()) / track.width()).clamp(0.0, 1.0);
        *v = (lo as f32 + t * (hi - lo) as f32).round() as u32;
    }
    if *v != old {
        resp.mark_changed();
    }
    let value = *v;
    resp.widget_info(|| egui::WidgetInfo::slider(ui.is_enabled(), value as f64, name));
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let h = theme::anim_bool(ui.ctx(), resp.id.with("h"), resp.hovered() || resp.dragged(), 0.12);
    let accent = ui.ctx().data(|d| d.get_temp::<Color32>(egui::Id::unique("accent"))).unwrap_or(theme::RED);
    let t = (value - lo) as f32 / (hi - lo) as f32;
    let knob = pos2(egui::lerp(track.left()..=track.right(), t), track.center().y);
    ui.painter().rect_filled(track, 2.0, theme::glass(0.15));
    ui.painter().rect_filled(Rect::from_min_max(track.min, pos2(knob.x, track.max.y)), 2.0, accent);
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

