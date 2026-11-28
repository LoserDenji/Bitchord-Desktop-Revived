//! What clicks and keys ask for: the actions pages queue up, and keyboard shortcuts.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::backend::Req;
use crate::model::{Target, Track};

use super::lists::{self, LIKED_MUSIC};
use super::queue::QueueFill;
use super::undo::Undo;
use super::{Action, App, Dialog, LibTab, NpTab, Page, jam, keybinds, palette, transfer};

impl App {
    pub(super) fn apply(&mut self, a: Action) {
        // In a party only the host drives, the relay turns these down for us: say so
        // instead of clicking into nothing. Playing the current song stays ours (a pause).
        if self.jam.locked() && jam::refused(self, &a) {
            self.toast("Only the host can change the party");
            return;
        }
        // The first result opened after a search is what Recently Searched remembers.
        self.remember_search_click(&a);
        match a {
            // Clicking the song that's already playing pauses/resumes it, not restart it.
            Action::Play(tracks, i) if tracks.get(i).is_some_and(|t| self.current_id() == Some(t.id.as_str())) && self.resume_at.is_none() => {
                self.toggle()
            }
            Action::Play(tracks, i) => {
                // Play buttons and rows live on the page they play from.
                self.queue_source = match &self.page {
                    Page::Album(id) => Some(Target::Album(id.clone())),
                    Page::Playlist(id) => Some(Target::Playlist(id.clone())),
                    Page::Library(LibTab::Songs) => Some(Target::Playlist(LIKED_MUSIC.into())),
                    Page::Artist(id) => Some(Target::Artist(id.clone())),
                    _ => None,
                };
                // All of a playlist that's still coming in, in order or shuffled: the rest follows.
                let fill = self.page_list().filter(|id| self.receiving(id)).and_then(|id| {
                    let o = self.opened(id).filter(|o| o.c.tracks.len() == tracks.len())?;
                    Some(QueueFill { id: id.to_owned(), load: o.load, shuffled: o.c.tracks.iter().zip(&tracks).any(|(a, b)| a.id != b.id), ran_out: false })
                });
                self.play_list_filling(tracks, i, fill);
            }
            Action::TogglePlay => self.toggle(),
            Action::TransferPick(t) => transfer::use_pick(self, t),
            // Kept already: at once. A playlist that isn't starts with its first page.
            Action::PlayCollection(Target::Album(id) | Target::Playlist(id), shuffle) if self.opened(&id).is_some() => self.play_opened(id, shuffle),
            Action::PlayCollection(target, shuffle) => {
                self.toast("Starting playback…");
                self.play_pending = Some((target.clone(), shuffle));
                match target {
                    Target::Playlist(id) => self.load_list(&id, false, false),
                    target => self.backend.request(Req::PlayCollection { target }),
                }
            }
            Action::Open(Target::Song(t)) | Action::Radio(t) if self.explicit_hidden(&t) => {}
            Action::Open(Target::Song(t)) | Action::Radio(t) => {
                self.queue_source = None;
                self.play_list(vec![t.clone()], 0);
                if !self.radio_pending {
                    self.radio_pending = true;
                    // the taste travels with the ask: the song itself plays whatever it
                    // is (the listener chose it), only what lines up around it is ruled on
                    let taste = crate::recommend::TasteLite::of(&self.listening, self.hide_explicit);
                    self.backend.request(Req::Radio { song_of: self.song_of(&t), seed: t.id, taste });
                }
            }
            Action::NotRecommended(t) => {
                self.listening.ban(&t.id);
                self.store_listening();
                self.toast(format!("\"{}\" won't be recommended", t.title));
            }
            Action::Open(Target::Album(id)) => self.go(Page::Album(id)),
            Action::Open(Target::Playlist(id)) => self.go(Page::Playlist(id)),
            Action::Open(Target::Artist(id)) => self.go(Page::Artist(id)),
            Action::PlayNext(t) | Action::Enqueue(t) | Action::QueueInsert { track: t, .. } if self.explicit_hidden(&t) => {}
            Action::PlayNext(t) => {
                let start = self.queue.play_next(t);
                self.toast("Playing next");
                if start {
                    self.play_current();
                } else {
                    self.prefetch_next();
                }
            }
            Action::Enqueue(t) => {
                let start = self.queue.enqueue(t);
                self.toast("Added to queue");
                if start {
                    self.play_current();
                }
            }
            Action::Go(p) => self.go(p),
            Action::QueueJump(i) => {
                self.play_pending = None;
                self.queue.index = Some(i);
                self.queue.drop_explicit();
                self.play_current();
            }
            Action::QueueRemove(i) => {
                if let Some(saved) = self.queue.remove(i) {
                    self.toast_undo("Removed from queue", Undo::Queue(saved));
                    self.prefetch_next();
                }
            }
            Action::QueueClear => {
                if let Some(saved) = self.queue.clear() {
                    self.toast_undo("Queue cleared", Undo::Queue(saved));
                }
            }
            Action::Undo => self.undo(),
            Action::QueueInsert { gap, track } => {
                self.queue.insert_at(gap, track);
                self.toast("Added to queue");
                self.prefetch_next();
            }
            Action::QueueMove { from, gap, id } => {
                if self.queue.tracks.get(from).is_some_and(|t| t.id == id) && self.queue.move_track(from, gap) {
                    self.prefetch_next();
                }
            }
            Action::ViewQueue => {
                if self.np_open {
                    self.np_tab = NpTab::UpNext;
                } else {
                    self.queue_open = true;
                }
            }
            Action::ToggleVideo => {
                self.show_video = !self.show_video;
                // Its song is playing: a video only stays in time with its own audio. A
                // song's music video goes with Video off; Video on leaves a song be.
                if self.show_video == self.current().is_some_and(|t| t.has_video()) {
                    self.play_listed_upload();
                } else if !self.show_video && self.current().is_some_and(|t| t.music_video) && self.play_songs && !self.jam.active() {
                    self.play_song_version();
                }
            }
            // Picked on a song's switch, Video or Song sticks for videos too, as their own
            // switch does.
            Action::SongVideo(video) => {
                self.show_video = video;
                if video {
                    self.play_music_video();
                } else {
                    self.play_listed_upload();
                }
            }
            Action::Seek(secs) => {
                self.seek_to(Duration::from_secs_f32(secs.max(0.0)));
                self.lyric_view.resume();
            }
            Action::Like(t, like) => {
                if !self.logged_in {
                    self.toast("Sign in to like songs");
                    return;
                }
                // Let the current write answer before sending another for the same song.
                if self.ratings.writing(&t.id) || (!like && self.unliking_soon(&t.id)) {
                    return;
                }
                let restored = self.rating(|id| like && id == t.id);
                if !like && self.song_is_liked(&t.id) {
                    let send = Req::Unlike(t.id.clone());
                    return self.unlike_soon(vec![t], send);
                }
                // Keep the original taste order when a re-like undid the pending unlike.
                if !restored {
                    self.set_liked(std::slice::from_ref(&t), like);
                }
                self.send_rating_edit(Req::Like { video_id: t.id, like });
            }
            Action::LikeMany(tracks, like) => {
                if !self.logged_in {
                    self.toast("Sign in to like songs");
                    return;
                }
                if like {
                    self.rating(|id| tracks.iter().any(|t| t.id == id));
                }
                let tracks: Vec<Track> = tracks.into_iter().filter(|t| !self.ratings.writing(&t.id) && self.song_is_liked(&t.id) != like).collect();
                if tracks.is_empty() { return; }
                if !like {
                    let send = Req::LikeMany { video_ids: tracks.iter().map(|t| t.id.clone()).collect(), like };
                    return self.unlike_soon(tracks, send);
                }
                self.set_liked(&tracks, true);
                if tracks.len() > 10 {
                    // One at a time, then checked: a few hundred take a few minutes.
                    self.toast(format!("Liking {} songs…", tracks.len()));
                }
                self.send_rating_edit(Req::LikeMany { video_ids: tracks.into_iter().map(|t| t.id).collect(), like });
            }
            Action::Share(target) => {
                self.share_pending = Some(share_url(&target));
                self.toast("Link copied");
            }
            Action::Pin(id, on) => {
                self.pinned.retain(|p| *p != id);
                if on {
                    self.pinned.insert(0, id);
                }
            }
            Action::Download(track) => {
                self.toast(format!("Downloading \u{201c}{}\u{201d}", track.title));
                // The song version playing for a music video: that video plays the file too.
                let for_video = if self.playing_as().is_some_and(|s| s.id == track.id && !s.has_video()) { self.current_id().map(str::to_owned) } else { None };
                self.backend.request(Req::Download { track, for_video });
            }
            Action::DownloadMany(target, title, tracks) => {
                self.backend.request(Req::DownloadMany { target, title, tracks, confirmed: false });
            }
            Action::CancelDownload(key) => self.backend.cancel_downloads(&key),
            Action::AddToPlaylist(tracks) => {
                if self.logged_in { self.dialog = Some(Dialog::PickPlaylist(tracks)); }
                else { self.toast("Sign in to edit playlists"); }
            }
            Action::RemoveFromPlaylist(mut rows) => {
                let Some(c) = self.current_collection().filter(|c| c.owned && !c.is_album) else { return };
                rows.sort_unstable();
                rows.dedup();
                rows.retain(|&i| c.set_ids.get(i).is_some_and(Option::is_some));
                if rows.is_empty() {
                    self.toast("YouTube Music didn't say where these songs are in the playlist. Reload it and try again.");
                    return;
                }
                let items: Vec<(String, String)> = rows.iter().map(|&i| (c.tracks[i].id.clone(), c.set_ids[i].clone().unwrap_or_default())).collect();
                let playlist = c.id.clone();
                let taken = lists::taken(c, &rows);
                // Optimistic: drop the rows now. YouTube Music is told once the Undo is gone,
                // and the playlist reloads when it confirms.
                c.remove_rows(&rows);
                log::debug!("list {playlist}: removing {} entries", items.len());
                self.lists.edits.remove(&playlist, items.iter().map(|(_, set)| set.clone()));
                self.selection.clear();
                self.toast_undo("Removed from playlist", Undo::Removal { id: playlist, rows: taken });
            }
            Action::NewPlaylist(tracks) => {
                if self.logged_in { self.dialog = Some(Dialog::NewPlaylist { title: String::new(), tracks }); }
                else { self.toast("Sign in to create playlists"); }
            }
            Action::EditPlaylist => {
                if let Some(c) = self.current_collection().filter(|c| !c.is_album) {
                    self.dialog = Some(Dialog::EditPlaylist {
                        id: c.id.clone(), title: c.title.clone(), description: c.description.clone().unwrap_or_default(), privacy: None,
                    });
                }
            }
            Action::SortPlaylist(id, actions) => {
                self.sorting.insert(id.clone(), super::lists::SortWait::new(actions.clone()));
                self.backend.request(Req::SortPlaylist { id, actions });
            }
            Action::Collaborate => {
                if let Some(id) = self.current_collection().filter(|c| !c.is_album && c.owned).map(|c| c.id.clone()) {
                    self.backend.request(Req::Collaboration(id.clone()));
                    self.dialog = Some(Dialog::Collaborate { id, info: None, busy: true, confirm_off: false });
                }
            }
            Action::DeletePlaylist => {
                if let Some(c) = self.current_collection().filter(|c| !c.is_album) {
                    self.dialog = Some(Dialog::ConfirmDelete { id: c.id.clone(), title: c.title.clone() });
                }
            }
            Action::SaveCollection(id, save) => {
                if self.logged_in { self.backend.request(Req::SaveCollection { id, save }); }
                else { self.toast("Sign in to save to library"); }
            }
            Action::Subscribe(on) => {
                if !self.logged_in {
                    self.toast("Sign in to subscribe");
                } else if let Some(a) = self.artist.as_mut().filter(|a| a.subscribed != on)
                    && let Some(channel_id) = a.channel_id.clone()
                    // One at a time, and not queued: the pill is greyed out while one runs.
                    && self.subscriptions.start(&channel_id, on)
                {
                    // Shown at once; put back if YouTube Music refuses it.
                    a.subscribed = on;
                    self.backend.request(Req::Subscribe { channel_id, on });
                }
            }
            Action::OpenNowPlaying(open) => {
                if self.current().is_some() {
                    self.np_open = open;
                }
            }
            Action::Mini(on) => self.mini.request(on),
            Action::RevertQuality => {
                // Mark the playing stream's id (a song version differs from the listed id)
                // so resolve skips hi-res for it, then replay at the same position.
                let listed = self.current_id().map(str::to_owned);
                let playing = self.playing_track().map(|t| t.id.clone()).or(listed.clone());
                let Some(id) = playing else { return };
                let at = self.audio.status.position();
                self.backend.revert_quality(&id);
                if let Some(listed) = listed.filter(|l| l != &id) {
                    self.backend.revert_quality(&listed);
                }
                self.play_current_at(at, false);
            }
            Action::UpgradeQuality => {
                let listed = self.current_id().map(str::to_owned);
                let playing = self.playing_track().map(|t| t.id.clone()).or(listed.clone());
                let Some(id) = playing else { return };
                self.backend.upgrade_quality(&id);
                if let Some(listed) = listed.filter(|l| l != &id) {
                    self.backend.upgrade_quality(&listed);
                }
                // An upgrade can come from any music source that's on, not just the
                // custom addon URLs: Monochrome servers the listener added, Monochrome's
                // own streamer, or JioSaavn all take over in place the same way. The
                // Addons row counts only when it's on and has at least one enabled link.
                let addons_ready = self.addons_on && self.hires_sources.iter().enumerate().any(|(i, s)| {
                    !s.trim().is_empty() && self.hires_enabled.get(i).copied().unwrap_or(true)
                });
                let any_upgrade_source = addons_ready || self.mono_servers_on || self.monochrome || self.jiosaavn;
                if !any_upgrade_source {
                    self.toast("No music source is set to upgrade from. Add one in Settings \u{203a} Music sources first.");
                } else if self.audio.status.wanted_gen.load(std::sync::atomic::Ordering::Relaxed) == self.generation {
                    // Ask for the stream that is playing, in place — whatever tier it
                    // holds: the backend swaps only for a strictly finer answer, so this
                    // is also how a Lossless copy chases a Hi-Res one. The backend
                    // reports the attempt either way, so nothing here claims success on
                    // its own.
                    self.upgrade_asked = true;
                    self.backend.try_upgrade_active(self.generation, self.playing_track().cloned());
                } else {
                    let at = self.audio.status.position();
                    self.play_current_at(at, false);
                }
            }
        }
    }

