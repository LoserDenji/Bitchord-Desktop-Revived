//! The system tray: Open, transport buttons and Quit. Its Play/Pause label
//! follows the player like a real desktop player's would, and it works even
//! when the window is hidden — the music keeps going after a close.

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager};

use crate::media;
use crate::commands::AppState;

/// The tray's Play/Pause item, kept so its label can follow the player.
pub struct TrayPlayItem(pub std::sync::Mutex<Option<MenuItem<tauri::Wry>>>);

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open BitChord", true, None::<&str>)?;
    let play = MenuItem::with_id(app, "play-pause", "Play", true, None::<&str>)?;
    let next = MenuItem::with_id(app, "next", "Next", true, None::<&str>)?;
    let previous = MenuItem::with_id(app, "previous", "Previous", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let autostart = MenuItem::with_id(app, "autostart", "Launch at login", false, None::<&str>)?;
    let separator2 = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "Quit BitChord", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &separator, &previous, &play, &next, &separator2, &autostart, &quit])?;

    let mut builder = TrayIconBuilder::with_id("bitchord")
        .menu(&menu)
        .tooltip("BitChord")
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => show_main(app),
            "play-pause" => toggle_play(app),
            "next" => media::eval_in_main(app, "window.__BitChordMediaAction('nexttrack')"),
            "previous" => media::eval_in_main(app, "window.__BitChordMediaAction('previoustrack')"),
            "autostart" => toggle_autostart(app),
            "quit" => {
                // A tray quit is a real quit, this time and every time: nothing
                // is persisted. (An old build stored `close_to_tray: false`
                // here, which then made every later close button quit the app.)
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let tauri::tray::TrayIconEvent::Click { button: tauri::tray::MouseButton::Left, button_state: tauri::tray::MouseButtonState::Up, .. } = event {
                show_main(tray.app_handle());
            }
        });
    if let Ok(icon) = tauri::image::Image::from_bytes(include_bytes!("../icons/32x32.png")) {
        builder = builder.icon(icon);
    } else if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    app.manage(TrayPlayItem(std::sync::Mutex::new(Some(play))));
    Ok(())
}

/// Raises the main window; the tray's Open and the island's card share it.
pub fn show_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn toggle_play(app: &AppHandle) {
    let playing = app
        .try_state::<AppState>()
        .map(|state| media::is_playing(&state.media.lock().unwrap().clone()))
        .unwrap_or(false);
    let action = if playing { "pause" } else { "play" };
    media::eval_in_main(app, &format!("window.__BitChordMediaAction('{action}')"));
}

fn toggle_autostart(app: &AppHandle) {
    use tauri_plugin_autostart::ManagerExt;
    let autostart = app.autolaunch();
    let on = autostart.is_enabled().unwrap_or(false);
    let result = if on { autostart.disable() } else { autostart.enable() };
    if result.is_ok() {
        // The menu item flips itself on the next open; nothing else to do.
    }
}

/// Keeps the tray's Play/Pause label honest as the player changes state.
pub fn sync_play_label(app: &AppHandle, snapshot: &media::MediaSnapshot) {
    let Some(play_item) = app.try_state::<TrayPlayItem>() else { return };
    let Ok(guard) = play_item.0.lock() else { return };
    if let Some(item) = guard.as_ref() {
        let _ = item.set_text(if media::is_playing(snapshot) { "Pause" } else { "Play" });
    }
}
