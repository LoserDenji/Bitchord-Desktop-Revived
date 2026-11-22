//! The hero that opens Home — Mochy's TV hero stood on a desktop: a wide stage
//! washed in the card's own art, the cover square at the left instead of cropped
//! across the screen, its text block beside it, and a grid of the songs around
//! it (the rest of the discoveries, the other playlists, the pool) filling the
//! space a phone keeps empty. Auto-advances every five and a half seconds (a
//! gacha page holds double — there's a roll to watch); ‹ › circles page it for
//! a mouse, the way the TV turns its cards from the couch.

use egui::{Align2, Color32, Rect, Sense, Ui, Vec2, pos2, vec2};

use super::widgets::{self, icon_at};
use super::{Action, App};
use crate::model::{Card, Target, Track};
use crate::theme::{self, icon};

/// How long a card holds before the next wipes in (Mochy's `HeroAdvanceMillis`).
const ADVANCE: f64 = 5.5;
/// A gacha page holds double: the roll shouldn't be walked away from mid-spin.
const GACHA_ADVANCE: f64 = 11.0;
/// A roll spins this long: three and a half to nine seconds.
const SPIN_MIN: f64 = 3.5;
const SPIN_MAX: f64 = 9.0;
/// The reel's covers cycle this fast while it spins.
const REEL: f64 = 8.0;
/// A gacha needs this many songs to roll from.
const GACHA_MIN_POOL: usize = 4;

/// A gacha roll under way: what it's spinning through, what it landed on, and when.
pub(super) struct Gacha {
    pub until: f64,
    pub won: Track,
    pool: Vec<Track>,
}

/// One card of the pager.
#[derive(Clone)]
enum CardKind {
    /// What's playing or was last played.
    Continue(Track),
    /// A discovery; the label line says what it grew from.
    Discover { seed: Track, found: Track },
    /// One of your playlists.
    Playlist(Card),
    /// A keep-listening song, filling the pager out.
    Keep(Track),
}

/// One row of the song grid beside the cover: what it shows and what a click does.
struct GridRow {
    thumbs: Vec<crate::model::Thumb>,
    title: String,
    sub: String,
    action: Option<GridAction>,
}

/// What a grid row does when clicked (`Action` itself isn't a `Clone`).
enum GridAction {
    /// The song's radio.
    Radio(Track),
    /// A collection played from the top.
    Play(Target),
}

