//! Settings › Discord: account linking, Rich Presence customization and its preview.

use egui::{Align2, Color32, Id, Layout, Rect, RichText, Sense, Ui, UiBuilder, Vec2, pos2, vec2};

use super::pages::{divider, dropdown_button, filtering, option_row, section_label, setting_row, setting_row_wide, settings_card, settings_section, toggle};
use super::{App, widgets};
use crate::discord::{self, Card, Kind, Page, Prefs, Presence, Show, Status};
use crate::discord_artwork::{self, Mode, Picture};
use crate::theme::{self, icon};

/// Dropdowns and the name box share a width, so the column lines up.
const CONTROL_W: f32 = 210.0;
/// A dropdown with its link switch.
const LINKED_W: f32 = CONTROL_W + 8.0 + 34.0;
/// The profile card's width, with the member list beside it.
const CARD_W: f32 = 340.0;

pub(super) fn settings(app: &mut App, ui: &mut Ui) {
    connection(app, ui);
    let mut prefs = app.discord_prefs.clone();
    let mut enabled = app.discord_enabled;
    // Off, the rest folds away; a search shows its rows either way.
    let shown = if filtering() { 1.0 } else { theme::anim_bool_soft(ui.ctx(), "discord-settings-shown", enabled, 0.3) };
    // Where `section_label` puts the heading, for Reset at its right end.
    let heading = Rect::from_min_size(ui.cursor().min + vec2(0.0, 30.0), vec2(ui.available_width(), 20.0));
    settings_section(ui, "Discord", None, None, |ui| {
        setting_row(ui, "Rich Presence", Some("Show the song you're playing on your Discord profile. Link the account above to show it on your own profile, without the Discord app running."), |ui| {
            toggle(ui, &mut enabled, "Rich Presence");
        });
        reveal(ui, "discord-settings-top", shown, |ui| {
            divider(ui);
            setting_row(ui, "Show when paused", Some("Keep the card visible while paused. Paused takes the place of the artist on the second line, and playback time is hidden."), |ui| {
                toggle(ui, &mut prefs.when_paused, "Show when paused");
            });
            divider(ui);
            setting_row(ui, "Hide details", Some("Say only that music is playing. No title, artist, album, artwork, time or buttons leave your computer."), |ui| {
                toggle(ui, &mut prefs.hide_details, "Hide details");
            });
        });
    });
    if !filtering() && shown > 0.0 && prefs != Prefs::default() && !app.discord_artwork.importing() {
        ui.scope(|ui| {
            ui.multiply_opacity(shown);
            if reset_link(ui, heading) {
                prefs = Prefs::default();
            }
        });
    }
    reveal(ui, "discord-settings-rest", shown, |ui| rest(app, ui, &mut prefs));
    if enabled != app.discord_enabled {
        app.set_discord_enabled(enabled);
    }
    // `sync_discord` sends what changed.
    app.discord_prefs = prefs;
}

fn connection(app: &mut App, ui: &mut Ui) {
    let linked = app.discord_link_token.is_some();
    let (mut connect, mut disconnect) = (false, false);
    settings_section(ui, "Discord connection", None, None, |ui| {
        // Without a linked account the plain IPC card shows through the Discord app.
        let detail = if linked {
            match &app.discord_link_user {
                Some(u) => format!("The Discord card shows on {}'s profile.", u.name),
                None => "Discord is linked. The card shows on the linked account.".to_owned(),
            }
        } else {
            "Link your account to show Rich Presence on your profile even when the Discord app is closed. The Discord desktop app can also show Rich Presence over IPC.".to_owned()
        };
        let status = app.discord_link.as_ref().map(|s| s.status()).unwrap_or(("off", None));
        let link_detail = match (&status.1, app.discord_link_busy) {
            (Some(p), _) => p.clone(),
            (None, true) => "Waiting for the Discord page…".to_owned(),
            (None, false) if linked => match status.0 {
                "ready" => detail,
                "connecting" => "Bringing the linked account's gateway up…".to_owned(),
                "error" => "The linked account's gateway is down.".to_owned(),
                _ => "Linked. The card starts with the Rich Presence switch below.".to_owned(),
            },
            (None, false) => detail,
        };
        setting_row(ui, "Link Discord", Some(&link_detail), |ui| {
            if linked {
                disconnect = widgets::pill(ui, "Disconnect", None, false).clicked();
            } else if app.discord_link_busy {
                ui.spinner();
            } else {
                connect = widgets::pill(ui, "Connect", None, false).clicked();
            }
        });
    });
    if connect {
        app.link_discord();
    }
    if disconnect {
        app.unlink_discord();
    }
}

