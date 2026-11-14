//! What clicks and keys ask for: the actions pages queue up, and keyboard shortcuts.

use std::time::Duration;

use crate::backend::Req;
use crate::model::{Target, Track};

use super::lists::LIKED_MUSIC;
use super::queue::QueueFill;
use super::{Action, App, Dialog, LibTab, NpTab, Page, jam, keybinds, transfer};

impl App {
    pub(super) fn apply(&mut self, a: Action) {
        // In someone else's jam, playback changes become requests to the host.
        let a = if self.jam.guest() {
            match jam::guest_action(self, a) {
                Some(a) => a,
                None => return,
            }
        } else {
            a
        };
        // In a locked party, pause is the listener's own; a resume rejoins the party.
        let a = match super::party::locked_action(self, a) {
            Some(a) => a,
            None => return,
        };
        // Whatever the party should hear about went through: it goes out shortly,
        // once the player has settled into the shape the action asked for.
        if matches!(
            a,
            Action::Play(..)
                | Action::PlayCollection(..)
                | Action::PlayNext(_)
                | Action::Enqueue(_)
                | Action::Radio(_)
                | Action::QueueJump(_)
                | Action::QueueRemove(_)
                | Action::QueueMove(..)
                | Action::Seek(_)
                | Action::TogglePlay
        ) {
            super::party::on_intent(self);
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
                    self.backend.request(Req::Radio { song_of: self.song_of(&t), seed: t.id });
                }
            }
            Action::Open(Target::Album(id)) => self.go(Page::Album(id)),
            Action::Open(Target::Playlist(id)) => self.go(Page::Playlist(id)),
            Action::Open(Target::Artist(id)) => self.go(Page::Artist(id)),
            Action::PlayNext(t) | Action::Enqueue(t) if self.explicit_hidden(&t) => {}
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
            Action::QueueRemove(i) => self.queue.remove(i),
            Action::QueueMove(from, to) => self.queue.move_track(from, to),
            Action::ViewQueue => {
                if self.np_open {
                    self.np_tab = NpTab::UpNext;
                } else {
                    self.queue_open = true;
                }
            }
            Action::ToggleVideo => {
                self.show_video = !self.show_video;
                // Its song is playing: a video only stays in time with its own audio.
                if self.show_video {
                    self.play_listed_upload();
                } else if self.current().is_some_and(|t| t.music_video) && self.play_songs && !self.jam.active() {
                    self.play_song_version();
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
                // Optimistic: update the like state and the cached Liked songs list now.
                self.set_liked(std::slice::from_ref(&t), like);
                self.backend.request(Req::Like { video_id: t.id, like });
            }
            Action::LikeMany(tracks, like) => {
                if !self.logged_in {
                    self.toast("Sign in to like songs");
                    return;
                }
                let tracks: Vec<Track> = tracks.into_iter().filter(|t| self.liked.contains(&t.id) != like).collect();
                if tracks.is_empty() { return; }
                if like {
                    self.set_liked(&tracks, true);
                } else {
                    // Unlikes go a few a second (YouTube Music drops faster ones): each song
                    // leaves the list when its unlike goes through, not all at once.
                    self.unliking.extend(tracks.iter().map(|t| (t.id.clone(), t.clone())));
                }
                if tracks.len() > 10 {
                    // One at a time, then checked: a few hundred take a few minutes.
                    self.toast(format!("{} {} songs…", if like { "Liking" } else { "Removing likes from" }, tracks.len()));
                }
                self.backend.request(Req::LikeMany { video_ids: tracks.into_iter().map(|t| t.id).collect(), like });
            }
            Action::Dislike(t) => {
                if !self.logged_in { self.toast("Sign in to rate songs"); return; }
                self.set_liked(std::slice::from_ref(&t), false);
                self.backend.request(Req::Dislike(t.id));
            }
            Action::Share(target) => {
                self.share_pending = Some(share_url(&target));
                self.toast("Link copied");
            }
            Action::Download(track) => {
                self.toast(format!("Downloading \u{201c}{}\u{201d}", track.title));
                // The song version playing for a music video: that video plays the file too.
                let for_video = if self.playing_as().is_some_and(|s| s.id == track.id) { self.current_id().map(str::to_owned) } else { None };
                self.backend.request(Req::Download { track, for_video });
            }
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
                // Optimistic: drop the rows now; the playlist reloads once YouTube confirms.
                c.remove_rows(&rows);
                log::debug!("list {playlist}: removing {} entries", items.len());
                self.lists.edits.remove(&playlist, items.iter().map(|(_, set)| set.clone()));
                self.selection.clear();
                self.backend.request(Req::RemoveFromPlaylist { playlist_id: playlist, items });
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
            Action::HomeChip(params) => {
                self.home_loading = true;
                if let Some(h) = &mut self.home {
                    h.shelves.clear();
                    for c in &mut h.chips {
                        c.selected = false;
                    }
                }
                self.backend.request(Req::Home(params));
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
                let at = self.audio.status.position();
                self.backend.upgrade_quality(&id);
                if let Some(listed) = listed.filter(|l| l != &id) {
                    self.backend.upgrade_quality(&listed);
                }
                self.play_current_at(at, false);
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
        let keys = &self.keybinds.bindings;
        use keybinds::Cmd;
        let (space, back, fwd, next, prev, seek_bwd, seek_fwd, lyr, queue, np, full, search, vol_up, vol_down, mini) = (
            keys.matches(Cmd::TogglePlay, ctx),
            keys.matches(Cmd::Back, ctx) || ctx.input(|i| i.pointer.button_pressed(egui::PointerButton::Extra1)),
            keys.matches(Cmd::Forward, ctx) || ctx.input(|i| i.pointer.button_pressed(egui::PointerButton::Extra2)),
            keys.matches(Cmd::Next, ctx),
            keys.matches(Cmd::Prev, ctx),
            keys.matches(Cmd::SeekBackward, ctx),
            keys.matches(Cmd::SeekForward, ctx),
            keys.matches(Cmd::Lyrics, ctx),
            keys.matches(Cmd::Queue, ctx),
            keys.matches(Cmd::NowPlaying, ctx),
            keys.matches(Cmd::FullScreen, ctx),
            keys.matches(Cmd::Search, ctx),
            keys.matches(Cmd::VolumeUp, ctx),
            keys.matches(Cmd::VolumeDown, ctx),
            // Once per press: holding it doesn't flip back and forth.
            keys.matches_once(Cmd::MiniPlayer, ctx),
        );
        let esc = ctx.input(|i| i.key_pressed(egui::Key::Escape));
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
        if lyr && self.current().is_some() {
            self.np_open = true;
            self.np_tab = NpTab::Lyrics;
        }
        if queue && self.current().is_some() {
            self.np_open = true;
            self.np_tab = NpTab::UpNext;
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
            if !self.np_open {
                self.np_open = true;
                self.set_fullscreen(ctx, true);
            } else {
                self.set_fullscreen(ctx, !self.fullscreen);
            }
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
        self.mini.toggle(ctx, now);
    }

    pub(super) fn set_fullscreen(&mut self, ctx: &egui::Context, on: bool) {
        self.fullscreen = on;
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
