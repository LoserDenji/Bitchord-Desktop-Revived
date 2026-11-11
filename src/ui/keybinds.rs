//! The player's own keyboard shortcuts: which key does what, and Settings › Shortcuts,
//! where each one can be taken over. Combinations live in storage as readable strings
//! (`"ctrl+ArrowRight"`); a command without one saved keeps its default.

use egui::{Align2, Sense, Ui, pos2, vec2};

use super::notes;
use super::pages;
use super::{App, widgets};
use crate::theme;

/// Everything a key can do here. The media keys a listener reaches for most, the views,
/// and the transport the mouse also drives. Esc keeps its own job (closing now playing,
/// leaving the full screen) and isn't listed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmd {
    TogglePlay,
    Next,
    Prev,
    VolumeUp,
    VolumeDown,
    Lyrics,
    Queue,
    NowPlaying,
    FullScreen,
    Search,
    MiniPlayer,
    Back,
    Forward,
}

impl Cmd {
    pub const ALL: [Cmd; 13] = [
        Self::TogglePlay,
        Self::Next,
        Self::Prev,
        Self::VolumeUp,
        Self::VolumeDown,
        Self::Lyrics,
        Self::Queue,
        Self::NowPlaying,
        Self::FullScreen,
        Self::Search,
        Self::MiniPlayer,
        Self::Back,
        Self::Forward,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::TogglePlay => "Play / pause",
            Self::Next => "Next song",
            Self::Prev => "Previous song",
            Self::VolumeUp => "Volume up",
            Self::VolumeDown => "Volume down",
            Self::Lyrics => "Lyrics",
            Self::Queue => "Queue",
            Self::NowPlaying => "Now playing",
            Self::FullScreen => "Full-screen player",
            Self::Search => "Search",
            Self::MiniPlayer => "Mini player",
            Self::Back => "Back",
            Self::Forward => "Forward",
        }
    }

    /// The storage key the combination is saved under.
    fn key(self) -> &'static str {
        match self {
            Self::TogglePlay => "keybind_play",
            Self::Next => "keybind_next",
            Self::Prev => "keybind_prev",
            Self::VolumeUp => "keybind_vol_up",
            Self::VolumeDown => "keybind_vol_down",
            Self::Lyrics => "keybind_lyrics",
            Self::Queue => "keybind_queue",
            Self::NowPlaying => "keybind_np",
            Self::FullScreen => "keybind_full",
            Self::Search => "keybind_search",
            Self::MiniPlayer => "keybind_mini",
            Self::Back => "keybind_back",
            Self::Forward => "keybind_forward",
        }
    }

    /// What the key does by default.
    fn default_combo(self) -> &'static str {
        match self {
            Self::TogglePlay => "Space",
            Self::Next => "ctrl+ArrowRight",
            Self::Prev => "ctrl+ArrowLeft",
            Self::VolumeUp => "ctrl+ArrowUp",
            Self::VolumeDown => "ctrl+ArrowDown",
            Self::Lyrics => "L",
            Self::Queue => "Q",
            Self::NowPlaying => "F",
            Self::FullScreen => "shift+F",
            Self::Search => "Slash",
            Self::MiniPlayer => "ctrl+M",
            Self::Back => "alt+ArrowLeft",
            Self::Forward => "alt+ArrowRight",
        }
    }
}

/// The combination for each command; a missing one keeps the default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bindings {
    combos: Vec<String>,
}

impl Default for Bindings {
    fn default() -> Self {
        Self { combos: Cmd::ALL.map(|cmd| cmd.default_combo().to_owned()).to_vec() }
    }
}

impl Bindings {
    pub fn get(&self, cmd: Cmd) -> &str {
        Cmd::ALL.iter().position(|c| *c == cmd).map_or(cmd.default_combo(), |i| self.combos[i].as_str())
    }

    pub fn set(&mut self, cmd: Cmd, combo: String) {
        if let Some(i) = Cmd::ALL.iter().position(|c| *c == cmd) {
            self.combos[i] = combo;
        }
    }