/// Everything under the Rich Presence switch's section, which shows only while it's on.
fn rest(app: &App, ui: &mut Ui, prefs: &mut Prefs) {
    if !filtering() {
        preview(app, ui, prefs);
    }
    let on = !prefs.hide_details;
    settings_section(ui, "Discord status", None, None, |ui| {
        setting_row(ui, "Activity type", Some("The verb Discord shows above the activity."), |ui| {
            dropdown(ui, "Activity type", &mut prefs.kind, &[Kind::Listening, Kind::Playing], |k| k.label().into());
        });
        divider(ui);
        setting_row(ui, "App name", Some("Leave blank for BitChord PC. Supports the same variables as the activity text."), |ui| {
            text_field(ui, "Discord app name", &mut prefs.app_name, discord::APP_NAME, 256);
        });
        ui.add_enabled_ui(on, |ui| {
            divider(ui);
            setting_row(ui, "Status text", Some("Choose which field Discord leads with in the member list."), |ui| {
                let lines = prefs.lines;
                let label = |s: Status| match s {
                    Status::Name => "App name".to_owned(),
                    Status::Line1 => format!("Details · {}", lines[0].label()),
                    Status::Line2 => format!("State · {}", lines[1].label()),
                };
                dropdown(ui, "Status text", &mut prefs.status, &Status::ALL, label);
            });
        });
    });
    let variables =
        "Insert {song}, {artist}, {album}, {duration}, {quality}, {status}, {app} or {apple}. {apple} is the Apple logo on devices that support it.";
    settings_section(ui, "Discord card", None, Some(variables), |ui| {
        ui.add_enabled_ui(on, |ui| {
            for (i, label) in ["Details", "State"].into_iter().enumerate() {
                ui.push_id(label, |ui| {
                    if i > 0 {
                        divider(ui);
                    }
                    setting_row_wide(ui, label, None, LINKED_W, |ui| {
                        dropdown(ui, label, &mut prefs.lines[i], &Show::ALL, |s| s.label().into());
                        link_toggle(ui, &mut prefs.links[i], "Make this line clickable", !matches!(prefs.lines[i], Show::Nothing | Show::Quality));
                    });
                    if prefs.lines[i] == Show::Template {
                        divider(ui);
                        setting_row(ui, "Text template", Some("Leave blank to hide this line."), |ui| {
                            text_field(ui, "Activity text template", &mut prefs.templates[i], "{song} · {artist}", 256);
                        });
                        if prefs.links[i] {
                            divider(ui);
                            setting_row(ui, "Metadata link", None, |ui| {
                                let pages = [Page::Song, Page::Artist, Page::Album, Page::Jam, Page::Project];
                                dropdown(ui, "Metadata link", &mut prefs.template_links[i], &pages, |p| p.label().into());
                            });
                        }
                    }
                });
            }
            divider(ui);
            setting_row(ui, "Third line", Some("Shows below the artist. Leave blank to use the large image hover text on Discord."), |ui| {
                text_field(ui, "Third line template", &mut prefs.annotation, "{quality}", 256);
            });
        });
    });
    settings_section(ui, "Discord large image", None, None, |ui| {
        ui.add_enabled_ui(on, |ui| {
            setting_row(ui, "Show large image", Some("Its link opens the album, or the song when there is no album page."), |ui| {
                toggle(ui, &mut prefs.art, "Show large image");
                link_toggle(ui, &mut prefs.art_link, "Large image is clickable", prefs.art);
            });
            ui.add_enabled_ui(prefs.art, |ui| {
                divider(ui);
                image_mode(app, ui, &mut prefs.images[0], 0);
                divider(ui);
                setting_row(ui, "Large image hover text", Some("The preview shows this tooltip. Discord uses the third line for both while that line is filled in."), |ui| {
                    dropdown(ui, "Large image hover text", &mut prefs.lines[2], &Show::ALL, |s| s.label().into());
                });
                if prefs.lines[2] == Show::Template {
                    divider(ui);
                    setting_row(ui, "Large image text template", None, |ui| {
                        text_field(ui, "Large image text template", &mut prefs.images[0].hover, "{album} · {quality}", 256);
                    });
                }
            });
        });
    });
    settings_section(ui, "Discord small image", None, None, |ui| {
        ui.add_enabled_ui(on, |ui| {
            image_mode(app, ui, &mut prefs.images[1], 1);
            if prefs.images[1].mode != Mode::Off {
                divider(ui);
                setting_row(ui, "Small image hover text", Some("Discord displays the small image over the large one."), |ui| {
                    text_field(ui, "Small image hover text", &mut prefs.images[1].hover, discord::STATUS_QUALITY, 256);
                });
            }
        });
    });
    settings_section(ui, "Discord animated artwork", None, None, |ui| {
        ui.add_enabled_ui(on, |ui| {
            setting_row(ui, "Host local and animated artwork", Some("Enable to show local images and generated GIFs on Discord. Uguu or Litterbox makes each image public to anyone with its link until it expires."), |ui| {
                toggle(ui, &mut prefs.artwork.host, "Host local and animated artwork");
            });
            divider(ui);
            setting_row(ui, "Seconds per turn", None, |ui| {
                let steps: Vec<_> = (2..=20).collect();
                dropdown(ui, "Seconds per turn", &mut prefs.artwork.spin_seconds, &steps, |s| format!("{s} s"));
            });
            divider(ui);
            setting_row(ui, "Rotation frame rate", None, |ui| {
                dropdown(ui, "Rotation frame rate", &mut prefs.artwork.spin_fps, discord_artwork::SPIN_FPS, |f| format!("{f} fps"));
            });
            divider(ui);
            setting_row(ui, "Canvas frame rate", Some("Spotify Canvas uses the Spotify account connected for Playlist Transfer."), |ui| {
                dropdown(ui, "Canvas frame rate", &mut prefs.artwork.canvas_fps, discord_artwork::CANVAS_FPS, |f| format!("{f} fps"));
            });
            divider(ui);
            setting_row(ui, "Canvas size", Some("Large GIFs use fewer frames or a smaller size. When a clip is unavailable, the cover stays still."), |ui| {
                dropdown(ui, "Canvas size", &mut prefs.artwork.canvas_size, discord_artwork::CANVAS_SIZE, |s| format!("{s} px"));
            });
        });
    });
    settings_section(ui, "Discord timestamps", None, None, |ui| {
        ui.add_enabled_ui(on, |ui| {
            setting_row(
                ui,
                "Show playback time",
                Some("Discord shows a progress bar for Listening activities and elapsed time for Playing activities."),
                |ui| {
                    toggle(ui, &mut prefs.progress, "Show playback time");
                },
            );
            ui.add_enabled_ui(prefs.progress && prefs.kind == Kind::Listening, |ui| {
                divider(ui);
                setting_row(ui, "Elapsed time only", None, |ui| {
                    toggle(ui, &mut prefs.elapsed_only, "Elapsed time only");
                });
            });
        });
    });
    let note = "Only other Discord users see the buttons. A button is hidden when its destination is unavailable, such as Listen together outside a room.";
    settings_section(ui, "Discord buttons", None, Some(note), |ui| {
        ui.add_enabled_ui(on, |ui| {
            setting_row(ui, "Show buttons", None, |ui| {
                toggle(ui, &mut prefs.show_buttons, "Show buttons");
            });
            ui.add_enabled_ui(prefs.show_buttons, |ui| {
                for (i, label) in ["First button", "Second button"].into_iter().enumerate() {
                    ui.push_id(label, |ui| button_settings(ui, prefs, i, label));
                }
            });
        });
    });
    settings_section(ui, "Discord advanced", None, None, |ui| {
        ui.add_enabled_ui(on, |ui| {
            setting_row(ui, "Minimum update interval", Some("Discord's rate limit may delay updates further. Stopped playback clears the activity immediately."), |ui| {
                dropdown(ui, "Minimum update interval", &mut prefs.refresh_seconds, &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10], |s| format!("{s} s"));
            });
        });
    });
}

