//! Discord, Last.fm, the system media controls, the app icon, updates and the tray.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use crate::backend;

use super::{App, dev_var, notes, updatable};

/// A close request this soon after launch is not the user: cancel it and stay up.
/// (Reports have seen the window vanish into the tray within half a second of
/// starting, behind an icon that was never drawn.)
const TRAY_GRACE: Duration = Duration::from_secs(2);
/// How often the tray icon is registered again: Shell_NotifyIcon can succeed and
/// the icon still never show (Windows), and a fresh registration heals that. Also
/// how long a failed registration waits before trying again.
const TRAY_REREGISTER: Duration = Duration::from_secs(15);
/// How many frames a show is re-asserted over, for shells that drop the first one.
const SHOW_ASSERTS: u8 = 8;

/// The Discord sign-in window's outcome, handed over from its thread.
static LINK_RESULT: std::sync::Mutex<Option<Result<(String, Option<crate::discord_link::User>), String>>> = std::sync::Mutex::new(None);

impl App {
    pub(super) fn set_discord_enabled(&mut self, enabled: bool) {
        self.discord_enabled = enabled;
        if enabled {
            // A linked account publishes presence through the account's own gateway;
            // without one, the plain IPC client (the app id's card) does it.
            if self.discord_link_token.is_some() {
                if self.discord_link.is_none() {
                    self.discord_link = Some(Arc::new(crate::discord_link::Service::new(&self.backend)));
                }
            } else if self.discord.is_none() {
                self.discord = crate::discord::Discord::spawn();
            }
        } else {
            if let Some(discord) = self.discord.take() {
                discord.set(None);
            }
            if let Some(link) = self.discord_link.take() {
                link.shutdown();
            }
            self.discord_sent = None;
        }
    }

    /// Mirror playback into Discord. Cheap enough to run every frame: it only sends on a
    /// new song, play/pause, a seek, or a newly learned duration.
    pub(super) fn sync_discord(&mut self) {
        if !self.discord_enabled {
            return;
        }
        // A saved link comes back as a service on the first frame; the plain IPC
        // one stays for a listener who never linked Discord.
        if self.discord_link.is_none() && self.discord_link_token.is_some() {
            self.discord_link = Some(Arc::new(crate::discord_link::Service::new(&self.backend)));
        }
        // The linked account's card goes through the account's own gateway; the plain
        // IPC one stays for a listener who never linked Discord.
        let linked = self.discord_link.is_some() && self.discord_link_token.is_some();
        if !linked && self.discord.is_none() {
            return;
        }
        let track = self.playing_track().filter(|_| self.playing() && !self.buffering);
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as i64;
        let start_ms = now - self.audio.status.position().as_millis() as i64;
        let dur = self.audio.status.duration_ms.load(Ordering::Relaxed) as i64;
        // Position ticks every 100 ms; only a jump of 2 s or more is a real seek.
        let same = match (track, &self.discord_sent) {
            (Some(t), Some(b)) => b.video_id == t.id && (start_ms - b.start_ms).abs() < 2000 && b.end_ms.is_some() == (dur > 0),
            (None, None) => true,
            _ => false,
        };
        if same {
            return;
        }
        let want = track.map(|t| crate::discord::Presence {
            video_id: t.id.clone(),
            title: t.title.clone(),
            artist: t.artist_line(),
            album: t.album.as_ref().map(|a| a.name.clone()),
            art: backend::art_url(t),
            start_ms,
            end_ms: (dur > 0).then_some(start_ms + dur),
        });
        if linked {
            let link = self.discord_link.as_ref().expect("checked above").clone();
            let token = self.discord_link_token.clone().expect("checked above");
            link.set_presence(token, want.clone(), self.discord_options.clone(), 1.0);
        } else if let Some(d) = &self.discord {
            d.set(want.clone());
        }
        self.discord_sent = want;
    }

    /// Connect Discord: the sign-in window opens discord.com; the token it hides
    /// behind a fresh iframe is captured and filed in the vault, then the account is
    /// asked who it is (for the row's name and picture).
    pub(super) fn link_discord(&mut self) {
        if self.discord_link_busy {
            return;
        }
        self.discord_link_busy = true;
        let profile = self.backend.data_dir().join("webview-discord");
        let backend = self.backend.clone();
        let done = move |outcome| {
            let b = backend.clone();
            backend.spawn(async move {
                match outcome {
                    crate::login::Outcome::Token(token) => {
                        if let Err(e) = crate::vault::set("discord token", &b.data_dir(), token.as_bytes()) {
                            log::warn!("saving the Discord sign-in: {e}");
                        }
                        let service = crate::discord_link::Service::new(&b);
                        let user = service.user_info(&token).await.ok();
                        *LINK_RESULT.lock().unwrap() = Some(Ok((token, user)));
                    }
                    crate::login::Outcome::Cancelled => {}
                    crate::login::Outcome::Failed(e) => {
                        *LINK_RESULT.lock().unwrap() = Some(Err(e));
                    }
                    crate::login::Outcome::Cookie(_) => {}
                }
                b.wake();
            });
        };
        crate::login::spawn(crate::login::Site::Discord, profile, done);
    }