/// The grid beside a card: the songs that share its world, so the wide stage
/// carries something to browse and not one cover on an empty field.
fn grid_items(app: &App, kind: Option<&CardKind>, max: usize) -> (String, Vec<GridRow>) {
    let mut rows: Vec<GridRow> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let label;
    match kind {
        // the keep-listening songs, then the rest of the recent plays, the one on
        // the cover skipped
        Some(CardKind::Continue(t)) => {
            label = "Pick back up";
            seen.push(t.id.clone());
            let keep: Vec<Track> = app.home_rows.keep.iter().filter_map(|c| match &c.target {
                Target::Song(t) => Some(t.clone()),
                _ => None,
            }).collect();
            let recent = app.listening.recently_played(8);
            push_tracks(&mut rows, &mut seen, keep.into_iter().chain(recent), max);
        }
        // the other discoveries, each grown from its own seed; when the day's few
        // discoveries run out, the recommendation rows carry the grid on — their
        // songs are the unheard picks off the same taste, grown the same way
        Some(CardKind::Discover { seed, found }) => {
            label = "More discoveries";
            seen.push(found.id.clone());
            for (s, f) in app.discover.iter() {
                if rows.len() >= max {
                    break;
                }
                if seen.contains(&f.id) {
                    continue;
                }
                seen.push(f.id.clone());
                rows.push(GridRow { thumbs: f.thumbs.clone(), title: f.title.clone(), sub: format!("Grown from {}", s.title), action: Some(GridAction::Radio(f.clone())) });
            }
            for r in app.reco.rows.iter() {
                if rows.len() >= max {
                    break;
                }
                for t in r.tracks.iter() {
                    if rows.len() >= max {
                        break;
                    }
                    if seen.contains(&t.id) {
                        continue;
                    }
                    seen.push(t.id.clone());
                    rows.push(GridRow { thumbs: t.thumbs.clone(), title: t.title.clone(), sub: format!("Grown from {}", r.title), action: Some(GridAction::Radio(t.clone())) });
                }
            }
            let _ = seed;
        }
        // the playlists beside the one on the cover
        Some(CardKind::Playlist(c)) => {
            label = "Your playlists";
            let playlists: Vec<Card> = app
                .library
                .as_ref()
                .map(|l| l.playlists.clone())
                .or_else(|| app.sidebar_cache.clone())
                .unwrap_or_default();
            for p in playlists.iter().filter(|p| p.target != c.target) {
                if rows.len() >= max {
                    break;
                }
                rows.push(GridRow { thumbs: p.thumbs.clone(), title: p.title.clone(), sub: p.subtitle.clone(), action: Some(GridAction::Play(p.target.clone())) });
            }
        }
        Some(CardKind::Keep(t)) => {
            label = "Pick back up";
            seen.push(t.id.clone());
            let keep: Vec<Track> = app.home_rows.keep.iter().filter_map(|c| match &c.target {
                Target::Song(t) => Some(t.clone()),
                _ => None,
            }).collect();
            push_tracks(&mut rows, &mut seen, keep.into_iter(), max);
        }
        // what a roll spins through
        None => {
            label = "In the machine";
            for t in app.gacha_pool.iter().take(max) {
                rows.push(GridRow { thumbs: t.thumbs.clone(), title: t.title.clone(), sub: t.artist_line(), action: Some(GridAction::Radio(t.clone())) });
            }
        }
    }
    (label.to_owned(), rows)
}

/// Tracks into grid rows, skipping anything in `seen`.
fn push_tracks(rows: &mut Vec<GridRow>, seen: &mut Vec<String>, tracks: impl Iterator<Item = Track>, max: usize) {
    for t in tracks {
        if rows.len() >= max {
            break;
        }
        if seen.contains(&t.id) {
            continue;
        }
        seen.push(t.id.clone());
        rows.push(GridRow { thumbs: t.thumbs.clone(), title: t.title.clone(), sub: t.artist_line(), action: Some(GridAction::Radio(t)) });
    }
}

/// The hero's cards, as the page opens with them.
fn cards(app: &App) -> Vec<CardKind> {
    let mut cards = Vec::new();
    // what's on now, else the last thing played
    let last = app.current().cloned().or_else(|| app.listening.recently_played(1).into_iter().next());
    if let Some(t) = last {
        cards.push(CardKind::Continue(t));
    }
    for (seed, found) in app.discover.iter().take(4) {
        cards.push(CardKind::Discover { seed: seed.clone(), found: found.clone() });
    }
    // your playlists, the library's (the sidebar's stand in until it arrives)
    let playlists: Vec<Card> = app
        .library
        .as_ref()
        .map(|l| l.playlists.clone())
        .or_else(|| app.sidebar_cache.clone())
        .unwrap_or_default();
    for c in playlists.iter().take(2) {
        cards.push(CardKind::Playlist(c.clone()));
    }
    // filling out the pager with what to pick back up
    let keep: Vec<Track> = app
        .home_rows
        .keep
        .iter()
        .filter_map(|c| match &c.target {
            Target::Song(t) => Some(t.clone()),
            _ => None,
        })
        .collect();
    for t in keep {
        if cards.len() >= 6 {
            break;
        }
        if !matches!(cards.first(), Some(CardKind::Continue(t2)) if t2.id == t.id) {
            cards.push(CardKind::Keep(t));
        }
    }
    cards
}