fn image_mode(app: &App, ui: &mut Ui, prefs: &mut discord_artwork::ImagePrefs, slot: usize) {
    ui.push_id(("discord image", slot), |ui| {
        setting_row(ui, "Artwork source", None, |ui| {
            let modes = if slot == 0 { &Mode::ALL[1..] } else { &Mode::ALL[..] };
            dropdown(ui, "Artwork source", &mut prefs.mode, modes, |m| m.label().into());
        });
        if prefs.mode == Mode::Custom {
            if prefs.file.is_none() {
                divider(ui);
                setting_row(ui, "Image URL", Some("Use a direct HTTP or HTTPS image link."), |ui| {
                    text_field(ui, "Image URL", &mut prefs.url, "https://…", 256);
                });
            }
            divider(ui);
            setting_row(ui, "Custom image", Some("Enable artwork hosting below to show a selected file on Discord."), |ui| {
                ui.add_enabled_ui(!app.discord_artwork.importing(), |ui| {
                    let image_set = prefs.file.is_some() || discord::web_url(&prefs.url, 256).is_some();
                    if image_set {
                        if widgets::pill(ui, "Remove", None, false).clicked() {
                            prefs.file = None;
                            prefs.url.clear();
                        }
                    } else if widgets::pill(ui, "Choose image", None, false).clicked() {
                        app.discord_artwork.import(&app.backend, slot);
                    }
                });
            });
        }
    });
}