    /// The sign-in window's outcome, if one has landed since last asked.
    pub(super) fn take_discord_link_result(&mut self) -> Option<Result<(String, Option<crate::discord_link::User>), String>> {
        let r = LINK_RESULT.lock().unwrap().take()?;
        self.discord_link_busy = false;
        Some(r)
    }

    /// Disconnect the linked account: the gateway goes away and the token is deleted.
    pub(super) fn unlink_discord(&mut self) {
        if let Some(link) = self.discord_link.take() {
            link.shutdown();
        }
        crate::vault::delete("discord token", &self.backend.data_dir());
        self.discord_link_token = None;
        self.discord_link_user = None;
        self.discord_sent = None;
        // Fall back to the plain IPC card while presence stays on.
        if self.discord_enabled && self.discord.is_none() {
            self.discord = crate::discord::Discord::spawn();
        }
    }

    /// Tell Last.fm what's playing, and scrobble it once it has played half its length
    /// or 4 minutes (songs under 30 s don't count). Runs every frame, like Discord's.
    pub(super) fn sync_lastfm(&mut self) {
        let result = self.lastfm_result.lock().unwrap().take();
        if let Some(r) = result {
            match r {
                Ok(name) => {
                    self.toast(format!("Scrobbling to Last.fm as {name}"));
                    self.lastfm_user = Some(name);
                    self.scrobble = None;
                }
                Err(e) => self.toast(format!("Last.fm sign-in failed: {e}")),
            }
        }
        if self.lastfm_user.is_none() {
            return;
        }
        let Some(t) = self.playing_track().filter(|_| self.playing() && !self.buffering) else { return };
        let pos = self.audio.status.position();
        let dur = Duration::from_millis(self.audio.status.duration_ms.load(Ordering::Relaxed));
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
        let play = crate::lastfm::Scrobble {
            artist: t.artists.first().map(|a| a.name.clone()).unwrap_or_default(),
            track: t.title.clone(),
            album: t.album.as_ref().map(|a| a.name.clone()),
            duration: (!dur.is_zero()).then_some(dur.as_secs() as u32),
            timestamp: now - pos.as_secs() as i64,
        };
        if play.artist.is_empty() {
            return;
        }
        let http = self.backend.http();
        if self.scrobble.is_none_or(|(g, _)| g != self.generation) {
            self.scrobble = Some((self.generation, false));
            let p = play.clone();
            self.backend.spawn(async move { crate::lastfm::now_playing(&http, &p).await });
            return;
        }
        if dur >= Duration::from_secs(30) && pos >= (dur / 2).min(Duration::from_secs(240)) && self.scrobble.is_some_and(|(_, done)| !done) {
            self.scrobble = Some((self.generation, true));
            self.backend.spawn(async move { crate::lastfm::scrobble(&http, play).await });
        }
    }

    pub(super) fn lastfm_sign_in(&mut self) {
        if self.lastfm_busy.swap(true, Ordering::Relaxed) {
            return;
        }
        let (busy, result, http, backend) = (self.lastfm_busy.clone(), self.lastfm_result.clone(), self.backend.http(), self.backend.clone());
        self.backend.spawn(async move {
            let r = crate::lastfm::sign_in(&http).await.map_err(|e| format!("{e:#}"));
            *result.lock().unwrap() = Some(r);
            busy.store(false, Ordering::Relaxed);
            backend.wake();
        });
    }

    pub(super) fn lastfm_sign_out(&mut self) {
        crate::lastfm::sign_out();
        self.lastfm_user = None;
        self.scrobble = None;
    }