    pub(super) fn shortcuts(&mut self, ctx: &egui::Context) {
        // Picking a combination in Settings: the dialog takes every key, and the
        // player's own leave it alone until it's done.
        if self.keybinds.capture.is_some() {
            keybinds::capture_dialog(self, ctx);
            return;
        }
        palette::shortcut(self, ctx);
        let keys = &self.keybinds.bindings;
        use keybinds::Cmd;
        let (space, back, fwd, next, prev, seek_bwd, seek_fwd, lyr, queue, np, full, search, vol_up, vol_down, mute, mini) = (
            keys.matches(Cmd::TogglePlay, ctx),
            keys.matches(Cmd::Back, ctx) || ctx.input(|i| i.pointer.button_pressed(egui::PointerButton::Extra1)),
            keys.matches(Cmd::Forward, ctx) || ctx.input(|i| i.pointer.button_pressed(egui::PointerButton::Extra2)),
            keys.matches(Cmd::Next, ctx),
            keys.matches(Cmd::Prev, ctx),
            keys.matches(Cmd::SeekBackward, ctx),
            keys.matches(Cmd::SeekForward, ctx),
            // Once per press, like the toggles below: L and Q flip their panel, and
            // holding the key doesn't flip back and forth.
            keys.matches_once(Cmd::Lyrics, ctx),
            keys.matches_once(Cmd::Queue, ctx),
            keys.matches(Cmd::NowPlaying, ctx),
            keys.matches(Cmd::FullScreen, ctx) || ctx.input(|i| i.key_pressed(egui::Key::F11)),
            keys.matches(Cmd::Search, ctx),
            keys.matches(Cmd::VolumeUp, ctx),
            keys.matches(Cmd::VolumeDown, ctx),
            // Once per press: holding it doesn't flip back and forth.
            keys.matches_once(Cmd::Mute, ctx),
            keys.matches_once(Cmd::MiniPlayer, ctx),
        );
        let esc = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        // Quits for real, with Close to tray on too: GNOME has no tray to quit from (#143).
        let quit = ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Q));
        if quit {
            self.quit(ctx);
        }
        // The toast's Undo. With Shift it would be redo, which there isn't.
        let undo = !ctx.text_edit_focused() && ctx.input(|i| i.modifiers.command && !i.modifiers.shift && i.key_pressed(egui::Key::Z));
        if undo && self.undo.is_some() {
            self.actions.push(Action::Undo);
        }
        if mini {
            self.actions.push(Action::Mini(!self.mini.target()));
        }
        // The mini player has no pages or now playing to go to, nor does a window on
        // its way back until it's there.
        let (back, fwd, lyr, queue, np, full, search, esc) = if self.mini.busy() { (false, false, false, false, false, false, false, false) } else { (back, fwd, lyr, queue, np, full, search, esc) };
        if space {
            self.toggle();
        }
        if back {
            self.nav_back();
        }
        if fwd {
            self.nav_forward();
        }
        if next {
            self.next();
        }
        if prev {
            self.prev();
        }
        // L and Q toggle, the trip the comments menu item makes: with their panel
        // already up they fold it back to the artwork instead of reopening it.
        if lyr && self.current().is_some() {
            self.toggle_lyrics();
        }
        // Q makes the trip the player bar's queue button makes: in now playing the
        // Up next panel, elsewhere the sidebar's queue — outside the view it used to
        // drag the whole now playing up for the queue alone.
        if queue {
            self.toggle_queue();
        }
        // F opens and closes the now playing view; Shift+F is the player full screen
        // (opening now playing first when it isn't up, since the full screen is its view).
        if np && self.current().is_some() {
            self.np_open = !self.np_open;
            if self.fullscreen && !self.np_open {
                self.set_fullscreen(ctx, false);
            }
        }
        if full && self.current().is_some() {
            self.toggle_fullscreen(ctx);
        }
        if search {
            // From any page: the Search page opens, and its box takes the typing. (On
            // the search page itself this just refocuses the box.)
            if !matches!(self.page, Page::Search(_)) {
                let q = self.search_text.trim().to_owned();
                self.go(Page::Search(q));
            }
            self.search_refocus = true;
        }
        if esc {
            // A drag being let go with Esc is just that: the drag ends, nothing closes.
            if self.row_drag.take().is_some() {
            } else if self.fullscreen {
                self.set_fullscreen(ctx, false);
            } else {
                self.np_open = false;
            }
        }
        // Seek steps five seconds either way, and holding the key keeps stepping, a
        // key's repeats counting as presses. Keys a text box or slider wants stay with
        // it (`matches` leaves those alone), and without a song there's nothing to seek.
        if self.current().is_some() && (seek_bwd || seek_fwd) {
            let at = self.audio.status.position().as_secs_f32() + if seek_fwd { 5.0 } else { -5.0 };
            self.seek_to(Duration::from_secs_f32(at.max(0.0)));
        }
        if vol_up || vol_down {
            let v = (self.audio.volume() + if vol_up { 0.05 } else { -0.05 }).clamp(0.0, 1.0);
            self.audio.set_volume(v);
        }
        // M, like the player bar's speaker: the volume it was at comes back on unmute.
        if mute {
            super::shell::toggle_mute(self, ctx);
        }
    }

    /// Now playing with its lyrics, or back out of it (L): with the panel already up
    /// it folds back to the artwork instead of reopening it.
    pub(super) fn toggle_lyrics(&mut self) {
        if self.current().is_some() {
            if self.np_open && self.np_tab == NpTab::Lyrics {
                self.np_tab = NpTab::Art;
            } else {
                self.np_open = true;
                self.np_tab = NpTab::Lyrics;
            }
        }
    }

    /// Full screen in now playing, or out of it (Shift+F): opening now playing first
    /// when it isn't up, since the full screen is its view. With a video on show, full
    /// screen is the video's, as F is on YouTube.
    pub(super) fn toggle_fullscreen(&mut self, ctx: &egui::Context) {
        if !self.np_open {
            self.np_open = true;
            self.set_fullscreen(ctx, true);
        } else if self.fullscreen {
            self.set_fullscreen(ctx, false);
        } else if self.shown_video().is_some() {
            self.set_video_fullscreen(ctx, true);
        } else {
            self.set_fullscreen(ctx, true);
        }
    }

    /// In now playing the queue is its Up next panel; elsewhere, the side panel.
    pub(super) fn queue_shown(&self) -> bool {
        if self.np_open { self.np_tab == NpTab::UpNext } else { self.queue_open }
    }

    /// The player bar's queue button: in now playing its Up next panel, elsewhere the
    /// side drawer.
    pub(super) fn toggle_queue(&mut self) {
        if self.np_open {
            self.np_tab = if self.queue_shown() { NpTab::Art } else { NpTab::UpNext };
        } else {
            self.queue_open = !self.queue_open;
        }
    }

    /// Into the mini player or back. Full screen ends first, a frame ahead.
    pub(super) fn set_mini(&mut self, ctx: &egui::Context, on: bool, now: f64) {
        if on == self.mini.active() {
            return;
        }
        if on && self.fullscreen {
            self.set_fullscreen(ctx, false);
            self.mini.request(true);
            return;
        }
        // With a video on show, the mini player is that video (picture in picture).
        let video = on && self.np_open && self.current().is_some_and(|t| self.video_mode(t));
        self.mini.toggle(ctx, now, video);
    }

    /// Full screen with just the video, as on YouTube. Leaving it is leaving full screen.
    pub(super) fn set_video_fullscreen(&mut self, ctx: &egui::Context, on: bool) {
        self.set_fullscreen(ctx, on);
        self.video_full = on;
        super::nowplaying::wake_video_controls(ctx);
    }

    pub(super) fn set_fullscreen(&mut self, ctx: &egui::Context, on: bool) {
        self.fullscreen = on;
        self.video_full = false;
        // Windows: in one step, without winit's unthemed-frame flash.
        if !crate::fullscreen::set(on) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(on));
        }
        ctx.request_repaint();
    }
}

