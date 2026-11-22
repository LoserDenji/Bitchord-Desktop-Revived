#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod appicon;
mod artist_signature;
mod autostart;
mod canvas;
mod fullscreen;
mod hls;
mod proxy;
mod renderer;
mod art;
mod audio;
mod backend;
mod bidi;
mod cache;
mod deeplink;
mod discord;
mod discord_artwork;
mod discord_link;
mod dash;
mod downloads;
mod hires;
mod images;
mod jam;
mod innertube;
mod jsc;
mod lastfm;
mod listening;
mod local;
mod logging;
mod login;
mod lyrics;
mod media;
mod model;
mod motion;
mod notify;
mod opus;
mod party;
mod preset;
mod providers;
mod recommend;
mod shader;
mod sources;
mod sponsorblock;
mod stream;
mod theme;
mod transfer;
mod translate;
mod tray;
mod turnstile;
mod ui;
mod update;
mod vault;
mod video;
mod webmusic;
mod ytdlp;
mod ytm;
mod ytmapi;

use std::sync::{Arc, OnceLock};

use crossbeam_channel::unbounded;

/// Window size on first launch, and the smallest it may be (points).
pub const DEFAULT_SIZE: [f32; 2] = [1200.0, 780.0];
pub const MIN_SIZE: [f32; 2] = [760.0, 480.0];

