//! System tray icon, always up from the first frame (ForkChord's tray: Open,
//! transport buttons, Launch at login, Quit). Its Play/Pause label follows the
//! player like a real desktop player's would, and it keeps working while the
//! window is hidden behind it — the music goes on after a close.
//!
//! The icon must be created on the UI thread (it owns a Win32 message window), so
//! the app creates and drops it from `logic`. Clicks arrive on that thread's message
//! loop; they are queued here and the UI is woken to handle them. On macOS it is a
//! menu bar item, made the same way (AppKit wants it on the main thread, which is the
//! UI thread there).
//!
//! On Linux the icon is a GTK one, and GTK runs its own loop on one thread for the
//! life of the process (it can't be started twice); the icon is made and dropped
//! there, and its menu items are touched only on that thread — the label updates
//! below hop over by way of the GTK main context.
//!
//! Note that on Windows the shell can accept the icon (so `Tray::new` succeeds) and
//! then never draw it — taskbar replacements and a busy notification-area cache can
//! both do that. Callers therefore re-create the icon every so often (see
//! `ui::integrations::tray_window`), and recovery never depends on the icon alone:
//! launching the app again (or `--show`) shows the window regardless.

use std::sync::{Mutex, Once};

use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmd {
    Show,
    PlayPause,
    Next,
    Prev,
    Autostart,
    Quit,
}

static PENDING: Mutex<Vec<Cmd>> = Mutex::new(Vec::new());

pub struct Tray {
    #[cfg(not(target_os = "linux"))]
    _icon: TrayIcon,
    /// The Play/Pause item, so its label can follow the player while the menu is up.
    #[cfg(not(target_os = "linux"))]
    play: MenuItem,
    /// The Launch at login item, so the tick answers right after a click on it.
    #[cfg(not(target_os = "linux"))]
    autostart: CheckMenuItem,
}

impl Tray {
    /// `wake` is called (from the UI thread's message loop) after each click.
    /// `playing` and `autostart_on` are what the menu shows until the next update.
    pub fn new(icon: &crate::appicon::Variant, playing: bool, autostart_on: bool, wake: impl Fn() + Send + Sync + Clone + 'static) -> anyhow::Result<Self> {
        // The handlers are process-wide and can only be set once.
        static HANDLERS: Once = Once::new();
        HANDLERS.call_once(|| {
            let w = wake.clone();
            TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
                if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                    push(Cmd::Show, &w);
                }
            }));
            MenuEvent::set_event_handler(Some(move |e: MenuEvent| match e.id.as_ref() {
                "show" => push(Cmd::Show, &wake),
                "play-pause" => push(Cmd::PlayPause, &wake),
                "next" => push(Cmd::Next, &wake),
                "previous" => push(Cmd::Prev, &wake),
                "autostart" => push(Cmd::Autostart, &wake),
                "quit" => push(Cmd::Quit, &wake),
                _ => {}
            }));
        });

        let png = icon.rgba(32).ok_or_else(|| anyhow::anyhow!("icon doesn't decode"))?;
        #[cfg(not(target_os = "linux"))]
        return Self::non_linux(png, playing, autostart_on);
        #[cfg(target_os = "linux")]
        {
            gtk_thread::show(png, playing, autostart_on)?;
            Ok(Self {})
        }
    }

    #[cfg(not(target_os = "linux"))]
    fn non_linux(png: image::RgbaImage, playing: bool, autostart_on: bool) -> anyhow::Result<Self> {
        let menu = TrayMenu::new(playing, autostart_on);
        let (w, h) = png.dimensions();
        let icon = TrayIconBuilder::new()
            .with_icon(tray_icon::Icon::from_rgba(png.into_raw(), w, h)?)
            .with_tooltip("BitChord")
            .with_menu(Box::new(menu.menu))
            .with_menu_on_left_click(false)
            .build()?;
        Ok(Self { _icon: icon, play: menu.play, autostart: menu.autostart })
    }

    /// Keeps the tray's Play/Pause label honest as the player changes state.
    #[cfg(not(target_os = "linux"))]
    pub fn set_play(&self, playing: bool) {
        self.play.set_text(if playing { "Pause" } else { "Play" });
    }

    /// Flips the Launch at login tick after the app toggled the OS state itself.
    #[cfg(not(target_os = "linux"))]
    pub fn set_autostart(&self, on: bool) {
        self.autostart.set_checked(on);
    }

    /// The menu item handles live on the GTK thread on Linux; these hop over.
    #[cfg(target_os = "linux")]
    pub fn set_play(&self, playing: bool) {
        gtk_thread::set_play(playing);
    }

    #[cfg(target_os = "linux")]
    pub fn set_autostart(&self, on: bool) {
        gtk_thread::set_autostart(on);
    }
}

/// The tray menu, ForkChord's: Open, transport (Play/Pause named for the state it
/// would switch to, like every desktop player's), Launch at login, Quit.
struct TrayMenu {
    menu: Menu,
    play: MenuItem,
    autostart: CheckMenuItem,
}