fn button_settings(ui: &mut Ui, prefs: &mut Prefs, i: usize, name: &str) {
    divider(ui);
    setting_row(ui, name, None, |ui| {
        dropdown(ui, name, &mut prefs.buttons[i], &Page::ALL, |p| p.label().into());
    });
    if prefs.buttons[i] == Page::None {
        return;
    }
    if prefs.buttons[i] == Page::Custom {
        divider(ui);
        setting_row(ui, "Button URL", None, |ui| {
            text_field(ui, "Button URL", &mut prefs.button_urls[i], "https://…", 512);
        });
    }
    divider(ui);
    setting_row(ui, "Custom button label", None, |ui| {
        let mut custom = prefs.button_labels[i].is_some();
        if toggle(ui, &mut custom, "Custom button label").changed() {
            prefs.button_labels[i] = custom.then(|| "Listen to {song}".into());
        }
    });
    if let Some(template) = &mut prefs.button_labels[i] {
        divider(ui);
        setting_row(ui, "Button label template", Some("Maximum 32 characters after expansion. Leave blank to hide this button."), |ui| {
            text_field(ui, "Button label template", template, "Listen to {song}", 128);
        });
    }
}

/// `body`, faded and folded to `shown` of its height: 0 leaves it out, 1 draws it as is.
/// Its full height is remembered from when it was last drawn.
fn reveal(ui: &mut Ui, id: &str, shown: f32, body: impl FnOnce(&mut Ui)) {
    let id = Id::unique(id);
    if shown >= 1.0 {
        let top = ui.cursor().top();
        body(ui);
        let h = ui.min_rect().bottom() - top;
        ui.ctx().data_mut(|d| d.insert_temp(id, h));
        return;
    }
    if shown <= 0.0 {
        return;
    }
    let top = ui.cursor().min;
    let w = ui.available_width();
    let mut child = ui.new_child(UiBuilder::new().max_rect(Rect::from_min_size(top, vec2(w, f32::INFINITY))).layout(Layout::top_down(egui::Align::Min)));
    child.multiply_opacity(shown);
    // Never drawn whole yet, it's measured as it's drawn.
    let full = ui.ctx().data(|d| d.get_temp::<f32>(id));
    if let Some(full) = full {
        child.set_clip_rect(ui.clip_rect().intersect(Rect::from_min_size(top - vec2(8.0, 0.0), vec2(w + 16.0, full * shown))));
    }
    body(&mut child);
    let full = full.unwrap_or_else(|| child.min_rect().bottom() - top.y);
    ui.ctx().data_mut(|d| d.insert_temp(id, full));
    ui.advance_cursor_after_rect(Rect::from_min_size(top, vec2(w, full * shown)));
}

fn dropdown<T: Copy + PartialEq>(ui: &mut Ui, name: &str, value: &mut T, all: &[T], label: impl Fn(T) -> String) {
    let btn = dropdown_button(ui, &label(*value), CONTROL_W, name);
    egui::Popup::menu(&btn).show(|ui| {
        ui.set_width(CONTROL_W);
        ui.spacing_mut().item_spacing = vec2(0.0, 2.0);
        ui.add_space(4.0);
        for &v in all {
            if option_row(ui, &label(v), v == *value).clicked() {
                *value = v;
                ui.close();
            }
        }
        ui.add_space(4.0);
    });
}

/// The link switch beside a part of the card: on, that part is clickable.
fn link_toggle(ui: &mut Ui, on: &mut bool, name: &str, enabled: bool) {
    ui.add_space(8.0);
    let enabled = enabled && ui.is_enabled();
    let (rect, mut resp) = ui.allocate_exact_size(Vec2::splat(34.0), if enabled { Sense::click() } else { Sense::hover() });
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let value = *on;
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, enabled, value, name));
    let resp = if enabled { resp.on_hover_text(name).on_hover_cursor(egui::CursorIcon::PointingHand) } else { resp };
    let t = theme::anim_bool(ui.ctx(), resp.id.with("t"), value && enabled, 0.16);
    let h = theme::anim_bool(ui.ctx(), resp.id.with("h"), resp.hovered() && enabled, 0.12);
    // On, it's the accent, like the switches.
    let accent = ui.ctx().data(|d| d.get_temp::<Color32>(egui::Id::unique("accent"))).unwrap_or(theme::RED);
    let fade = if enabled { 1.0 } else { 0.45 };
    let off = theme::glass(0.06 + 0.06 * h);
    ui.painter().circle_filled(rect.center(), 17.0, theme::with_alpha(theme::lerp_color(off, accent, t), fade));
    let color = theme::lerp_color(theme::lerp_color(theme::text_faint(), theme::text(), h), theme::on_fill(accent), t);
    widgets::icon_at(ui, rect.center(), icon::LINK, 16.0, theme::with_alpha(color, fade));
}