fn main() -> eframe::Result {
    // glibc gives each thread that allocates its own heap (up to 8 per core), and what a
    // song frees in one isn't reused by the next song's threads: on Linux the app grew a
    // few MB with every song. Two heaps keep the freed memory in use. And big blocks (a
    // song's buffer, its pictures) go back to the system as soon as they're freed: glibc
    // otherwise raises that limit past them after the first one and keeps them in its
    // heaps. Before any thread starts; MALLOC_ARENA_MAX and MALLOC_MMAP_THRESHOLD_ still
    // set them.
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        // SAFETY: only changes allocator settings, while the process is single-threaded.
        if std::env::var_os("MALLOC_ARENA_MAX").is_none() {
            unsafe { libc::mallopt(libc::M_ARENA_MAX, 2) };
        }
        if std::env::var_os("MALLOC_MMAP_THRESHOLD_").is_none() {
            unsafe { libc::mallopt(libc::M_MMAP_THRESHOLD, 128 * 1024) };
        }
    }
    // Child-process modes for the webview windows (see login.rs / turnstile.rs).
    let mut args = std::env::args_os().skip(1);
    if let (Some(flag), Some(dir)) = (args.next(), args.next()) {
        // The parent hands its proxy setting down through the environment.
        proxy::init(std::path::Path::new(""));
        // Children are separate processes: their warnings and panics land in their
        // own log, not the main one (which the parent holds open).
        if flag == login::FLAG {
            logging::init_child(&data_dir(), "login");
            login::child_main(login::Site::Google, dir.into());
            return Ok(());
        }
        if flag == login::SPOTIFY_FLAG {
            logging::init_child(&data_dir(), "spotify");
            login::child_main(login::Site::Spotify, dir.into());
            return Ok(());
        }
        if flag == login::DISCORD_FLAG {
            logging::init_child(&data_dir(), "discord");
            login::child_main(login::Site::Discord, dir.into());
            return Ok(());
        }
        if flag == turnstile::FLAG {
            logging::init_child(&data_dir(), "turnstile");
            turnstile::child_main(dir.into());
            return Ok(());
        }
    }
    // The single-instance hand-off, before anything slow: the first launch takes the
    // per-user channel and serves it; a later one (the shortcut clicked again, a jam
    // link) finds it taken, hands its message over and exits — or, when the holder
    // turns out to be gone, takes the channel over and carries on. Before `init_logging`,
    // which would truncate the running instance's log file.
    let claim = deeplink::claim();
    let mut show_requested = false;
    let mut handed = false;
    if claim.busy() {
        if let Some(invite) = deeplink::from_args() {
            handed = deeplink::forward(&invite.message());
        } else {
            handed = deeplink::forward_show();
        }
        show_requested = !handed && std::env::args_os().skip(1).any(|a| a == deeplink::SHOW_FLAG);
        if !handed {
            // Nobody answered: the holder died mid-startup. Keep its channel for us.
            deeplink::take_over();
        }
    }
    if handed {
        return Ok(());
    }
    if let Some(code) = deeplink::from_args() {
        deeplink::push(code);
    }
    logging::init(&data_dir());
    guard_settings(&data_dir());
    if show_requested {
        if claim.busy() {
            // The holder was there but didn't answer; `take_over` has the channel now.
            log::warn!("--show: the running copy didn't answer; starting");
        } else {
            log::info!("--show: no running copy to hand the window to; starting");
        }
    }
    update::clean_portable();
    stream::init_dev_hooks();
    proxy::init(&data_dir());

    // Dev hook: YTM_BACKGROUND opens the window off-screen, unfocused and off the
    // taskbar, so automated test runs don't pop up over whatever the user is doing.
    let background = std::env::var_os("YTM_BACKGROUND").is_some();
    ui::init_titlebar(&data_dir());
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("BitChord")
        .with_app_id("bitchord")
        .with_inner_size(DEFAULT_SIZE)
        .with_min_inner_size(MIN_SIZE)
        // Windows, and Linux where the desktop draws none: the app draws its own title
        // bar (see `ui::titlebar`).
        .with_decorations(!ui::custom_titlebar())
        // macOS: its frame, without its title strip; the traffic lights go on the app's
        // top row (see `ui::titlebar`).
        .with_fullsize_content_view(ui::titlebar_inset())
        .with_titlebar_shown(!ui::titlebar_inset())
        .with_title_shown(!ui::titlebar_inset())
        .with_icon(appicon::get(appicon::load(&data_dir())).icon_data().expect("bundled icons are valid PNGs"));
    if background {
        viewport = viewport.with_position([-4000.0, 0.0]).with_inner_size([1920.0, 1057.0]).with_active(false).with_taskbar(false);
    }
    let renderer = renderer::init(&data_dir());
    let options = eframe::NativeOptions {
        viewport,
        renderer: match renderer {
            renderer::Renderer::OpenGl => eframe::Renderer::Glow,
            renderer::Renderer::Wgpu => eframe::Renderer::Wgpu,
        },
        persist_window: !background,
        // Settings live with the rest of the data: beside the portable exe, or a
        // YTM_DATA_DIR profile (e.g. a clean signed-out one for screenshots).
        persistence_path: Some(data_dir().join("app.ron")),
        // No MSAA/depth/stencil: egui doesn't need them and they cost VRAM.
        multisampling: 0,
        depth_buffer: 0,
        stencil_buffer: 0,
        // Dev hooks: YTM_FPS paces frames itself (see `ui::pace`), so vsync is off;
        // YTM_NO_VSYNC turns it off as a driver forcing it off would.
        glow_options: eframe::egui_glow::GlowConfiguration {
            vsync: std::env::var_os("YTM_FPS").is_none() && std::env::var_os("YTM_NO_VSYNC").is_none(),
            ..Default::default()
        },
        wgpu_options: renderer::wgpu_options(),
        ..Default::default()
    };

    let run = eframe::run_native(
        "BitChord",
        options,
        Box::new(|cc| {
            let ctx = cc.egui_ctx.clone();
            theme::install_style(&ctx);
            theme::install_fonts(&ctx, "inter");

            // Background threads wake the UI through this; set once the context exists.
            static CTX: OnceLock<egui::Context> = OnceLock::new();
            let _ = CTX.set(ctx.clone());
            let wake = || {
                if let Some(c) = CTX.get() {
                    c.request_repaint();
                }
            };

            // Jam links from later launches arrive over the channel claimed at startup:
            // this is where their wake-up meets the UI.
            deeplink::set_wake(Arc::new(move || wake()));
            // Before the first frame, which then draws in the desktop's light or dark style.
            #[cfg(target_os = "linux")]
            theme::follow_system_scheme(wake);
            // Whether song notifications can show here (Windows: a Start menu entry).
            notify::look_up();
            // Keep `ytm-rs:` links pointing at this copy; dev builds leave it alone
            // (even with YTM_UPDATE_FORCE).
            if update::installed() && !cfg!(debug_assertions) {
                deeplink::register_protocol();
            }

            let volume = cc
                .storage
                .and_then(|s| s.get_string("volume"))
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.8f32);
            // The slider is logarithmic unless turned off. A volume saved before that
            // existed was linear: move it to where it sounds the same.
            let log_setting = cc.storage.and_then(|s| s.get_string("log_volume"));
            let log_volume = log_setting.as_deref().is_none_or(|v| v == "1");
            let volume = if log_setting.is_none() { volume.cbrt() } else { volume };
            // Dev hook: YTM_MUTE starts silent and never persists the volume.
            let volume = if std::env::var_os("YTM_MUTE").is_some() { 0.0 } else { volume };

            let device = cc.storage.and_then(|s| s.get_string("output_device")).filter(|d| !d.is_empty());

            let (audio_tx, audio_rx) = unbounded();
            let audio = audio::spawn(volume, log_volume, device, audio_tx, wake);

            let data_dir = data_dir();
            let (resp_tx, resp_rx) = unbounded();
            let backend = backend::Backend::new(data_dir, resp_tx, audio.clone(), Arc::new(wake))?;

            egui_extras::install_image_loaders(&ctx);
            ctx.add_bytes_loader(Arc::new(images::HttpImages::new(backend.clone())));

            let (media_tx, media_rx) = unbounded();
            if let Some(h) = hwnd(cc) {
                appicon::set_hwnd(h);
                fullscreen::set_hwnd(h);
                ui::set_titlebar_hwnd(h);
                ui::set_mini_hwnd(h);
                // eframe sets only the small icon; the taskbar button wants the big one.
                appicon::set_window_icon(appicon::get(appicon::load(backend.data_dir())));
            }
            #[cfg(target_os = "macos")]
            if let Ok(h) = raw_window_handle::HasWindowHandle::window_handle(cc)
                && let raw_window_handle::RawWindowHandle::AppKit(h) = h.as_raw()
            {
                ui::set_titlebar_view(h.ns_view.as_ptr());
            }
            let media = media::Media::new(hwnd(cc), media_tx, media::Direct::new(audio.clone()), wake);

            // Which GL driver this is, for reports of drawing problems (Windows on ARM runs
            // GL through Mesa's D3D12 layer).
            if let Some(gl) = &cc.gl {
                use eframe::glow::HasContext;
                // SAFETY: plain string queries on the current context.
                let (renderer, version) = unsafe { (gl.get_parameter_string(eframe::glow::RENDERER), gl.get_parameter_string(eframe::glow::VERSION)) };
                log::info!("gl: {renderer} ({version})");
            }
            if let Some(rs) = &cc.wgpu_render_state {
                let info = rs.adapter.get_info();
                log::info!("wgpu: {} ({:?}, driver {} {})", info.name, info.backend, info.driver, info.driver_info);
            }
            // The animated background needs the GPU context; without it the UI falls back
            // to a flat gradient.
            let shader = std::env::var_os("YTM_NO_SHADER").is_none();
            let bg = match (&cc.gl, &cc.wgpu_render_state) {
                (Some(gl), _) if shader => shader::Background::new(gl).map_err(|e| log::warn!("background shader unavailable: {e:#}")).ok(),
                (_, Some(rs)) if shader => Some(shader::Background::new_wgpu(rs)),
                _ => None,
            };

            Ok(Box::new(ui::App::new(
                backend,
                audio,
                ui::Channels { resp_rx, audio_rx, media_rx },
                media,
                bg,
                // `--no-tray`: a session with "Close to tray" off, so × quits for real
                // (for when the tray icon can't be trusted).
                std::env::args_os().skip(1).any(|a| a == deeplink::NO_TRAY_FLAG),
                cc.storage,
            )))
        }),
    );
    // A wgpu window that won't open would fail the same way at every launch: go back
    // to OpenGL for the next one.
    if run.is_err() && renderer == renderer::Renderer::Wgpu {
        log::error!("renderer: {} failed to start; switching back to OpenGL", renderer::api_name());
        let _ = renderer::save(&data_dir(), renderer::Renderer::OpenGl);
    }
    run
}

