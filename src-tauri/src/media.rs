//! Native media controls. The web frontend drives `navigator.mediaSession`
//! through the shim; this module mirrors that state into the platform's
//! player: MPRIS on Linux (souvlaki) and the macOS Now Playing widget
//! (souvlaki). Windows needs nothing here — WebView2's own media session
//! already owns SMTC and the hardware keys — but the tray still reads the
//! snapshot for its Play/Pause label.

use tauri::{AppHandle, Manager};

use souvlaki::{MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig};

use crate::commands::{platform_name, AppState, MediaUpdate};

#[derive(Debug, Clone, Default)]
pub struct MediaSnapshot {
    pub playback_state: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub artwork_url: Option<String>,
    pub duration_sec: f64,
    pub position_sec: f64,
    pub rate: f64,
}

// The controls live on the main thread (a requirement on macOS; harmless on
// Linux), so async commands reach them through `run_on_main_thread`.
thread_local! {
    static CONTROLS: std::cell::RefCell<Option<MediaControls>> = const { std::cell::RefCell::new(None) };
}

/// Creates the platform controls and wires their buttons into the page.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn attach(app: &AppHandle) {
    let config = PlatformConfig {
        dbus_name: "bitchord",
        display_name: "BitChord",
        hwnd: None,
    };
    // attach takes &mut self — and this whole path is cfg'd to Linux/macOS,
    // so a Windows-only build never saw it.
    let mut controls = match MediaControls::new(config) {
        Ok(controls) => controls,
        Err(error) => {
            log::warn!("media controls unavailable: {error}");
            return;
        }
    };
    let handle = app.clone();
    let attach_result = controls.attach(move |event| handle_event(&handle, event));
    if let Err(error) = attach_result {
        log::warn!("media controls attach failed: {error}");
        return;
    }
    CONTROLS.with(|cell| *cell.borrow_mut() = Some(controls));
}

#[cfg(target_os = "windows")]
pub fn attach(_app: &AppHandle) {}

fn handle_event(app: &AppHandle, event: MediaControlEvent) {
    // The page binds play and pause separately, each guarded by the player's
    // own state; Toggle resolves against the last reported state.
    let playing = app
        .try_state::<AppState>()
        .map(|state| state.media.lock().map(|m| m.playback_state == "playing").unwrap_or(false))
        .unwrap_or(false);
    let (action, arg): (&str, Option<f64>) = match event {
        MediaControlEvent::Play => ("play", None),
        MediaControlEvent::Pause => ("pause", None),
        MediaControlEvent::Toggle => {
            if playing {
                ("pause", None)
            } else {
                ("play", None)
            }
        }
        MediaControlEvent::Next => ("nexttrack", None),
        MediaControlEvent::Previous => ("previoustrack", None),
        MediaControlEvent::Stop => ("pause", None),
        MediaControlEvent::SetPosition(position) => ("seekto", Some(position.0.as_secs_f64())),
        MediaControlEvent::SeekBy(direction, duration) => (
            match direction {
                souvlaki::SeekDirection::Forward => "seekforward",
                souvlaki::SeekDirection::Backward => "seekbackward",
            },
            Some(duration.as_secs_f64()),
        ),
        MediaControlEvent::Seek(direction) => (
            match direction {
                souvlaki::SeekDirection::Forward => "seekforward",
                souvlaki::SeekDirection::Backward => "seekbackward",
            },
            Some(10.0),
        ),
        _ => return,
    };
    eval_in_main(
        app,
        &format!(
            "window.__BitChordMediaAction({}, {})",
            serde_json::to_string(action).unwrap_or_else(|_| "\"\"".into()),
            arg.map(|seconds| seconds.to_string()).unwrap_or_else(|| "undefined".into())
        ),
    );
    if let MediaControlEvent::Quit = event {
        let _ = app.exit(0);
    }
}

/// Runs a script in the main window, from any thread.
pub fn eval_in_main(app: &AppHandle, script: &str) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.eval(script);
    }
}

/// The shim's throttled updates land here: the snapshot feeds the tray, and
/// the platform controls are refreshed on the main thread.
pub fn apply_update(app: &AppHandle, update: MediaUpdate) {
    let state = app.state::<AppState>();
    let mut snapshot = state.media.lock().unwrap().clone();

    match update.kind.as_str() {
        "metadata" => {
            if let Some(metadata) = &update.metadata {
                // A genuinely new track (not a re-report of the same one)
                // starts its own timeline: drop the old track's position at
                // once, so the island and MPRIS never count down the previous
                // song's remaining time over the new one's title while the
                // page's first position update is still in flight.
                let track_changed = snapshot.title != metadata.title || snapshot.artwork_url != metadata.artwork_url;
                snapshot.title = metadata.title.clone();
                snapshot.artist = metadata.artist.clone();
                snapshot.album = metadata.album.clone();
                snapshot.artwork_url = metadata.artwork_url.clone();
                if track_changed {
                    snapshot.position_sec = 0.0;
                }
            }
        }
        "playback" => {
            snapshot.playback_state = update.state.clone().unwrap_or_default();
        }
        "position" => {
            snapshot.duration_sec = update.duration;
            snapshot.position_sec = update.position;
            snapshot.rate = if update.rate > 0.0 { update.rate } else { 1.0 };
        }
        _ => {}
    }
    *state.media.lock().unwrap() = snapshot.clone();

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        let for_controls = snapshot.clone();
        let kind = update.kind.clone();
        let _ = app.run_on_main_thread(move || {
            CONTROLS.with(|cell| {
                let mut guard = cell.borrow_mut();
                let Some(controls) = guard.as_mut() else { return };
                // souvlaki's metadata borrows: `as_deref` maps Option<String> → Option<&str>.
                let result = match kind.as_str() {
                    "metadata" => controls.set_metadata(MediaMetadata {
                        title: for_controls.title.as_deref(),
                        artist: for_controls.artist.as_deref(),
                        album: for_controls.album.as_deref(),
                        duration: Some(std::time::Duration::from_secs_f64(for_controls.duration_sec.max(0.0))),
                        cover_url: for_controls.artwork_url.as_deref(),
                    }),
                    "playback" | "position" => controls.set_playback(match for_controls.playback_state.as_str() {
                        "playing" => MediaPlayback::Playing {
                            progress: Some(MediaPosition(std::time::Duration::from_secs_f64(for_controls.position_sec.max(0.0)))),
                        },
                        "paused" => MediaPlayback::Paused {
                            progress: Some(MediaPosition(std::time::Duration::from_secs_f64(for_controls.position_sec.max(0.0)))),
                        },
                        _ => MediaPlayback::Stopped,
                    }),
                    _ => Ok(()),
                };
                if let Err(error) = result {
                    log::warn!("media controls update failed: {error}");
                }
            });
        });
    }

    crate::tray::sync_play_label(app, &snapshot);

    // The Dynamic Island redraws from the same snapshot the tray reads —
    // one truth for playback, three mirrors (tray, MPRIS, island).
    crate::island::sync(app, &snapshot);
}

/// Whether the player is currently sounding, for the tray's label.
pub fn is_playing(snapshot: &MediaSnapshot) -> bool {
    snapshot.playback_state == "playing"
}

#[allow(dead_code)]
fn _platform_reference() {
    let _ = platform_name();
}