fn share_url(target: &Target) -> String {
    match target {
        Target::Song(t) => format!("https://music.youtube.com/watch?v={}", t.id),
        Target::Playlist(id) => format!("https://music.youtube.com/playlist?list={}", crate::ytm::list_id(id)),
        Target::Album(id) => format!("https://music.youtube.com/browse/{id}"),
        Target::Artist(id) => format!("https://music.youtube.com/channel/{id}"),
    }
}

/// Subscribing and unsubscribing from artist pages: one at a time, and what each settled,
/// so an artist page asked for before a change can't undo it.
#[derive(Default)]
pub(super) struct Subscriptions {
    /// The one under way: its channel, and whether it subscribes.
    running: Option<(String, bool)>,
    /// Each channel's last change: where it stands, and when YouTube Music answered.
    settled: HashMap<String, (bool, Instant)>,
}

impl Subscriptions {
    pub(super) fn busy(&self) -> bool {
        self.running.is_some()
    }

    /// Starts one, unless another is under way.
    pub(super) fn start(&mut self, channel: &str, on: bool) -> bool {
        if self.busy() {
            return false;
        }
        self.running = Some((channel.to_owned(), on));
        true
    }

    /// Its answer, which lets the next one go: where the channel now stands.
    pub(super) fn finish(&mut self, channel: &str, on: bool, ok: bool) -> bool {
        self.running = None;
        let now = on == ok;
        self.settled.insert(channel.to_owned(), (now, Instant::now()));
        now
    }