    /// The command already on `combo`, if any other.
    pub fn used_by(&self, combo: &str, except: Cmd) -> Option<Cmd> {
        Cmd::ALL.into_iter().find(|c| *c != except && self.get(*c) == combo)
    }

    /// Whether `cmd`'s combination is down right now — and the kind of widget that has
    /// the keyboard allows taking it: plain keys stay out of anything that wants keys
    /// (a text box, a slider), modified ones only leave when a text box specifically
    /// wants them (Ctrl+arrows move its cursor).
    pub fn matches(&self, cmd: Cmd, ctx: &egui::Context) -> bool {
        self.pressed(cmd, ctx, false)
    }

    /// The same, but only once per press: holding the key doesn't keep firing (what the
    /// mini player's toggle wants).
    pub fn matches_once(&self, cmd: Cmd, ctx: &egui::Context) -> bool {
        self.pressed(cmd, ctx, true)
    }

    fn pressed(&self, cmd: Cmd, ctx: &egui::Context, once: bool) -> bool {
        let Some((want_mods, want_key)) = parse_combo(self.get(cmd)) else { return false };
        let wanted = !if want_mods.is_none() { ctx.egui_wants_keyboard_input() } else { ctx.text_edit_focused() };
        let (want_ctrl, want_shift, want_alt) = want_mods.map_or((false, false, false), |m| (m.ctrl, m.shift, m.alt));
        let same_mods = |m: egui::Modifiers| m.ctrl == want_ctrl && m.shift == want_shift && m.alt == want_alt;
        ctx.input(|i| {
            if once {
                wanted
                    && i.events.iter().any(
                        |e| matches!(e, egui::Event::Key { key, pressed: true, repeat: false, modifiers, .. } if *key == want_key && same_mods(*modifiers)),
                    )
            } else {
                wanted && i.key_pressed(want_key) && same_mods(i.modifiers)
            }
        })
    }

    /// The saved combinations, or the defaults when nothing readable is stored.
    pub fn load(get: &dyn Fn(&str) -> Option<String>) -> Self {
        let mut out = Self::default();
        for (i, cmd) in Cmd::ALL.into_iter().enumerate() {
            if let Some(v) = get(cmd.key())
                && parse_combo(&v).is_some()
            {
                out.combos[i] = v;
            }
        }
        out
    }

    pub fn save(&self, storage: &mut dyn eframe::Storage) {
        for (i, cmd) in Cmd::ALL.into_iter().enumerate() {
            storage.set_string(cmd.key(), self.combos[i].clone());
        }
    }
}

/// The settings' working set: the combinations, and a combination being picked.
pub(super) struct State {
    pub bindings: Bindings,
    pub capture: Option<Capture>,
}

impl State {
    pub(super) fn load(get: &dyn Fn(&str) -> Option<String>) -> Self {
        Self { bindings: Bindings::load(get), capture: None }
    }

    pub(super) fn get(&self, cmd: Cmd) -> &str {
        self.bindings.get(cmd)
    }

    fn set(&mut self, cmd: Cmd, combo: String) {
        self.bindings.set(cmd, combo);
    }

    fn used_by(&self, combo: &str, except: Cmd) -> Option<Cmd> {
        self.bindings.used_by(combo, except)
    }
}

/// `"ctrl+shift+F"` → (modifiers, key), as egui reads input. `None` for anything this
/// build doesn't take.
fn parse_combo(combo: &str) -> Option<(Option<Mods>, egui::Key)> {
    let mut mods = Mods::default();
    let mut key = None;
    for part in combo.split('+') {
        match part.trim().to_ascii_lowercase().as_str() {
            "ctrl" => mods.ctrl = true,
            "shift" => mods.shift = true,
            "alt" => mods.alt = true,
            // The key keeps the case it was saved in: egui's names are mixed-case
            // ("ArrowUp", "Space"), and they're what `from_name` looks for.
            _ => key = key_from_name(part.trim()).or(key),
        }
    }
    Some((if mods == Mods::default() { None } else { Some(mods) }, key?))
}