/// What a card says, and what its Play pill does.
fn card_words(app: &App, kind: Option<&CardKind>) -> (String, String, String, Option<Action>) {
    match kind {
        Some(CardKind::Continue(t)) => (
            (if app.current().is_some() { "Now playing" } else { "Continue" }).to_owned(),
            t.title.clone(),
            t.artist_line(),
            Some(Action::Radio(t.clone())),
        ),
        Some(CardKind::Discover { seed, found }) => (
            format!("Because you like {}", seed.title),
            found.title.clone(),
            found.artist_line(),
            Some(Action::Radio(found.clone())),
        ),
        Some(CardKind::Playlist(c)) => ("From your playlists".to_owned(), c.title.clone(), c.subtitle.clone(), Some(Action::PlayCollection(c.target.clone(), false))),
        Some(CardKind::Keep(t)) => ("Pick back up".to_owned(), t.title.clone(), t.artist_line(), Some(Action::Radio(t.clone()))),
        None => ("Gacha here!".to_owned(), "Roll for a random play".to_owned(), "A radio grows out of whatever lands".to_owned(), None),
    }
}

/// A card's art.
fn card_thumbs(kind: Option<&CardKind>) -> &[crate::model::Thumb] {
    match kind {
        Some(CardKind::Continue(t)) => &t.thumbs,
        Some(CardKind::Discover { found, .. }) => &found.thumbs,
        Some(CardKind::Playlist(c)) => &c.thumbs,
        Some(CardKind::Keep(t)) => &t.thumbs,
        None => &[],
    }
}