fn text_field(ui: &mut Ui, id: &str, value: &mut String, hint: &str, limit: usize) {
    let (rect, _) = ui.allocate_exact_size(vec2(CONTROL_W, 36.0), Sense::hover());
    let bg = ui.painter().add(egui::Shape::Noop);
    let inner = rect.shrink2(vec2(14.0, 4.0));
    let edit = egui::TextEdit::singleline(value)
        .id_salt(id)
        .hint_text(RichText::new(hint).color(theme::text_faint()))
        .font(theme::regular(14.0))
        .frame(egui::Frame::NONE)
        .char_limit(limit)
        .align(Align2::LEFT_CENTER)
        .desired_width(inner.width());
    let resp = ui.put(inner, edit);
    let focus = theme::anim_bool(ui.ctx(), resp.id.with("focus"), resp.has_focus(), 0.15);
    ui.painter().set(bg, egui::Shape::rect_filled(rect, 10.0, theme::glass(0.08 + 0.03 * focus)));
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as i64
}

/// Shown with nothing playing.
fn example() -> Presence {
    let now = now_ms();
    Presence {
        video_id: String::new(),
        title: "Song title".into(),
        artist: "Artist".into(),
        artist_id: None,
        album: Some("Album".into()),
        album_id: None,
        art: Some(discord::ICON.into()),
        artist_art: None,
        quality: Some("Lossless - 1411 kbps - 44.1 kHz".into()),
        duration_ms: 205_000,
        position_ms: 79_000,
        clock: (0, 0, 0),
        start_ms: now - 79_000,
        end_ms: Some(now + 126_000),
        paused: false,
        jam: None,
    }
}

/// The profile card and member list as Discord would show them, under the Discord section.
fn preview(app: &App, ui: &mut Ui, prefs: &Prefs) {
    let (p, state) = shown(app);
    section_label(ui, "Preview", Some(state));
    // Paused, the card is drawn as it would be kept up.
    let artwork = app.discord_artwork.pictures(&app.backend, &p, prefs, app.playing_track(), true);
    let card = Card::new(&p, prefs, artwork);
    let notes = notes(&p, &card, prefs);
    let name = app.account_name.as_deref().unwrap_or("You");
    settings_card(ui, |ui| {
        ui.add_space(16.0);
        let w = ui.available_width();
        let wide = w >= 600.0;
        let card_w = if wide { CARD_W } else { w.min(380.0) };
        let side = |ui: &mut Ui| {
            caption(ui, "Member list");
            member_row(ui, &card, name, app.account_avatar.as_deref(), ui.available_width().min(380.0));
            for n in &notes {
                note(ui, n);
            }
        };
        if wide {
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(card_w);
                    caption(ui, "Profile card");
                    profile_card(ui, &card, card_w, !app.reduce_motion);
                });
                ui.add_space(24.0);
                ui.vertical(side);
            });
        } else {
            // Stacked, in a column down the middle.
            ui.horizontal_top(|ui| {
                ui.add_space(((w - card_w) / 2.0).max(0.0));
                ui.vertical(|ui| {
                    ui.set_width(card_w);
                    caption(ui, "Profile card");
                    profile_card(ui, &card, card_w, !app.reduce_motion);
                    ui.add_space(14.0);
                    side(ui);
                });
            });
        }
        ui.add_space(16.0);
    });
    if card.times.is_some() {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
    }
}