/// The key a stored name stands for. `egui::Key::from_name` is case-sensitive, and the
/// combinations are saved with egui's own names ("ArrowUp", "Space"), but a name typed
/// by hand reads just as well whatever case it's in.
fn key_from_name(name: &str) -> Option<egui::Key> {
    if let Some(key) = egui::Key::from_name(name) {
        return Some(key);
    }
    // Some of from_name's aliases aren't what `Key::name` answers ("ArrowUp" is stored
    // but `name` reads "Up"), so those go by their own names, any case. Anything else
    // finds its key by its name or symbol, any case again.
    match name.to_ascii_lowercase().as_str() {
        "arrowup" | "up" => Some(egui::Key::ArrowUp),
        "arrowdown" | "down" => Some(egui::Key::ArrowDown),
        "arrowleft" | "left" => Some(egui::Key::ArrowLeft),
        "arrowright" | "right" => Some(egui::Key::ArrowRight),
        "escape" | "esc" => Some(egui::Key::Escape),
        "return" | "numpadenter" => Some(egui::Key::Enter),
        _ => egui::Key::ALL.iter().copied().find(|k| {
            k.name().eq_ignore_ascii_case(name) || k.symbol_or_name().eq_ignore_ascii_case(name)
        }),
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct Mods {
    ctrl: bool,
    shift: bool,
    alt: bool,
}

/// The keycaps for a stored combination (`["Ctrl", "→"]`).
pub fn keycaps(combo: &str) -> Vec<String> {
    combo.split('+').map(|part| cap_name(part.trim()).to_owned()).collect()
}

/// The keycap a stored key name draws as.
fn cap_name(stored: &str) -> &str {
    match stored {
        "ctrl" => "Ctrl",
        "shift" => "Shift",
        "alt" => "Alt",
        "ArrowUp" => "↑",
        "ArrowDown" => "↓",
        "ArrowLeft" => "←",
        "ArrowRight" => "→",
        "Slash" => "/",
        "Comma" => ",",
        "Period" => ".",
        "Minus" => "-",
        "Equals" | "Plus" => "=",
        "Semicolon" => ";",
        "Backslash" => "\\",
        "OpenBracket" => "[",
        "CloseBracket" => "]",
        "Backtick" => "`",
        "Quote" => "'",
        "Backspace" => "⌫",
        "Enter" => "⏎",
        "Tab" => "⇥",
        other => other,
    }
}

/// The key a combination is picked on: what it's saved as (egui's name), so
/// [`egui::Key::from_name`] reads it back.
fn key_of(key: egui::Key) -> Option<&'static str> {
    use egui::Key as K;
    Some(match key {
        K::A => "A",
        K::B => "B",
        K::C => "C",
        K::D => "D",
        K::E => "E",
        K::F => "F",
        K::G => "G",
        K::H => "H",
        K::I => "I",
        K::J => "J",
        K::K => "K",
        K::L => "L",
        K::M => "M",
        K::N => "N",
        K::O => "O",
        K::P => "P",
        K::Q => "Q",
        K::R => "R",
        K::S => "S",
        K::T => "T",
        K::U => "U",
        K::V => "V",
        K::W => "W",
        K::X => "X",
        K::Y => "Y",
        K::Z => "Z",
        K::Num0 => "0",
        K::Num1 => "1",
        K::Num2 => "2",
        K::Num3 => "3",
        K::Num4 => "4",
        K::Num5 => "5",
        K::Num6 => "6",
        K::Num7 => "7",
        K::Num8 => "8",
        K::Num9 => "9",
        K::F1 => "F1",
        K::F2 => "F2",
        K::F3 => "F3",
        K::F4 => "F4",
        K::F5 => "F5",
        K::F6 => "F6",
        K::F7 => "F7",
        K::F8 => "F8",
        K::F9 => "F9",
        K::F10 => "F10",
        K::F11 => "F11",
        K::F12 => "F12",
        K::ArrowUp => "ArrowUp",
        K::ArrowDown => "ArrowDown",
        K::ArrowLeft => "ArrowLeft",
        K::ArrowRight => "ArrowRight",
        K::Space => "Space",
        K::Enter => "Enter",
        K::Tab => "Tab",
        K::Backspace => "Backspace",
        K::Delete => "Delete",
        K::Insert => "Insert",
        K::Home => "Home",
        K::End => "End",
        K::PageUp => "PageUp",
        K::PageDown => "PageDown",
        K::Minus => "Minus",
        K::Plus | K::Equals => "Equals",
        K::Comma => "Comma",
        K::Period => "Period",
        K::Semicolon => "Semicolon",
        K::Slash => "Slash",
        K::Backslash => "Backslash",
        K::OpenBracket => "OpenBracket",
        K::CloseBracket => "CloseBracket",
        K::Backtick => "Backtick",
        K::Quote => "Quote",
        _ => return None,
    })
}

/// A combination being picked for `cmd`.
pub(super) struct Capture {
    cmd: Cmd,
    error: Option<String>,
}

/// Saves `combo` and puts it to work at once (it lands in storage with the rest of the
/// settings, next save).
fn set(app: &mut App, cmd: Cmd, combo: String) {
    app.keybinds.set(cmd, combo.clone());
    app.keybinds.capture = None;
    app.toast(format!("{} is now on {}", cmd.label(), keycaps(&combo).join(" + ")));
}

/// The dialog that takes the new combination: Esc cancels, anything else binds.
pub(super) fn capture_dialog(app: &mut App, ctx: &egui::Context) {
    let Some(cmd) = app.keybinds.capture.as_ref().map(|c| c.cmd) else { return };
    let (events, held) = ctx.input(|i| (i.events.clone(), i.modifiers));
    // Gone to another app: the player's keys shouldn't stay off behind this dialog.
    if ctx.input(|i| i.viewport().focused) == Some(false) {
        app.keybinds.capture = None;
        return;
    }
    for event in events {
        let egui::Event::Key { key, pressed: true, repeat: false, modifiers, .. } = event else { continue };
        if key == egui::Key::Escape {
            app.keybinds.capture = None;
            return;
        }
        // The physical key is what gets saved, so Shift+1 binds Shift and that key.
        let Some(name) = key_of(key) else { continue };
        let combo = match (modifiers.ctrl, modifiers.alt, modifiers.shift) {
            (false, false, false) => name.to_owned(),
            (ctrl, alt, shift) => {
                let mut parts = Vec::new();
                if ctrl {
                    parts.push("ctrl");
                }
                if alt {
                    parts.push("alt");
                }
                if shift {
                    parts.push("shift");
                }
                format!("{}+{}", parts.join("+"), name)
            }
        };
        let error = app.keybinds.used_by(&combo, cmd).map(|other| format!("That's already {}.", other.label().to_lowercase()));
        if let Some(error) = error {
            if let Some(capture) = &mut app.keybinds.capture {
                capture.error = Some(error);
            }
            continue;
        }
        set(app, cmd, combo);
        return;
    }
    let error = app.keybinds.capture.as_ref().and_then(|c| c.error.clone());
    let mut cancelled = false;
    egui::Modal::new(egui::Id::unique("keybind-capture")).frame(notes::surface()).backdrop_color(theme::shade(0.55)).show(ctx, |ui| {
        ui.set_width(400.0_f32.min(ctx.content_rect().width() - 48.0));
        egui::Frame::new().inner_margin(egui::Margin { left: 26, right: 26, top: 24, bottom: 22 }).show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            ui.label(egui::RichText::new(cmd.label()).font(theme::bold(22.0)).color(theme::text()));
            ui.add_space(6.0);
            ui.label(egui::RichText::new("Press the new shortcut, or Esc to cancel.").font(theme::regular(13.0)).color(theme::text_dim()));
            ui.add_space(16.0);
            let mut names = Vec::new();
            if held.ctrl {
                names.push("Ctrl");
            }
            if held.alt {
                names.push("Alt");
            }
            if held.shift {
                names.push("Shift");
            }
            let text = if names.is_empty() { "Waiting for a key".to_owned() } else { format!("{} + …", names.join(" + ")) };
            ui.label(egui::RichText::new(text).font(theme::semibold(17.0)).color(theme::text()));
            if let Some(error) = error {
                ui.add_space(10.0);
                ui.label(egui::RichText::new(error).font(theme::regular(12.5)).color(theme::RED));
            }
            ui.add_space(20.0);
            cancelled = widgets::pill(ui, "Cancel", None, false).clicked();
        });
    });
    if cancelled {
        app.keybinds.capture = None;
    }
}

