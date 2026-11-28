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
        if !enabled {
            self.discord_artwork.allow_publish(false);
        }
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

    /// A notification when a new song starts while the window is hidden, minimized or
    /// behind other windows (the mini player on show counts as seen). Not for the song
    /// picked in the app, nor the same song again (Repeat one).
    pub(super) fn sync_song_notification(&mut self, ctx: &egui::Context) {
        if !self.song_notifications || !self.playing() || self.buffering || !crate::notify::available() {
            return;
        }
        if self.notified.as_ref().is_some_and(|(g, _)| *g == self.generation) {
            return;
        }
        let Some(t) = self.current().cloned() else { return };
        let again = self.notified.as_ref().is_some_and(|(_, id)| *id == t.id);
        self.notified = Some((self.generation, t.id.clone()));
        let focused = ctx.input(|i| i.viewport().focused) != Some(false);
        let seen = !self.window_hidden && (focused || self.mini.showing());
        if again || seen {
            return;
        }
        if self.notifier.is_none() {
            self.notifier = crate::notify::Notifier::spawn();
        }
        let Some(n) = self.notifier.clone() else { return };
        let song = crate::notify::Song {
            title: t.title.clone(),
            artist: t.artist_line(),
            cover: None,
        };
        // A local song's cover is a file already; there's nothing to download for it.
        let local = crate::local::cover_file(&t.id);
        let (art, id, generation) = (backend::art_url(&t), t.id, self.generation);
        n.expect(generation);
        let dir = self.backend.data_dir().join("notification-cover");
        let http = self.backend.http();
        self.backend.spawn(async move {
            let cover = match local {
                Some(file) => Some(file),
                None => match art {
                    Some(url) => async {
                        let bytes = http.get(&url).timeout(Duration::from_secs(5)).send().await?.error_for_status()?.bytes().await?;
                        // A new name per song: Windows keeps showing an older picture by its path.
                        let _ = std::fs::remove_dir_all(&dir);
                        std::fs::create_dir_all(&dir)?;
                        let path = dir.join(format!("{id}.jpg"));
                        std::fs::write(&path, &bytes)?;
                        anyhow::Ok(path)
                    }
                    .await
                    .inspect_err(|e| log::debug!("notification cover: {e:#}"))
                    .ok(),
                    None => None,
                },
            };
            n.show(generation, crate::notify::Song { cover, ..song });
        });
    }

    /// What's playing, as Discord would get it, paused or not.
    pub(super) fn discord_presence(&self) -> Option<crate::discord::Presence> {
        if self.discord_finished == Some(self.generation) && self.resume_at.is_none() {
            return None;
        }
        let t = self.playing_track()?;
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as i64;
        // In real time: a song at 2x is over in half its length.
        let speed = self.audio.status.speed().max(0.1);
        let seek = self.audio.status.seek_revision.load(Ordering::Acquire);
        let paused = !self.playing() || self.buffering;
        let position = if paused { self.audio.status.position() } else { self.audio.status.position_smooth() };
        let position_ms = position.as_millis() as u64;
        let duration_ms = match self.audio.status.duration_ms.load(Ordering::Relaxed) {
            0 => t.duration.map_or(0, |d| u64::from(d) * 1000),
            value => value,
        };
        let start_ms = now - (position_ms as f64 / f64::from(speed)) as i64;
        let dur = (duration_ms as f64 / f64::from(speed)) as i64;
        // The stream's own sample rate can differ from the audio device's output rate.
        // The line says the tier, the bitrate and the rate: the stream's measured
        // numbers when the decoder reported them, the addon's own when it's the
        // source of truth, and the tier off the addon's info — a plain YouTube
        // stream has none, so it plays as High.
        let stats = self.stream_stats.as_ref().filter(|(g, _)| *g == self.generation).map(|(_, s)| s);
        let info = self.audio_info.as_ref().filter(|(g, _)| *g == self.generation).map(|(_, i)| i);
        let quality = match (stats, info) {
            (Some(s), Some(i)) => Some(crate::discord::quality_summary(i.tier(), s.bitrate_kbps, Some(s.sample_rate))),
            (Some(s), None) => Some(crate::discord::quality_summary("High", s.bitrate_kbps, Some(s.sample_rate))),
            (None, Some(i)) => Some(crate::discord::quality_summary(i.tier(), i.bitrate_kbps, i.sample_rate)),
            (None, None) => Some("High".to_owned()),
        };
        let code = self.jam.code();
        Some(crate::discord::Presence {
            video_id: t.id.clone(),
            title: t.title.clone(),
            artist: t.artist_line(),
            artist_id: t.first_artist_id().map(str::to_owned),
            album: t.album.as_ref().map(|a| a.name.clone()),
            album_id: t.album.as_ref().and_then(|a| a.id.clone()),
            art: backend::art_url(t),
            artist_art: self
                .artist
                .as_ref()
                .filter(|a| Some(a.id.as_str()) == t.first_artist_id())
                .and_then(|a| crate::model::pick(&a.banner, 160))
                .map(|u| crate::model::sized(u, 512)),
            quality,
            start_ms,
            end_ms: (dur > 0).then_some(start_ms + dur),
            paused,
            duration_ms,
            position_ms,
            clock: (self.generation, seek, speed.to_bits()),
            jam: (!code.is_empty()).then(|| crate::jam::share_link(&self.jam_server, &code)),
        })
    }

    /// Mirror playback into Discord. Cheap enough to run every frame: it only sends when
    /// the card would look different, as on a new song, play/pause, a seek or new settings.
    /// A linked account gets the card through its own gateway; a listener who never
    /// linked one gets the plain IPC card — both laid out by the same wording.
    pub(super) fn sync_discord(&mut self, ctx: &egui::Context) {
        if !self.discord_enabled {
            return;
        }
        // A saved link comes back as a service on the first frame.
        if self.discord_link.is_none() && self.discord_link_token.is_some() {
            self.discord_link = Some(Arc::new(crate::discord_link::Service::new(&self.backend)));
        }
        let linked = self.discord_link.is_some() && self.discord_link_token.is_some();
        if !linked && self.discord.is_none() {
            return;
        }
        let now = std::time::Instant::now();
        let prefs = &self.discord_prefs;
        if self.discord_edit.as_ref().is_none_or(|(wanted, _)| wanted != prefs) {
            self.discord_edit = Some((prefs.clone(), now));
        }
        let (wanted, edited) = self.discord_edit.as_ref().expect("settings were sampled above");
        let settle = std::time::Duration::from_millis(600).saturating_sub(now.duration_since(*edited));
        let applied = self.discord_applied.get_or_insert_with(|| wanted.clone());
        if applied != wanted {
            if settle.is_zero()
                || wanted.hide_details != applied.hide_details
                || wanted.when_paused != applied.when_paused
                || (applied.artwork.host && !wanted.artwork.host)
            {
                applied.clone_from(wanted);
            } else {
                ctx.request_repaint_after(settle);
            }
        }
        let prefs = self.discord_applied.as_ref().expect("settings were applied above");
        let presence = self.discord_presence().filter(|p| !p.paused || prefs.when_paused);
        self.discord_artwork.allow_publish(presence.is_some() && !prefs.hide_details && prefs.art && prefs.artwork.host);
        let want = presence.map(|p| {
            let artwork = self.discord_artwork.pictures(&self.backend, &p, prefs, self.playing_track(), false);
            crate::discord::Card::new(&p, prefs, artwork)
        });
        let same = match (&want, &self.discord_sent) {
            (Some(a), Some(b)) => a.same(b, false),
            (a, b) => a.is_none() && b.is_none(),
        };
        if !same {
            let badge_progress = want.as_ref().zip(self.discord_sent.as_ref()).is_some_and(|(a, b)| a.same(b, true));
            let delay = self.discord_updates.delay(now, prefs.refresh_seconds, badge_progress);
            let hiding = want.is_none()
                || (prefs.hide_details
                    && self
                        .discord_sent
                        .as_ref()
                        .is_some_and(|c| c.lines.iter().any(Option::is_some) || c.annotation.is_some() || c.art.is_some() || !c.buttons.is_empty() || c.times.is_some()));
            if !hiding && !delay.is_zero() {
                ctx.request_repaint_after(delay);
                return;
            }
            self.discord_updates.record(now);
            if linked {
                let link = self.discord_link.as_ref().expect("checked above").clone();
                let token = self.discord_link_token.clone().expect("checked above");
                link.set_presence(token, want.clone());
            } else if let Some(d) = &self.discord {
                d.set(want.clone());
            }
            self.discord_sent = want;
        }
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
        // A song's music video picked on its switch is still that song.
        let Some(t) = self.heard_song().filter(|_| self.playing() && !self.buffering) else { return };
        let pos = self.audio.status.position();
        let dur = Duration::from_millis(self.audio.status.duration_ms.load(Ordering::Relaxed));
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
        let play = crate::lastfm::Scrobble {
            artist: t.artists.first().map(|a| a.name.clone()).unwrap_or_default(),
            track: t.title.clone(),
            album: t.album.as_ref().map(|a| a.name.clone()),
            duration: (!dur.is_zero()).then_some(dur.as_secs() as u32),
            // When it started, in real time.
            timestamp: now - (pos.as_secs_f64() / self.audio.status.speed() as f64) as i64,
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
            // A packaged copy (.deb, Arch, Flatpak): once per version, not every launch (the sidebar keeps saying so).
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
    pub(super) fn tray_window(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
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
        if self.tray.is_none() && !self.tray_off && !self.exiting && self.tray_error.is_none() && self.tray_retry_at.is_none_or(|t| t.elapsed() >= TRAY_REREGISTER) {
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
                    if let Some(u) = e.downcast_ref::<crate::tray::Unsupported>() {
                        // It can't work here: no retries, and the reason shows under the
                        // setting. Without an icon there'd be no way back: × just closes.
                        self.tray_error = Some(u.0.clone());
                    } else {
                        self.tray_retry_at = Some(std::time::Instant::now());
                    }
                }
            }
        }
        // The menu shows the song, with Play/Pause, Next and Previous under it, and
        // hovering the icon shows the song too; they follow the player.
        let playing = self.playing() || self.buffering;
        let song = self.current().map(|t| (t.title.clone(), t.artist_line()));
        if let Some(t) = &mut self.tray {
            t.set_now(song.as_ref().map(|(title, artist)| (title.as_str(), artist.as_str())), playing);
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
                crate::tray::Cmd::Show => self.bring_back(ctx, is_wayland(frame)),
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

    /// The window back for the tray's Show or a later launch. Wayland ignores Visible,
    /// un-minimizing and Focus, so there it can only ask for attention (see `tray_window`).
    pub(super) fn bring_back(&mut self, ctx: &egui::Context, wayland: bool) {
        if wayland {
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Informational));
        } else {
            self.show_window(ctx);
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

/// Whether the window runs under Wayland (it can't be raised or focused there).
pub(super) fn is_wayland(frame: &eframe::Frame) -> bool {
    use raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
    frame.display_handle().is_ok_and(|h| matches!(h.as_raw(), RawDisplayHandle::Wayland(_)))
}