/// A dim "Reset" at the right end of the heading in `rect`, brighter on hover. True when clicked.
fn reset_link(ui: &mut Ui, rect: Rect) -> bool {
    let font = theme::semibold(13.0);
    let size = ui.painter().layout_no_wrap("Reset".to_owned(), font.clone(), theme::text_dim()).size();
    let area = Align2::RIGHT_CENTER.anchor_size(pos2(rect.right() - 2.0, rect.center().y), size);
    let link = ui.interact(area.expand(4.0), Id::unique("discord-reset"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    link.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Reset Discord settings to defaults"));
    let color = if link.hovered() { theme::text() } else { theme::text_dim() };
    ui.painter().text(area.right_center(), Align2::RIGHT_CENTER, "Reset", font, color);
    link.clicked()
}

/// The song the preview shows, and what it is.
fn shown(app: &App) -> (Presence, &'static str) {
    let p = app.discord_presence();
    let state = match &p {
        None => "Example",
        Some(p) if p.paused => "Paused",
        Some(_) => "Now playing",
    };
    (p.unwrap_or_else(example), state)
}

/// What the preview can't show by itself.
fn notes(p: &Presence, card: &Card, prefs: &Prefs) -> Vec<&'static str> {
    let mut notes = Vec::new();
    if p.paused && !prefs.when_paused {
        notes.push("Paused, so this isn't on your profile right now. Turn on Show when paused to keep it up.");
    }
    if !card.buttons.is_empty() {
        notes.push("Only others see the buttons.");
    }
    if card
        .art
        .iter()
        .chain(card.small_art.iter())
        .any(|picture| picture.rotation.is_some() || picture.preview.as_ref().is_some_and(|preview| preview.frames.len() > 1))
    {
        notes.push(if prefs.artwork.host {
            "Discord animates hosted GIFs when supported by the client and its motion settings."
        } else {
            "This animation stays in the preview until artwork hosting is enabled."
        });
    }
    notes
}

fn note(ui: &mut Ui, text: &str) {
    ui.add_space(10.0);
    ui.label(RichText::new(text).font(theme::regular(12.5)).color(theme::text_dim()));
}

fn caption(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).font(theme::semibold(12.5)).color(theme::text_dim()));
    ui.add_space(6.0);
}

const PAD: f32 = 12.0;
const ART: f32 = 76.0;
const LINE: f32 = 20.0;
const TIME: f32 = 22.0;
const BUTTON: f32 = 32.0;
const GAP: f32 = 8.0;

/// Discord's green: the online dot, and the music note before what's playing.
const GREEN: Color32 = Color32::from_rgb(0x23, 0xa5, 0x5a);

fn profile_card(ui: &mut Ui, c: &Card, width: f32, animate: bool) {
    let lines: Vec<_> = c.lines[..2].iter().flatten().collect();
    let text_h = (lines.len() + usize::from(c.annotation.is_some())) as f32 * LINE + if c.times.is_some() { TIME } else { 0.0 };
    let body_h = if c.art.is_some() { ART.max(text_h) } else { text_h };
    let buttons_h = c.buttons.len() as f32 * (BUTTON + GAP);
    let h = PAD + 18.0 + if body_h > 0.0 { 10.0 + body_h } else { 0.0 } + if buttons_h > 0.0 { 12.0 + buttons_h - GAP } else { 0.0 } + PAD;
    let (rect, _) = ui.allocate_exact_size(vec2(width, h), Sense::hover());
    ui.painter().rect_filled(rect, 10.0, theme::glass(0.07));
    let inner_w = width - 2.0 * PAD;
    let mut y = rect.top() + PAD;
    widgets::text_at(ui, pos2(rect.left() + PAD, y), &format!("{} {}", c.kind.label(), c.name()), theme::semibold(12.5), theme::text_dim(), inner_w - 28.0);
    // The card's ··· menu.
    for i in 0..3 {
        ui.painter().circle_filled(pos2(rect.right() - PAD - 2.0 - 6.0 * i as f32, y + 9.0), 1.6, theme::text_dim());
    }
    y += 18.0 + 10.0;
    let mut x = rect.left() + PAD;
    if let Some(art) = &c.art {
        let r = Rect::from_min_size(pos2(x, y), Vec2::splat(ART));
        ui.painter().rect_filled(r, 8.0, theme::glass(0.08));
        paint_picture(ui, r, art, 8.0, "large", animate);
        let mut over_badge = false;
        if let Some(small) = &c.small_art {
            let badge = Rect::from_center_size(r.right_bottom() - vec2(8.0, 8.0), Vec2::splat(30.0));
            ui.painter().circle_filled(badge.center(), 17.0, theme::panel(1.0));
            paint_picture(ui, badge, small, 15.0, "small", animate);
            over_badge = ui.rect_contains_pointer(badge);
            if let Some(text) = &c.small_text {
                ui.interact(badge, ui.scope_id().with("discord small artwork"), Sense::hover()).on_hover_text(text);
            }
        }
        // The badge overlaps the cover. Only the topmost image should own its tooltip.
        if !over_badge && let Some(line) = &c.lines[2] {
            ui.interact(r, ui.scope_id().with("discord large artwork"), Sense::hover()).on_hover_text(&line.text);
        }
        x += ART + 12.0;
    }
    let tw = rect.right() - PAD - x;
    let mut ty = y + ((body_h - text_h) / 2.0).max(0.0);
    for (i, l) in lines.iter().enumerate() {
        let (font, color) = if i == 0 && c.lines[0].is_some() { (theme::bold(15.0), theme::text()) } else { (theme::regular(13.5), theme::text_dim()) };
        if let Some(url) = &l.url {
            let galley = widgets::galley(ui, &l.text, font, color, tw);
            let r = Rect::from_min_size(pos2(x, ty), vec2(galley.size().x, LINE));
            let link = ui.interact(r, ui.scope_id().with(("discord preview line", i)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
            link.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Link, true, &l.text));
            ui.painter().galley(r.min, galley, color);
            if link.hovered() {
                ui.painter().hline(r.x_range(), r.bottom() - 3.0, egui::Stroke::new(1.0, color));
            }
            if link.clicked() {
                ui.ctx().open_url(egui::OpenUrl::new_tab(url));
            }
        } else {
            widgets::text_at(ui, pos2(x, ty), &l.text, font, color, tw);
        }
        ty += LINE;
    }
    if let Some(text) = &c.annotation {
        widgets::text_at(ui, pos2(x, ty), text, theme::regular(12.5), theme::text_dim(), tw);
        ty += LINE;
    }
    if let Some((start, end)) = c.times {
        elapsed(ui, pos2(x, ty + TIME / 2.0), tw, start, end);
    }
    y += body_h + 12.0;
    for (label, _) in &c.buttons {
        let r = Rect::from_min_size(pos2(rect.left() + PAD, y), vec2(inner_w, BUTTON));
        ui.painter().rect_filled(r, 8.0, theme::glass(0.10));
        ui.painter().text(r.center(), Align2::CENTER_CENTER, label, theme::semibold(13.5), theme::text());
        y += BUTTON + GAP;
    }
}