/// Settings › Shortcuts: every command with its keys, a Change button each, and Reset
/// for the ones that left their defaults. Seek and volume stay on the arrows beside the
/// songs' skip; Esc keeps its own job.
pub(super) fn settings_section(app: &mut App, ui: &mut Ui) {
    pages::section_label(ui, "Keyboard shortcuts", None);
    pages::settings_card(ui, |ui| {
        let mut change = None;
        let mut reset = None;
        for (i, cmd) in Cmd::ALL.into_iter().enumerate() {
            let caps = keycaps(app.keybinds.get(cmd));
            if i > 0 {
                pages::divider(ui);
            }
            let customized = app.keybinds.get(cmd) != cmd.default_combo();
            let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::hover());
            let mid = rect.center().y;
            ui.painter().text(pos2(rect.left(), mid), Align2::LEFT_CENTER, cmd.label(), theme::regular(14.0), theme::text());
            if customized {
                let name_w = ui.painter().layout_no_wrap(cmd.label().to_owned(), theme::regular(14.0), theme::text()).size().x;
                let size = ui.painter().layout_no_wrap("Reset".to_owned(), theme::regular(12.5), theme::text_dim()).size();
                let area = Align2::LEFT_CENTER.anchor_size(pos2(rect.left() + name_w + 14.0, mid), size);
                let link = ui.interact(area.expand(3.0), ui.scope_id().with(("keybind-reset", cmd.key())), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
                let color = if link.hovered() { theme::text() } else { theme::text_dim() };
                ui.painter().text(area.left_center(), Align2::LEFT_CENTER, "Reset", theme::regular(12.5), color);
                if link.clicked() {
                    reset = Some(cmd);
                }
            }
            let mut controls = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::right_to_left(egui::Align::Center)));
            if widgets::pill(&mut controls, "Change", None, false).clicked() {
                change = Some(cmd);
            }
            let mut x = controls.cursor().right() - 14.0;
            for (j, key) in caps.iter().enumerate().rev() {
                x = pages::keycap(ui, pos2(x, mid), key) - 6.0;
                if j > 0 {
                    let plus = ui.painter().text(pos2(x, mid), Align2::RIGHT_CENTER, "+", theme::regular(13.0), theme::text_dim());
                    x = plus.left() - 6.0;
                }
            }
        }
        // Seek keeps the plain arrows and volume the Ctrl arrows; Esc keeps its own job.
        pages::divider(ui);
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::hover());
        ui.painter().text(pos2(rect.left(), rect.center().y), Align2::LEFT_CENTER, "Seek and volume", theme::regular(14.0), theme::text_dim());
        let mut x = rect.right();
        for key in ["Ctrl + ↑ / ↓", "← / →"] {
            x = pages::keycap(ui, pos2(x, rect.center().y), key) - 6.0;
        }
        if let Some(cmd) = reset {
            set(app, cmd, cmd.default_combo().to_owned());
        }
        if let Some(cmd) = change {
            // Let go first: pressing a combination still held would never reach the window.
            app.keybinds.capture = Some(Capture { cmd, error: None });
        }
    });
}