    /// Update checks: shortly after launch, then every six hours. A downloaded update is
    /// only offered as a notice; nothing installs until the user restarts for it.
    pub(super) fn tick_updates(&mut self, ctx: &egui::Context, now: f64) {
        use crate::update::{State, Updater};
        if self.updater.is_none() {
            let c = ctx.clone();
            self.updater = Some(Updater::new(self.backend.data_dir(), self.update_nightly, move || c.request_repaint()));
        }
        let Some(u) = self.updater.clone() else { return };
        let due = match self.update_checked {
            None => now > 8.0,
            Some(t) => now - t > 6.0 * 3600.0 && !matches!(u.state(), State::Ready { .. }),
        };
        let enabled = updatable() && dev_var("YTM_BACKGROUND").is_none_or(|_| dev_var("YTM_UPDATE_FORCE").is_some() || dev_var("YTM_UPDATE_FEED").is_some());
        if enabled && due {
            self.update_checked = Some(now);
            u.check();
        } else if enabled {
            // An idle app doesn't repaint on its own: wake up when the next check is due.
            let next = self.update_checked.map_or(8.0, |t| t + 6.0 * 3600.0);
            ctx.request_repaint_after(Duration::from_secs_f64((next - now).max(0.5) + 0.1));
        }
        match u.state() {
            State::Ready { version, .. } if !self.update_announced => {
                self.update_announced = true;
                self.notes_dialog = Some(notes::NotesDialog { version, ready: true });
            }
            // The .deb: once per version, not every launch (the sidebar keeps saying so).
            State::Available { version, .. } if !self.update_announced && self.update_seen.as_ref() != Some(&version) => {
                self.update_announced = true;
                self.update_seen = Some(version.clone());
                self.notes_dialog = Some(notes::NotesDialog { version, ready: true });
            }
            _ => {}
        }
    }

    /// Switch between the stable and nightly update channels, and look for an update
    /// on the new one right away.
    pub(super) fn set_update_nightly(&mut self, nightly: bool) {
        if nightly == self.update_nightly {
            return;
        }
        self.update_nightly = nightly;
        log::info!("update channel: {}", if nightly { "nightly" } else { "stable" });
        if let Some(u) = &self.updater {
            u.set_nightly(nightly);
            if updatable() {
                u.check();
            }
        }
    }

    /// Close the app so the downloaded update installs, then relaunch it.
    pub(super) fn restart_to_update(&mut self, ctx: &egui::Context) {
        self.restart_for_update = true;
        self.quit(ctx);
    }

    /// Close and start again, e.g. so a new proxy setting takes effect.
    pub(super) fn restart(&mut self, ctx: &egui::Context) {
        self.restart_app = true;
        self.quit(ctx);
    }