    /// Whether an artist page asked for at `asked` shows the channel subscribed: as the
    /// page says, unless a change under way, or answered since, says otherwise.
    pub(super) fn shown(&self, channel: &str, says: bool, asked: Instant) -> bool {
        match (&self.running, self.settled.get(channel)) {
            (Some((c, on)), _) if c == channel => *on,
            (_, Some(&(on, at))) if asked < at => on,
            _ => says,
        }
    }
}

#[cfg(test)]
mod subscription_tests {
    use super::*;

    #[test]
    fn one_at_a_time() {
        let mut s = Subscriptions::default();
        assert!(s.start("UCa", true));
        assert!(s.busy());
        assert!(!s.start("UCa", false), "a click while one runs does nothing");
        assert!(!s.start("UCb", true));
        assert!(s.finish("UCa", true, true));
        assert!(!s.busy());
        assert!(s.start("UCb", true));
        // A failure lets the next one go too, and puts the old state back.
        assert!(!s.finish("UCb", true, false));
        assert!(!s.busy());
    }

    #[test]
    fn earlier_pages_dont_undo_a_change() {
        let mut s = Subscriptions::default();
        let before = Instant::now().checked_sub(Duration::from_secs(1)).unwrap();
        assert!(s.shown("UCa", true, before), "nothing changed here: the page says");
        s.start("UCa", true);
        // Under way: what was asked, whenever the page was fetched.
        assert!(s.shown("UCa", false, before));
        assert!(s.shown("UCa", false, Instant::now()));
        assert!(!s.shown("UCb", false, before), "another channel: the page says");
        s.finish("UCa", true, true);
        assert!(s.shown("UCa", false, before), "asked before the answer: the answer stands");
        let after = Instant::now() + Duration::from_secs(1);
        assert!(!s.shown("UCa", false, after), "asked after it: the page says");
        // A refused unsubscribe leaves it subscribed, over a page from before it.
        s.start("UCa", false);
        s.finish("UCa", false, false);
        assert!(s.shown("UCa", false, before));
    }
}

#[cfg(test)]
mod share_tests {
    use super::*;

    #[test]
    fn shares_youtube_music_urls() {
        let song = Track { id: "abc123".into(), title: String::new(), artists: vec![], album: None, duration: None, thumbs: vec![], track_nr: None, plays: None, ..Default::default() };
        assert_eq!(share_url(&Target::Song(song)), "https://music.youtube.com/watch?v=abc123");
        assert_eq!(share_url(&Target::Playlist("PL123".into())), "https://music.youtube.com/playlist?list=PL123");
        // A podcast show is shared as its playlist.
        assert_eq!(share_url(&Target::Playlist("MPSPPL123".into())), "https://music.youtube.com/playlist?list=PL123");
        assert_eq!(share_url(&Target::Album("MPRE123".into())), "https://music.youtube.com/browse/MPRE123");
        assert_eq!(share_url(&Target::Artist("UC123".into())), "https://music.youtube.com/channel/UC123");
    }
}