#[derive(Clone)]
struct PreviewTexture {
    key: String,
    index: usize,
    texture: egui::TextureHandle,
}

fn paint_picture(ui: &Ui, r: Rect, picture: &Picture, radius: f32, slot: &str, animate: bool) {
    if !ui.is_rect_visible(r) {
        return;
    }
    if let Some(rotation) = &picture.rotation {
        paint_rotation(ui, r, rotation, radius, animate);
    } else if let Some(preview) = &picture.preview {
        let id = ui.scope_id().with(("discord preview texture", slot));
        let length = if animate { preview.frames.last().map_or(0, |(end, _)| *end) } else { 0 };
        let now = (ui.input(|i| i.time) * 1000.0) as u64;
        let at = if length > 0 { (now % u64::from(length)) as u32 } else { 0 };
        let index = preview.frames.partition_point(|(end, _)| *end <= at).min(preview.frames.len() - 1);
        let frame = || preview.frames[index].1.clone();
        let mut state = ui.ctx().data(|data| data.get_temp::<PreviewTexture>(id)).unwrap_or_else(|| PreviewTexture {
            key: preview.key.clone(),
            index,
            texture: ui.ctx().load_texture("Discord artwork preview", frame(), egui::TextureOptions::LINEAR),
        });
        if state.key != preview.key || state.index != index {
            state.texture.set(frame(), egui::TextureOptions::LINEAR);
            state.key.clone_from(&preview.key);
            state.index = index;
        }
        let sprite = widgets::MotionSprite { id: state.texture.id(), size: state.texture.size_vec2() };
        widgets::paint_motion(ui, r, radius, &sprite);
        ui.ctx().data_mut(|data| data.insert_temp(id, state));
        if length > 0 {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(u64::from(preview.frames[index].0 - at)));
        }
    } else if let Some((step, paused)) = picture.badge {
        let uri = format!("bytes://discord-status-{step}-{paused}.png");
        ui.ctx().include_bytes(uri.clone(), discord_artwork::badge_bytes(step, paused));
        widgets::paint_remote(ui, &uri, r, radius, full_uv(), Color32::WHITE);
    } else if let Some(url) = &picture.url {
        widgets::paint_remote(ui, preview_uri(ui, url), r, radius, full_uv(), Color32::WHITE);
    }
}

fn preview_uri<'a>(ui: &Ui, url: &'a str) -> &'a str {
    if url == discord::ICON {
        const URI: &str = "bytes://discord-app-icon.png";
        ui.ctx().include_bytes(URI, discord::ICON_BYTES);
        URI
    } else {
        url
    }
}