pub(super) fn draw(app: &mut App, ui: &mut Ui, now: f64) {
    let cards = cards(app);
    let gacha_slot = app.gacha_pool.len() >= GACHA_MIN_POOL;
    let pages = cards.len() + gacha_slot as usize;
    if pages == 0 {
        return;
    }

    // A settled spin fires and clears, before anything else draws.
    if let Some(g) = app.gacha.take() {
        if now >= g.until {
            let won = g.won;
            app.toast(format!("Rolled \"{}\"", won.title));
            app.actions.push(Action::Radio(won));
        } else {
            app.gacha = Some(g);
        }
    }
    let spinning = app.gacha.is_some();
    if spinning {
        // the reel needs its frames
        ui.ctx().request_repaint();
    }

    // Half the screen stood wide: enough for a cover, its words and a grid,
    // without the stage pushing the rows below off the first view.
    let screen_h = ui.ctx().input(|i| i.raw.screen_rect.map_or(800.0, |r| r.height()));
    let height = (screen_h * 0.52).clamp(300.0, 430.0);
    let width = ui.available_width();
    let (band, band_resp) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    let band_resp = band_resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    ui.add_space(10.0);
    if !ui.is_rect_visible(band) {
        return;
    }
    let page = app.hero_page % pages;
    let on_gacha = gacha_slot && page == 1;
    // page 0 is the first card; with a gacha, page 1 is its card, the rest follow
    let kind = if on_gacha {
        None
    } else {
        let idx = page - if gacha_slot && page > 1 { 1 } else { 0 };
        cards.get(idx).cloned()
    };

    let advance = if on_gacha { GACHA_ADVANCE } else { ADVANCE };
    // Not while the pointer is on the stage (a card being read) or a roll is spinning.
    if pages > 1 && !spinning && !band_resp.contains_pointer() && now - app.hero_at > advance {
        app.hero_page = (page + 1) % pages;
        app.hero_at = now;
        ui.ctx().animate_value_with_time(slide_id(ui), width, 0.0);
    }
    ui.ctx().request_repaint_after(std::time::Duration::from_secs_f32((0.5f64).max(advance - (now - app.hero_at)) as f32));

    // --- the stage's wash: the card's art, melted into the page at its edges
    let reel = if on_gacha {
        app.gacha
            .as_ref()
            .and_then(|g| g.pool.get(((now * REEL) as usize) % g.pool.len()))
            .or_else(|| app.gacha_pool.first())
            .cloned()
    } else {
        None
    };
    let wash_thumbs: Vec<crate::model::Thumb> = match (&kind, &reel) {
        (_, Some(t)) => t.thumbs.clone(),
        (Some(k), None) => card_thumbs(Some(k)).to_vec(),
        _ => Vec::new(),
    };
    widgets::paint_backdrop(ui, wash_id(ui), band, theme::card_radius().min(24.0), &wash_thumbs, Color32::WHITE);
    // a quiet dim so the wash reads as a stage, and the gradients melt its left
    // and bottom into the page the way the TV's backdrop sinks away
    ui.painter().rect_filled(band, theme::card_radius().min(24.0), Color32::from_black_alpha(if on_gacha { 115 } else { 55 }));
    melt(ui, band, theme::bg());

    // --- the square cover, left, with the wipe sliding the next art in over it
    let cover = (height - 88.0).clamp(180.0, 290.0);
    let crect = Rect::from_center_size(pos2(band.left() + 40.0 + cover / 2.0, band.center().y), Vec2::splat(cover));
    match &kind {
        Some(CardKind::Continue(t)) => cover_slide(ui, crect, &t.thumbs),
        Some(CardKind::Discover { found, .. }) => cover_slide(ui, crect, &found.thumbs),
        Some(CardKind::Playlist(c)) => cover_slide(ui, crect, &c.thumbs),
        Some(CardKind::Keep(t)) => cover_slide(ui, crect, &t.thumbs),
        None => {
            if let Some(t) = &reel {
                cover_slide(ui, crect, &t.thumbs);
            }
            ui.painter().rect_filled(crect, 18.0, theme::shade(0.45));
        }
    }

    // --- the words, beside the cover
    let (label, title, sub, action) = card_words(app, kind.as_ref());
    let tx = crect.right() + 34.0;
    let tw = ((band.right() - 130.0 - tx) * 0.42).clamp(240.0, 420.0);
    // A soft shade behind the words: the art keeps its edges, the text keeps its
    // contrast — the reference's card puts its scrim under the block, not on the
    // whole stage. Clear of the cover, which draws over whatever reaches it.
    let scrim = Rect::from_min_max(pos2(crect.right() + 6.0, band.top()), pos2(tx + tw + 80.0, band.bottom()));
    super::shell::hgradient(ui.painter(), scrim, theme::shade(0.45), Color32::TRANSPARENT);
    let title_g = widgets::galley_wrapped(ui, &title, theme::bold(29.0), theme::text(), tw, 2);
    let sub_g = widgets::galley(ui, &sub, theme::regular(13.5), theme::text_dim(), tw);
    let (title_h, sub_h) = (title_g.size().y, sub_g.size().y);
    let block_h = 22.0 + title_h + (if !sub.is_empty() { sub_h + 8.0 } else { 0.0 }) + 16.0 + 34.0;
    let mut ty = band.center().y - block_h / 2.0;
    // the label's mini cover, round, before its text (a discovery names its seed)
    let mut label_x = tx;
    if let Some(CardKind::Discover { seed, .. }) = kind.as_ref() {
        if let Some(url) = crate::model::pick(&seed.thumbs, 120) {
            let r = Rect::from_center_size(pos2(tx + 9.0, ty + 9.0), Vec2::splat(18.0));
            widgets::paint_remote(ui, url, r, 9.0, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        label_x = tx + 26.0;
    }
    ui.painter().text(pos2(label_x, ty + 9.0), Align2::LEFT_CENTER, &label, theme::semibold(12.5), theme::with_alpha(Color32::WHITE, 0.72));
    ty += 22.0;
    ui.painter().galley(pos2(tx, ty), title_g, theme::text());
    ty += title_h + 8.0;
    if !sub.is_empty() {
        ui.painter().galley(pos2(tx, ty), sub_g, theme::text_dim());
        ty += sub_h;
    }
    ty += 16.0;

    // the pill: Play for a song (Pause on the one playing), Roll on the gacha card
    let playing = app.current().is_some() && matches!(kind.as_ref(), Some(CardKind::Continue(_)));
    let can_roll = app.gacha_pool.len() >= GACHA_MIN_POOL;
    let mut rolled = false;
    {
        let acts = &mut app.actions;
        match kind.as_ref() {
            None if spinning => {
                ui.painter().text(pos2(tx, ty + 17.0), Align2::LEFT_CENTER, "Rolling…", theme::semibold(15.0), theme::text());
            }
            None => {
                rolled = pill_at(ui, Rect::from_min_size(pos2(tx, ty), vec2(96.0, 34.0)), "Roll", Some(icon::SPARKLES), false).clicked();
            }
            Some(CardKind::Playlist(c)) => {
                if pill_at(ui, Rect::from_min_size(pos2(tx, ty), vec2(96.0, 34.0)), "Play", Some(icon::PLAY), true).clicked() {
                    acts.push(Action::PlayCollection(c.target.clone(), false));
                }
            }
            Some(_) => {
                let (name, glyph) = if playing { ("Pause", icon::PAUSE) } else { ("Play", icon::PLAY) };
                if pill_at(ui, Rect::from_min_size(pos2(tx, ty), vec2(96.0, 34.0)), name, Some(glyph), true).clicked() {
                    if playing {
                        acts.push(Action::TogglePlay);
                    } else if let Some(a) = action {
                        acts.push(a);
                    }
                }
            }
        }
    }
    if rolled && can_roll {
        // pick from the pool with chance: the reel spins what it spins through, and
        // the roll takes 3.5–9 s before it lands
        let mut rng = crate::recommend::Rng::new();
        let won = app.gacha_pool[(rng.unit() * app.gacha_pool.len() as f64) as usize].clone();
        let spin = SPIN_MIN + rng.unit() * (SPIN_MAX - SPIN_MIN);
        app.gacha = Some(Gacha { until: now + spin, won, pool: app.gacha_pool.clone() });
        app.hero_at = now;
    }

    // --- the grid: the songs around this card, each row its own play
    let gx = tx + tw + 30.0;
    let gw = band.right() - 128.0 - gx;
    if gw >= 280.0 {
        let (grid_label, rows) = grid_items(app, kind.as_ref(), ((height - 96.0) / 56.0).floor().clamp(2.0, 5.0) as usize);
        let gy0 = band.top() + 26.0;
        ui.painter().text(pos2(gx, gy0 + 7.0), Align2::LEFT_CENTER, &grid_label.to_uppercase(), theme::semibold(10.5), theme::with_alpha(Color32::WHITE, 0.55));
        let mut y = gy0 + 22.0;
        let acts = &mut app.actions;
        for r in &rows {
            let rect = Rect::from_min_size(pos2(gx, y), vec2(gw, 50.0));
            let resp = ui.interact(rect, egui::Id::unique(("hero-grid", gx as u32, y as u32)), Sense::click());
            let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
            let h = theme::anim_bool(ui.ctx(), resp.id.with("h"), resp.hovered(), 0.15);
            if h > 0.0 {
                ui.painter().rect_filled(rect, 12.0, theme::glass(0.09 * h));
            }
            let img = Rect::from_center_size(pos2(rect.left() + 26.0, rect.center().y), Vec2::splat(40.0));
            widgets::paint_cover(ui, img, &r.thumbs, theme::thumb_radius().min(8.0), 1.0);
            if h > 0.5 {
                ui.painter().rect_filled(img, theme::thumb_radius().min(8.0), theme::shade(0.5 * h));
                widgets::play_triangle(ui, img.center(), 7.0, theme::with_alpha(Color32::WHITE, h));
            }
            let text_w = gw - 62.0;
            let g = widgets::galley(ui, &r.title, theme::semibold(13.5), theme::text(), text_w);
            let th = g.size().y;
            ui.painter().galley(pos2(rect.left() + 54.0, rect.center().y - th - 1.0), g, theme::text());
            let s = widgets::galley(ui, &r.sub, theme::regular(11.5), theme::text_dim(), text_w);
            ui.painter().galley(pos2(rect.left() + 54.0, rect.center().y + 1.0), s, theme::text_dim());
            if resp.clicked() && !spinning {
                match &r.action {
                    Some(GridAction::Radio(t)) => acts.push(Action::Radio(t.clone())),
                    Some(GridAction::Play(target)) => acts.push(Action::PlayCollection(target.clone(), false)),
                    None => {}
                }
            }
            y += 56.0;
        }
    }

    // clicking the stage plays (a playlist plays all of it); the pill, the grid and
    // the arrows take their own clicks before it
    if band_resp.clicked() && !spinning {
        let acts = &mut app.actions;
        match kind.as_ref() {
            Some(CardKind::Playlist(c)) => acts.push(Action::PlayCollection(c.target.clone(), false)),
            _ => {
                let track = match kind.as_ref() {
                    Some(CardKind::Continue(t)) | Some(CardKind::Keep(t)) => Some(t.clone()),
                    Some(CardKind::Discover { found, .. }) => Some(found.clone()),
                    _ => None,
                };
                if let Some(t) = track {
                    acts.push(Action::Radio(t));
                }
            }
        }
    }

    // the pager's ‹ › and its dots, only with something to page through
    if pages > 1 {
        let (l, r) = pager_arrows(ui, band);
        if l {
            app.hero_page = (page + pages - 1) % pages;
            app.hero_at = now;
            ui.ctx().animate_value_with_time(slide_id(ui), -cover, 0.0);
        }
        if r {
            app.hero_page = (page + 1) % pages;
            app.hero_at = now;
            ui.ctx().animate_value_with_time(slide_id(ui), cover, 0.0);
        }
        // Mochy's TV dots: the lit one stretched, so more cards a click away read at a glance
        let dot_y = band.bottom() - 16.0;
        let mut x = band.right() - 26.0;
        for p in (0..pages).rev() {
            let on = p == page;
            if on {
                let r = Rect::from_center_size(pos2(x - 8.0, dot_y), vec2(18.0, 5.0));
                ui.painter().rect_filled(r, 2.5, theme::with_alpha(Color32::WHITE, 0.95));
                x -= 22.0;
            } else {
                ui.painter().circle_filled(pos2(x - 2.5, dot_y), 2.5, theme::with_alpha(Color32::WHITE, 0.4));
                x -= 11.0;
            }
        }
    }
}

/// The wipe: the incoming cover slides in over the last one's dark underlay.
fn cover_slide(ui: &Ui, rect: Rect, thumbs: &[crate::model::Thumb]) {
    let slide = ui.ctx().animate_value_with_time(slide_id(ui), 0.0, 1.15 * theme::speed());
    // under it, the cover holds — the wipe never shows a bare background
    widgets::paint_cover(ui, rect, thumbs, 18.0, 1.0);
    if slide.abs() > 0.5 {
        let shifted = Rect::from_min_size(pos2(rect.left() + slide, rect.top()), rect.size());
        widgets::paint_cover(ui, shifted, thumbs, 18.0, 1.0);
        ui.ctx().request_repaint();
    }
}

fn slide_id(ui: &Ui) -> egui::Id {
    egui::Id::unique(("hero-slide", ui.scope_id()))
}

fn wash_id(ui: &Ui) -> egui::Id {
    egui::Id::unique(("hero-wash", ui.scope_id()))
}

/// The wash's edges melt into the page on every side: gradients of the page's own
/// colour, the TV backdrop's masks — the stage reads as a wash of art, not a
/// rectangle stood on it.
fn melt(ui: &Ui, band: Rect, bg: Color32) {
    let mesh_at = |painter: &egui::Painter, corners: [Color32; 4]| {
        let [tl, tr, bl, br] = corners;
        let mut mesh = egui::Mesh::default();
        mesh.colored_vertex(band.left_top(), tl);
        mesh.colored_vertex(band.right_top(), tr);
        mesh.colored_vertex(band.left_bottom(), bl);
        mesh.colored_vertex(band.right_bottom(), br);
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(1, 3, 2);
        painter.add(egui::Shape::mesh(mesh));
    };
    // left: the page's colour falls away toward the middle (the cover sits on it)
    let a = theme::with_alpha(bg, 0.88);
    mesh_at(ui.painter(), [a, Color32::TRANSPARENT, a, Color32::TRANSPARENT]);
    // right: the far flank gives way the same, so the grid's edge and the pager
    // arrows sit on the page, not on a cut
    let r = band.width() * 0.14;
    super::shell::hgradient(ui.painter(), Rect::from_min_max(pos2(band.right() - r, band.top()), band.right_bottom()), theme::with_alpha(bg, 0.0), theme::with_alpha(bg, 0.85));
    // top: the header above meets the stage, not a hard edge
    let t = band.height() * 0.3;
    super::shell::vgradient(ui.painter(), Rect::from_min_max(band.left_top(), pos2(band.right(), band.top() + t)), theme::with_alpha(bg, 0.85), theme::with_alpha(bg, 0.0));
    // bottom: the rows below meet the stage, not a hard edge
    let b = theme::with_alpha(bg, 0.82);
    mesh_at(ui.painter(), [Color32::TRANSPARENT, Color32::TRANSPARENT, b, b]);
}

/// A pill at a rect of the hero's own — `widgets::pill` lays itself out in a
/// panel's flow; the hero pins its own. Same look: white when primary.
fn pill_at(ui: &mut Ui, rect: Rect, label: &str, glyph: Option<char>, primary: bool) -> egui::Response {
    let resp = ui.interact(rect, egui::Id::unique(("hero-pill", label)), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let h = theme::anim_bool(ui.ctx(), resp.id.with("h"), resp.hovered(), 0.15);
    let (fill, fg) = if primary {
        if theme::light() { (theme::lerp_color(theme::text(), Color32::BLACK, h), Color32::WHITE) } else { (theme::lerp_color(Color32::from_gray(240), Color32::WHITE, h), Color32::from_gray(12)) }
    } else {
        (theme::glass(0.14 + 0.12 * h), theme::text())
    };
    let r = rect.shrink(if resp.is_pointer_button_down_on() { 1.0 } else { 0.0 });
    ui.painter().rect_filled(r, r.height() / 2.0, fill);
    let text = ui.painter().layout_no_wrap(label.to_owned(), theme::semibold(14.0), Color32::PLACEHOLDER);
    let icon_w = if glyph.is_some() { 24.0 } else { 0.0 };
    let mut x = r.left() + (r.width() - text.size().x - icon_w) / 2.0;
    if let Some(g) = glyph {
        icon_at(ui, pos2(x + 8.0, r.center().y), g, 15.0, fg);
        x += icon_w;
    }
    ui.painter().galley(pos2(x, r.center().y - text.size().y / 2.0), text, fg);
    resp
}

/// The pager's ‹ › circles on the stage's right edge, halfway up — a mouse's
/// way round what a phone swipes, big enough to find and lit enough to read.
fn pager_arrows(ui: &mut Ui, band: Rect) -> (bool, bool) {
    let cy = band.center().y;
    let mut clicked = (false, false);
    for (i, (glyph, name)) in [(icon::CHEVRON_RIGHT, "Next card"), (icon::CHEVRON_LEFT, "Previous card")].into_iter().enumerate() {
        let center = pos2(band.right() - 34.0 - i as f32 * 46.0, cy);
        let hit = Rect::from_center_size(center, Vec2::splat(38.0));
        let resp = ui.interact(hit, egui::Id::unique(("hero-arrow", i)), Sense::click());
        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(name);
        let h = theme::anim_bool(ui.ctx(), resp.id.with("h"), resp.hovered(), 0.15);
        // a filled glass circle the hover raises, brighter than the shelf arrows:
        // on art of any colour a bare glyph is a needle in a haystack
        ui.painter().circle_filled(center, 19.0 + 1.5 * h, theme::with_alpha(Color32::BLACK, 0.28 + 0.20 * h));
        if h > 0.0 {
            ui.painter().circle_filled(center, 19.0 + 1.5 * h, theme::glass(0.16 * h));
        }
        icon_at(ui, center, glyph, 20.0, theme::lerp_color(theme::with_alpha(Color32::WHITE, 0.75), Color32::WHITE, h));
        if resp.clicked() {
            clicked.1 = i == 0;
            clicked.0 = i == 1;
        }
    }
    clicked
}