    /// Close the app for real, even with "Close to tray" on.
    pub(super) fn quit(&mut self, ctx: &egui::Context) {
        self.exiting = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    /// "Close to tray": × hides the window behind the tray icon; the icon brings it
    /// back or quits. eframe keeps running `logic` while the window is hidden.
    ///
    /// The icon itself is always up (ForkChord's tray: Open, transport, Launch at
    /// login, Quit), from the first frame, so the transport buttons work with the
    /// window open too. The hidden window must still never become unreachable, so
    /// three things hold here: a close that arrives moments after launch is treated
    /// as stale (reports have seen the window vanish into a tray whose icon was
    /// never drawn); the icon is re-registered every so often, because on Windows
    /// the shell can accept it and then never display it (taskbar replacements, a
    /// busy tray cache); and if the icon can't be had at all, a close just quits
    /// instead of hiding.
    pub(super) fn tray_window(&mut self, ctx: &egui::Context) {
        // Shells mid-refresh have been seen to drop a single show request: keep
        // re-asserting it for a few frames, focus last, once. (Before the early
        // returns below: after a show, `hidden_to_tray` is false and the rest of
        // the hide bookkeeping must not run.)
        if self.show_asserts > 0 {
            if ctx.input(|i| i.viewport().minimized == Some(true)) {
                // The user minimized it again meanwhile: their call wins.
                self.show_asserts = 0;
            } else {
                self.show_asserts -= 1;
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                if self.show_asserts == 0 {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
            }
        }
        // The icon, from the first frame on (--no-tray sessions never get one).
        // A failed registration isn't retried every frame; the next try comes at
        // the re-register interval.
        if self.tray.is_none() && !self.tray_off && !self.exiting && self.tray_retry_at.is_none_or(|t| t.elapsed() >= TRAY_REREGISTER) {
            let c = ctx.clone();
            match crate::tray::Tray::new(crate::appicon::get(self.app_icon), self.playing() || self.buffering, crate::autostart::enabled(), move || c.request_repaint()) {
                Ok(t) => {
                    log::info!("tray icon up (Open, transport, Launch at login, Quit)");
                    self.tray = Some(t);
                    self.tray_registered_at = Some(std::time::Instant::now());
                    self.tray_retry_at = None;
                }
                Err(e) => {
                    log::warn!("tray icon unavailable: {e:#}");
                    self.tray_retry_at = Some(std::time::Instant::now());
                }
            }
        }
        // The tray's Play/Pause label follows the player; it only changes with it.
        let playing = self.playing() || self.buffering;
        if self.tray.is_some() && self.tray_play != Some(playing) {
            if let Some(t) = &self.tray {
                t.set_play(playing);
            }
            self.tray_play = Some(playing);
        }
        if ctx.input(|i| i.viewport().close_requested()) && self.close_to_tray && !self.exiting {
            // A real click on × needs the app to have been up for a moment. The
            // upgrade/install path has been seen to deliver a close request right at
            // startup; hiding on it leaves just the (possibly undrawn) icon behind.
            if self.started_at.elapsed() < TRAY_GRACE {
                log::warn!(
                    "ignoring a close request {:.1}s after launch (too early to hide to the tray); keeping the window visible",
                    self.started_at.elapsed().as_secs_f32()
                );
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            } else if self.tray.is_some() {
                log::info!("closing to the tray");
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                self.hidden_to_tray = true;
            }
            // Without an icon there'd be no way back: the close runs as a quit.
        }
        // While the icon is up, re-register it now and then: Shell_NotifyIcon can
        // succeed and the icon still never show (the report behind this fix), and
        // a fresh registration is the only way to heal it. Windows only — macOS's
        // menu bar item and the GTK icon don't vanish like this, and re-creating
        // them would only flicker.
        #[cfg(windows)]
        if self.tray.is_some() && self.tray_registered_at.is_none_or(|t| t.elapsed() >= TRAY_REREGISTER) {
            let c = ctx.clone();
            match crate::tray::Tray::new(crate::appicon::get(self.app_icon), playing, crate::autostart::enabled(), move || c.request_repaint()) {
                Ok(t) => {
                    log::info!("re-registered the tray icon");
                    self.tray = Some(t);
                    self.tray_registered_at = Some(std::time::Instant::now());
                }
                Err(e) => {
                    log::warn!("tray icon could not be re-registered: {e:#}; bringing the window back");
                    self.show_window(ctx);
                }
            }
        }
        if self.tray.is_none() {
            return;
        }
        // The tray's clicks, handled like the keyboard's own transport keys.
        for cmd in crate::tray::take() {
            match cmd {
                crate::tray::Cmd::Show => self.show_window(ctx),
                crate::tray::Cmd::PlayPause => self.toggle(),
                crate::tray::Cmd::Next => self.next(),
                crate::tray::Cmd::Prev => self.prev(),
                crate::tray::Cmd::Autostart => {
                    let on = crate::autostart::toggle();
                    if let Some(t) = &self.tray {
                        t.set_autostart(on);
                    }
                }
                crate::tray::Cmd::Quit => {
                    self.quit(ctx);
                    break;
                }
            }
        }
    }

    /// Bring the window back (from the tray, or minimized) and to the front. The
    /// tray icon stays up, transport buttons and all.
    pub(super) fn show_window(&mut self, ctx: &egui::Context) {
        if std::mem::take(&mut self.hidden_to_tray) {
            log::info!("bringing the window back from the tray");
        }
        // Re-asserted over the next few frames (see `tray_window`), so one dropped
        // request doesn't strand the window.
        self.show_asserts = SHOW_ASSERTS;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    /// Download the media controls' cover through our own client, into a file.
    pub(super) fn fetch_media_cover(&self, video_id: String, url: String) {
        let (slot, b) = (self.media_cover.clone(), self.backend.clone());
        let dir = self.backend.data_dir().join("media-cover");
        self.backend.spawn(async move {
            let res = async {
                let bytes = b.http().get(&url).send().await?.error_for_status()?.bytes().await?;
                // A new name per song, so Windows doesn't show a cached older cover.
                let _ = std::fs::remove_dir_all(&dir);
                std::fs::create_dir_all(&dir)?;
                let path = dir.join(format!("{video_id}.jpg"));
                std::fs::write(&path, &bytes)?;
                anyhow::Ok(path)
            }
            .await;
            match res {
                Ok(path) => {
                    *slot.lock().unwrap() = Some((video_id, format!("file://{}", path.display())));
                    b.wake();
                }
                Err(e) => log::debug!("media cover: {e:#}"),
            }
        });
    }
}