fn paint_rotation(ui: &Ui, r: Rect, rotation: &discord_artwork::Rotation, radius: f32, animate: bool) {
    let uri = preview_uri(ui, &rotation.source);
    let Ok(egui::load::TexturePoll::Ready { texture }) = egui::Image::new(uri).load_for_size(ui.ctx(), r.size()) else {
        widgets::paint_remote(ui, uri, r, radius, full_uv(), Color32::WHITE);
        return;
    };
    let now = ui.input(|i| i.time) * 1000.0;
    let frame_ms = f64::from(rotation.length_ms) / f64::from(rotation.frames);
    let frame = if animate { (now / frame_ms).floor() } else { 0.0 };
    let angle = (frame.rem_euclid(f64::from(rotation.frames)) / f64::from(rotation.frames)) as f32 * std::f32::consts::TAU;
    let crop = Vec2::splat(texture.size.x.min(texture.size.y)) / texture.size;
    let center = r.center();
    let edge = r.width().min(r.height()) * 0.5;
    let mut mesh = egui::Mesh::with_texture(texture.id);
    const SIDES: u32 = 40;
    // A ring maps the cropped square texture to a disc with the same centre cutout as the GIF.
    for i in 0..=SIDES {
        let phase = i as f32 * std::f32::consts::TAU / SIDES as f32;
        let (y, x) = phase.sin_cos();
        let (v, u) = (phase - angle).sin_cos();
        for size in [1.0, 7.0 / 96.0] {
            mesh.vertices.push(egui::epaint::Vertex {
                pos: center + vec2(x, y) * (edge * size),
                uv: pos2(0.5, 0.5) + vec2(u, v) * crop * (size * 0.5),
                color: Color32::WHITE,
            });
        }
        if i < SIDES {
            let n = i * 2;
            mesh.add_triangle(n, n + 1, n + 2);
            mesh.add_triangle(n + 1, n + 3, n + 2);
        }
    }
    ui.painter().add(egui::Shape::mesh(mesh));
    if animate {
        ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64((((frame + 1.0) * frame_ms - now) / 1000.0).max(0.001)));
    }
}

/// Discord shows a progress bar for a ranged timestamp and a clock for a start alone.
fn elapsed(ui: &Ui, at: egui::Pos2, width: f32, start: i64, end: Option<i64>) {
    let now = now_ms();
    let secs = ((now - start) / 1000).max(0) as u32;
    if let Some(end) = end.filter(|e| *e > start) {
        let ratio = ((now - start) as f32 / (end - start) as f32).clamp(0.0, 1.0);
        let bar = Rect::from_min_size(at + vec2(0.0, -2.0), vec2(width, 4.0));
        ui.painter().rect_filled(bar, 2.0, theme::glass(0.15));
        ui.painter().rect_filled(Rect::from_min_size(bar.min, vec2(width * ratio, 4.0)), 2.0, GREEN);
        ui.interact(bar.expand(4.0), ui.scope_id().with("discord elapsed"), Sense::hover()).on_hover_text(format!(
            "{} / {}",
            super::fmt_time(secs),
            super::fmt_time(((end - start) / 1000) as u32)
        ));
    } else {
        widgets::icon_at(ui, at + vec2(6.0, 0.0), icon::MUSIC, 13.0, GREEN);
        widgets::time_at(ui, at + vec2(17.0, 0.0), Align2::LEFT_CENTER, &super::fmt_time(secs), theme::semibold(12.5), GREEN);
    }
}

fn member_row(ui: &mut Ui, c: &Card, name: &str, avatar: Option<&str>, width: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, 52.0), Sense::hover());
    ui.painter().rect_filled(rect, 8.0, theme::glass(0.07));
    let pic = Rect::from_center_size(pos2(rect.left() + 26.0, rect.center().y), Vec2::splat(34.0));
    let accent = ui.ctx().data(|d| d.get_temp::<Color32>(egui::Id::unique("accent"))).unwrap_or(theme::RED);
    ui.painter().circle_filled(pic.center(), 17.0, accent);
    if let Some(url) = avatar {
        widgets::paint_remote(ui, url, pic, 17.0, full_uv(), Color32::WHITE);
    }
    // Online: a green dot in a ring cut out of the picture.
    let dot = pic.right_bottom() - vec2(4.0, 4.0);
    ui.painter().circle_filled(dot, 7.0, Color32::from_black_alpha(220));
    ui.painter().circle_filled(dot, 4.5, GREEN);
    let x = pic.right() + 10.0;
    let w = rect.right() - 10.0 - x;
    widgets::text_at(ui, pos2(x, rect.top() + 8.0), name, theme::semibold(14.0), theme::text(), w);
    let y = rect.top() + 36.0;
    widgets::icon_at(ui, pos2(x + 6.0, y), icon::MUSIC, 12.0, GREEN);
    widgets::text_at(ui, pos2(x + 17.0, y - 8.0), &c.member_text(), theme::regular(12.5), theme::text_dim(), w - 17.0);
}

fn full_uv() -> Rect {
    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))
}