impl TrayMenu {
    fn new(playing: bool, autostart_on: bool) -> Self {
        let open = MenuItem::with_id("show", "Open BitChord", true, None);
        let previous = MenuItem::with_id("previous", "Previous", true, None);
        let play = MenuItem::with_id("play-pause", if playing { "Pause" } else { "Play" }, true, None);
        let next = MenuItem::with_id("next", "Next", true, None);
        let autostart = CheckMenuItem::with_id("autostart", "Launch at login", true, autostart_on, None);
        let quit = MenuItem::with_id("quit", "Quit BitChord", true, None);
        let menu = Menu::with_items(&[
            &open,
            &PredefinedMenuItem::separator(),
            &previous,
            &play,
            &next,
            &PredefinedMenuItem::separator(),
            &autostart,
            &PredefinedMenuItem::separator(),
            &quit,
        ])
        .expect("tray menu");
        Self { menu, play, autostart }
    }
}

#[cfg(target_os = "linux")]
impl Drop for Tray {
    fn drop(&mut self) {
        gtk_thread::hide();
    }
}

#[cfg(target_os = "linux")]
fn build(png: image::RgbaImage, playing: bool, autostart_on: bool) -> anyhow::Result<(TrayIcon, MenuItem, CheckMenuItem)> {
    let menu = TrayMenu::new(playing, autostart_on);
    let (w, h) = png.dimensions();
    let icon = TrayIconBuilder::new()
        .with_icon(tray_icon::Icon::from_rgba(png.into_raw(), w, h)?)
        .with_tooltip("BitChord")
        .with_menu(Box::new(menu.menu))
        .with_menu_on_left_click(false)
        .build()?;
    Ok((icon, menu.play, menu.autostart))
}

#[cfg(target_os = "linux")]
mod gtk_thread {
    use std::cell::RefCell;
    use std::sync::OnceLock;
    use std::time::Duration;

    use tray_icon::TrayIcon;

    thread_local! {
        static ICON: RefCell<Option<TrayIcon>> = const { RefCell::new(None) };
        /// The menu items that follow the player and the login setting; kept so
        /// their labels can be set from this thread alone.
        static LABELS: RefCell<Option<(tray_icon::menu::MenuItem, tray_icon::menu::CheckMenuItem)>> = const { RefCell::new(None) };
    }

    /// Start GTK's thread once; an error if GTK can't start (no display).
    fn start() -> anyhow::Result<()> {
        static STARTED: OnceLock<Result<(), String>> = OnceLock::new();
        STARTED
            .get_or_init(|| {
                let (tx, rx) = std::sync::mpsc::channel();
                std::thread::Builder::new()
                    .name("gtk".into())
                    .spawn(move || match gtk::init() {
                        Ok(()) => {
                            let _ = tx.send(Ok(()));
                            gtk::main();
                        }
                        Err(e) => {
                            let _ = tx.send(Err(e.to_string()));
                        }
                    })
                    .map_err(|e| e.to_string())?;
                rx.recv().map_err(|e| e.to_string())?
            })
            .clone()
            .map_err(|e| anyhow::anyhow!("GTK: {e}"))
    }

    pub fn show(png: image::RgbaImage, playing: bool, autostart_on: bool) -> anyhow::Result<()> {
        start()?;
        let (tx, rx) = std::sync::mpsc::channel();
        gtk::glib::MainContext::default().invoke(move || {
            let res = super::build(png, playing, autostart_on).map(|(icon, play, autostart)| {
                ICON.with(|i| *i.borrow_mut() = Some(icon));
                LABELS.with(|l| *l.borrow_mut() = Some((play, autostart)));
            });
            let _ = tx.send(res.map_err(|e| e.to_string()));
        });
        rx.recv_timeout(Duration::from_secs(5))?.map_err(|e| anyhow::anyhow!(e))
    }

    pub fn hide() {
        gtk::glib::MainContext::default().invoke(|| ICON.with(|i| drop(i.borrow_mut().take())));
    }

    pub fn set_play(playing: bool) {
        gtk::glib::MainContext::default().invoke(move || {
            LABELS.with(|l| {
                if let Some((play, _)) = l.borrow().as_ref() {
                    play.set_text(if playing { "Pause" } else { "Play" });
                }
            });
        });
    }

    pub fn set_autostart(on: bool) {
        gtk::glib::MainContext::default().invoke(move || {
            LABELS.with(|l| {
                if let Some((_, autostart)) = l.borrow().as_ref() {
                    autostart.set_checked(on);
                }
            });
        });
    }
}

fn push(cmd: Cmd, wake: &impl Fn()) {
    PENDING.lock().unwrap().push(cmd);
    wake();
}

/// Clicks since the last call.
pub fn take() -> Vec<Cmd> {
    std::mem::take(&mut *PENDING.lock().unwrap())
}