#[cfg(windows)]
fn hwnd(cc: &eframe::CreationContext) -> Option<*mut std::ffi::c_void> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match cc.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get() as *mut std::ffi::c_void),
        _ => None,
    }
}

#[cfg(not(windows))]
fn hwnd(_: &eframe::CreationContext) -> Option<*mut std::ffi::c_void> {
    None
}

/// Settings, caches and the sign-in: `data` beside the portable build's exe, else the
/// user's app data. Dev hook: YTM_DATA_DIR runs on a separate profile.
fn data_dir() -> std::path::PathBuf {
    if let Some(d) = std::env::var_os("YTM_DATA_DIR") {
        return d.into();
    }
    if let Some(dir) = update::portable_dir() {
        return dir.join("data");
    }
    directories::ProjectDirs::from("", "", "ytm-rs").expect("no home directory").data_dir().to_path_buf()
}

/// eframe reads an unparsable `app.ron` as no settings at all, and its next save
/// overwrites what's left — a kill mid-write (the updater's installer asking the app
/// to close, a power cut) shows up at the next launch as every setting wiped. So the
/// file is kept one good copy deep: a copy that parses is backed up, a copy that
/// doesn't is set aside and the backup takes its place, before eframe ever opens it.
/// The half-written copy stays as `app.ron.bad` for anyone who wants it back.
pub(crate) fn guard_settings(dir: &std::path::Path) {
    use std::io::BufReader;
    let path = dir.join("app.ron");
    let Ok(file) = std::fs::File::open(&path) else { return };
    let reads = ron::de::from_reader::<_, std::collections::HashMap<String, String>>(BufReader::new(file)).is_ok();
    if reads {
        let _ = std::fs::copy(&path, dir.join("app.ron.bak"));
        return;
    }
    log::warn!("settings: {} is unreadable; setting it aside", path.display());
    let _ = std::fs::rename(&path, dir.join("app.ron.bad"));
    let bak = dir.join("app.ron.bak");
    if std::fs::copy(&bak, &path).is_ok() {
        log::info!("settings: restored the last good copy");
    }
}
