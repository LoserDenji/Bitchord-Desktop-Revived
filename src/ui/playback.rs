//! Playing the queue: starting songs, Next and Back, pausing and seeking, picking up
//! after a dropped download, SponsorBlock, the art and lyrics that follow a song, and
//! the session kept across launches and devices.

use std::sync::atomic::Ordering;
use std::time::Duration;

use crate::audio::AudioCmd;
use crate::backend::{self, Req};
use crate::lyrics::Lyrics;
use crate::model::Track;

use super::queue::QueueFill;
use super::{App, MixFrom, PlayingAs, Repeat, Session, Watched, lyric_prefs};

/// Where in the blend the two songs trade places: the outgoing one holds its level for
/// the first 30% (see `audio::mix_handoff`) and the curves are mirrors, so the next song
/// becomes the louder of the two at 65% of the mix. That's when the player follows it.
const MIX_SWITCH: f32 = 0.65;

impl App {
    pub fn current(&self) -> Option<&Track> {
        self.queue.current()
    }

    /// The lyrics shown for the current song: the song's pick, then the listener's order.
    fn lyric_choice(&self) -> Option<&Lyrics> {
        let (id, set) = self.lyrics.as_ref()?;
        set.get(self.lyric_prefs.choose(id, set)?)
    }

    /// Lyrics have loaded, and no enabled source has any.
    pub(super) fn no_lyrics(&self) -> bool {
        self.lyrics.is_some() && self.lyric_choice().is_none()
    }

    /// Ask for romanization/translation of the shown source once, when they're switched on
    /// and some line needs one the lyrics don't ship.
    pub(super) fn request_lyric_extras(&mut self) {
        if !(self.show_roman || self.show_trans) {
            return;
        }
        let Some(l) = self.lyric_choice() else { return };
        let Some((id, _)) = &self.lyrics else { return };
        let target = self.lyric_prefs.target.clone();
        let want = (id.clone(), l.provider.key().to_owned(), target.clone());
        if self.extras_asked.as_ref() == Some(&want) {
            return;
        }
        let needs = l.lines.iter().any(|x| {
            !x.instrumental
                && !x.text.trim().is_empty()
                && ((self.show_roman && x.romanization.is_none() && crate::translate::non_latin(&x.text))
                    || (self.show_trans && lyric_prefs::shipped_translation(x, &target).is_none()))
        });
        let req = needs.then(|| Req::LyricsExtras {
            video_id: id.clone(),
            source: l.provider.key().to_owned(),
            lines: l.lines.iter().map(|x| if x.instrumental { String::new() } else { x.text.clone() }).collect(),
            lang: l.language.clone(),
            target,
        });
        self.extras_asked = Some(want);
        if let Some(r) = req {
            self.backend.request(r);
        }
    }

    pub(super) fn current_id(&self) -> Option<&str> {
        self.current().map(|t| t.id.as_str())
    }

    /// The song version standing in for the current music video. Only for the play it
    /// came with: a new play, or a queue that changed under it, leaves it behind.
    pub(super) fn playing_as(&self) -> Option<&Track> {
        self.playing_as.as_ref().filter(|p| p.generation == self.generation && self.current_id() == Some(p.listed.as_str())).map(|p| &p.song)
    }

    /// What actually plays: the current track, or the song version standing in for it.
    /// Lyrics, SponsorBlock, Last.fm, Discord, media controls and Now playing's actions go
    /// by this (timing, length and title are the upload's own); lists and the queue keep
    /// the track as listed.
    pub(super) fn playing_track(&self) -> Option<&Track> {
        self.playing_as().or(self.current())
    }

    pub(super) fn playing_id(&self) -> Option<&str> {
        self.playing_track().map(|t| t.id.as_str())
    }

    /// The outgoing song of an Automix handoff, while it is still the louder of the
    /// two: its tail keeps the player until the next song has taken over.
    pub(super) fn mix_hold(&self) -> Option<&MixFrom> {
        self.mix_from.as_ref().filter(|h| !h.released)
    }

    /// The blend is under way: the handoff exists until both of its halves have played.
    pub(super) fn mixing(&self) -> bool {
        self.mix_from.as_ref().is_some_and(|h| h.started && self.audio.status.mixing())
    }

    /// That song's progress, in seconds: its tail runs out as the mix goes on.
    pub(super) fn mix_hold_pos(&self, h: &MixFrom) -> f32 {
        self.audio.status.outgoing_position().as_secs_f32().min(h.total)
    }

    /// Note where the handoff has got to; true on the frame the next song becomes the
    /// louder one and takes the screen over (see `MIX_SWITCH`).
    pub(super) fn watch_mix(&mut self) -> bool {
        let pos = self.audio.status.position().as_secs_f32();
        let Some(h) = &mut self.mix_from else { return false };
        if let Some(duration) = self.audio.status.mix_duration(self.generation) {
            h.started = true;
            h.mix = duration;
        }
        let switch = h.started && !h.released && pos >= h.mix * MIX_SWITCH;
        if switch {
            h.released = true;
        }
        switch
    }

    /// The handoff's whole blend has played: the transition is over.
    pub(super) fn mix_over(&self) -> bool {
        self.mix_from.as_ref().is_some_and(|h| h.started && !self.audio.status.mixing())
    }

    /// The song the player shows: a handoff's outgoing one while it's still the louder,
    /// then whatever actually plays.
    pub(super) fn shown(&self) -> Option<&Track> {
        self.mix_hold().map(|h| &h.track).or_else(|| self.playing_track())
    }

    /// The next song has taken the blend over: its art, lyrics and media info follow.
    pub(super) fn release_mix(&mut self) {
        if let Some(track) = self.playing_track().cloned() {
            self.show_in_media(&track);
            self.request_art();
            self.refresh_lyrics(track);
        }
    }

    /// The song of what's playing, for scrobbles: the song version standing in for a
    /// music video, or the song a music video stands in for.
    pub(super) fn heard_song(&self) -> Option<&Track> {
        self.playing_as().filter(|t| !t.has_video()).or(self.current())
    }

    /// The video on show, when one is: the music video playing, or the one picked on a
    /// song's switch. Lyrics are timed to the song, so they don't go with it.
    pub(super) fn shown_video(&self) -> Option<&Track> {
        let t = self.playing_track().filter(|t| t.has_video() && !self.jam.active())?;
        (self.show_video || self.playing_as().is_some()).then_some(t)
    }

    /// What the Song/Video switch shows picked: on a song, whether its music video plays
    /// in its place; on a video, the Video preference.
    pub(super) fn video_picked(&self) -> bool {
        match self.current() {
            Some(t) if !t.has_video() => self.playing_as().is_some(),
            _ => self.show_video,
        }
    }

    /// The current song's music video, for its Song/Video switch. Asked for the first
    /// time it's wanted, once per song.
    pub(super) fn song_video(&mut self) -> Option<&crate::ytm::MusicVideo> {
        let t = self.current().filter(|t| !t.has_video() && !self.jam.active())?;
        if !self.music_videos.contains_key(&t.id) {
            let t = t.clone();
            if self.music_videos.len() >= 1000 {
                self.music_videos.clear();
            }
            self.music_videos.insert(t.id.clone(), None);
            self.backend.request(Req::MusicVideo(t));
            return None;
        }
        let found = self.music_videos.get(&t.id)?.as_ref()?;
        (!(self.hide_explicit && found.video.explicit)).then_some(found)
    }

    /// How far the music video playing for the current song runs ahead of the song.
    fn video_offset(&self) -> i64 {
        let (Some(song), Some(video)) = (self.current(), self.playing_as()) else { return 0 };
        self.music_videos.get(&song.id).and_then(Option::as_ref).filter(|m| m.video.id == video.id).map_or(0, |m| m.offset_ms)
    }

    /// Play the current music video as itself again, from where its song is now. Not in
    /// a party: the switch stays out of rooms, whose songs each device plays its own way.
    pub(super) fn play_listed_upload(&mut self) {
        if self.playing_as().is_none() {
            return;
        }
        let at = shifted(self.audio.status.position(), -self.video_offset());
        self.playing_as = None;
        if self.playing() || self.buffering {
            self.play_current_at(at, false);
        } else {
            self.resume_at = Some(at);
        }
    }

    /// From a song to its music video, at the same place in the song: the video stands
    /// in for it as a song version does for a video, so it's the same play.
    pub(super) fn play_music_video(&mut self) {
        if self.playing_as().is_some() {
            return;
        }
        let Some(found) = self.song_video().cloned() else { return };
        let Some(listed) = self.current_id().map(str::to_owned) else { return };
        let at = shifted(self.audio.status.position(), found.offset_ms);
        self.playing_as = Some(PlayingAs { generation: self.generation, listed, song: found.video });
        if self.playing() || self.buffering {
            self.start_current(at, false, true);
        } else {
            self.resume_at = Some(at);
        }
    }

    /// `t`, when it's a music video to play as its song version. A party shares song ids,
    /// not streams: each device plays the best copy of the song it has.
    pub(super) fn song_of(&self, t: &Track) -> Option<Track> {
        (self.play_songs && !self.show_video && t.music_video).then(|| t.clone())
    }

    pub(super) fn playing(&self) -> bool {
        self.audio.status.playing.load(Ordering::Relaxed)
    }

    /// Watch what each song actually gets of listening, for the events and skips
    /// Home grows from (see `listening`). The watched song is the listed one, so the
    /// Song/Video switch mid-song doesn't split it into two plays; the seconds count
    /// only while the player runs, and settle the moment the song stops being current.
    pub(super) fn watch_listening(&mut self, now: f64) {
        let dt = (now - self.watched_frame).max(0.0) as f32;
        self.watched_frame = now;
        let current = self.current().cloned();
        let same = self.watched.as_ref().is_some_and(|w| current.as_ref().is_some_and(|c| c.id == w.track.id));
        if same {
            if self.playing() {
                let w = self.watched.as_mut().unwrap();
                // the event's timestamp is when the song began, not when it was queued
                w.started.get_or_insert_with(crate::listening::unix_now);
                w.played += dt;
            }
            return;
        }
        if self.watched.is_some() {
            self.settle_listening();
            self.store_listening();
        }
        if let Some(c) = current {
            self.watched = Some(Watched { track: c, started: None, played: 0.0 });
        }
    }

    pub(super) fn settle_listening(&mut self) {
        if let Some(w) = self.watched.take() {
            self.listening.note_left(w.track, w.started.unwrap_or_else(crate::listening::unix_now), w.played);
        }
    }

    /// Fetch the current song's art for the background and accent. A local song's
    /// cover is a file the backend reads, not a fetch (see `art_uri`).
    pub(super) fn request_art(&mut self) {
        let Some(track) = self.current() else { return };
        if self.art_key.as_deref() == Some(track.id.as_str()) {
            return;
        }
        if let Some(url) = backend::art_uri(track) {
            let key = track.id.clone();
            self.backend.request(Req::Art { key, url });
        }
    }

    pub(super) fn play_list(&mut self, tracks: Vec<Track>, start: usize) {
        self.play_list_filling(tracks, start, None);
    }

    /// `fill`: the queue is a playlist still coming in, and its other songs follow.
    pub(super) fn play_list_filling(&mut self, tracks: Vec<Track>, start: usize, fill: Option<QueueFill>) {
        // A song opened from a link, say: the next one that isn't explicit plays, if any.
        let hidden = tracks.get(start).is_some_and(|t| self.hide_explicit && t.explicit);
        let played = self.queue.replace(tracks, start, fill);
        if hidden {
            self.toast(if played { "Skipped an explicit song" } else { "Nothing to play that isn't explicit" });
        }
        if !played {
            return;
        }
        // Whatever was about to play doesn't.
        self.play_pending = None;
        self.play_current();
    }

    /// Put last session's queue back, paused at the saved position, without loading audio.
    pub(super) fn restore_session(&mut self, json: &str) {
        let Ok(s) = serde_json::from_str::<Session>(json) else { return };
        let Some(track) = s.queue.get(s.index).cloned() else { return };
        // It stopped on a song that's hidden now: nothing to pick up.
        if self.hide_explicit && track.explicit {
            return;
        }
        self.queue.adopt(s.queue, s.index);
        let shown = s.playing_as.clone().unwrap_or_else(|| track.clone());
        self.playing_as = s.playing_as.map(|song| PlayingAs { generation: self.generation, listed: track.id.clone(), song });
        let dur = shown.duration.map_or(s.duration_ms, |d| d as u64 * 1000);
        self.audio.status.duration_ms.store(dur, Ordering::Relaxed);
        self.audio.status.position_ms.store(s.position_ms, Ordering::Relaxed);
        self.resume_at = Some(Duration::from_millis(s.position_ms));
        self.listened_at = s.listened_at;
        self.request_art();
        self.show_in_media(&shown);
        self.backend.request(Req::Lyrics(shown));
    }

    /// Ask for the queue YouTube Music keeps across devices when the app starts and
    /// whenever its window comes back into focus with nothing playing, so a song
    /// started on the phone is here to carry on with.
    pub(super) fn sync_server_queue(&mut self, refocused: bool, now: f64) {
        if self.playing() {
            self.listened_at = unix_now();
        }
        // Back in the window: likes changed on another device show. Liked Music's entries
        // stand in for individual like reads, so it's asked for again, at most every ten
        // minutes (upstream's Ratings::refresh_if_stale); the cross-device queue keeps its
        // own minute below.
        if refocused && self.liked_checked.is_none_or(|t| now - t >= 600.0) {
            self.liked_checked = Some(now);
            self.request_liked(false);
        }
        let idle = !self.playing() && !self.buffering && !self.jam.active() && self.smoke.is_none();
        let due = self.queue_checked.is_none_or(|t| refocused && now - t > 60.0);
        if self.logged_in && idle && due && std::env::var_os("YTM_SMOKE").is_none() {
            self.queue_checked = Some(now);
            self.backend.request(Req::ServerQueue);
        }
    }

    /// Take up what another device was playing, paused at its start like a restored
    /// session, when it played there after anything played here.
    pub(super) fn adopt_server_queue(&mut self, q: crate::ytm::ServerQueue) {
        let Some(track) = q.tracks.get(q.index).cloned() else { return };
        let idle = !self.playing() && !self.buffering && !self.jam.active();
        // A minute's slack: this app's own plays reach that queue too.
        let hidden = self.hide_explicit && track.explicit;
        if !idle || hidden || q.watched_at <= self.listened_at + 60 || self.current_id() == Some(track.id.as_str()) {
            return;
        }
        log::info!("picking up {} from another device ({} in its queue)", track.id, q.tracks.len());
        self.queue.adopt(q.tracks, q.index);
        // Not a new play: an old song version mustn't come back with its video.
        self.playing_as = None;
        self.queue_source = None;
        self.listened_at = q.watched_at;
        self.audio.send(AudioCmd::Stop);
        self.audio.status.duration_ms.store(track.duration.map_or(0, |d| d as u64 * 1000), Ordering::Relaxed);
        self.audio.status.position_ms.store(0, Ordering::Relaxed);
        self.resume_at = Some(Duration::ZERO);
        self.resume_counts = true;
        self.request_art();
        self.show_in_media(&track);
        self.backend.request(Req::Lyrics(track.clone()));
        self.toast(format!("Picked up \"{}\" from your other device. Press play to carry on.", track.title));
    }

    pub(super) fn session_json(&self) -> Option<String> {
        let (queue, index) = self.queue.saved()?;
        serde_json::to_string(&Session {
            queue: queue.to_vec(),
            index,
            position_ms: self.audio.status.position().as_millis() as u64,
            duration_ms: self.audio.status.duration_ms.load(Ordering::Relaxed),
            listened_at: if self.playing() { unix_now() } else { self.listened_at },
            playing_as: self.playing_as().cloned(),
        })
        .ok()
    }

    /// The song's download failed partway: pick it up where it stopped from a fresh
    /// stream, rather than letting it end early and skip to the next one. If that keeps
    /// failing, stay on the song, ready to resume from there.
    pub(super) fn recover(&mut self, at: Duration, failure: crate::stream::DownloadFailure) {
        const MAX_RECOVERIES: u32 = 3;
        let reason = failure.reason;
        if failure.offer_backup && self.offer_backup_stream(at) {
            log::warn!("playback interrupted at {at:?} ({reason}); offering the backup stream");
            return;
        }
        if self.recoveries < MAX_RECOVERIES {
            self.recoveries += 1;
            log::warn!("playback interrupted at {at:?} ({reason}); reopening (try {})", self.recoveries);
            self.play_current_at(at, false);
            return;
        }
        log::warn!("playback interrupted at {at:?} ({reason}); giving up");
        self.hold_interrupted_track(at);
        let title = self.current().map(|t| t.title.clone()).unwrap_or_default();
        self.toast(format!("Lost the connection while playing \"{title}\". Press play to pick up where it stopped."));
    }

    /// Offer the backup-stream prompt for a bot-checked stream: it takes over the song,
    /// held where it stopped. False if it can't (already on, dismissed, a jam guest, or
    /// nothing playing).
    pub(super) fn offer_backup_stream(&mut self, at: Duration) -> bool {
        use super::BackupPrompt;
        if self.backup_stream || self.backup_prompt == BackupPrompt::Dismissed || self.jam.follows() || self.current().is_none() {
            return false;
        }
        self.backup_prompt = BackupPrompt::Open;
        self.skip_streak = 0;
        self.recoveries = 0;
        self.audio.send(AudioCmd::Pause);
        self.hold_interrupted_track(at);
        true
    }

    /// The player has already run dry; Stop would also reset the position to 0:00.
    fn hold_interrupted_track(&mut self, at: Duration) {
        self.audio.status.position_ms.store(at.as_millis() as u64, Ordering::Relaxed);
        self.resume_at = Some(at);
        self.buffering = false;
    }

    pub(super) fn play_current(&mut self) {
        self.recoveries = 0;
        super::jam::hand_picked(self);
        self.play_current_at(Duration::ZERO, true);
    }

    /// Start the current track, opened directly at `at` so nothing plays from 0:00 first.
    /// `report`: a new play for the account's history (not picking up last session's song).
    pub(super) fn play_current_at(&mut self, at: Duration, report: bool) {
        self.start_current(at, report, false);
    }

    /// Whether a video is what's on show for `t`: lyrics are timed to its song, so they
    /// don't go with it.
    pub(super) fn video_mode(&self, t: &Track) -> bool {
        self.show_video && t.has_video() && self.playing_as().is_none() && !self.jam.active()
    }

    /// From its video to its song version, at about the same place.
    pub(super) fn play_song_version(&mut self) {
        let at = self.audio.status.position();
        if self.playing() || self.buffering {
            self.start_current(at, false, true);
        } else {
            self.resume_at = Some(at);
            self.resume_as_song = true;
        }
    }

    fn start_current(&mut self, at: Duration, report: bool, as_song: bool) {
        let as_song = std::mem::take(&mut self.resume_as_song) || as_song;
        let Some(track) = self.current().cloned() else { return };
        // A natural Automix handoff keeps the outgoing song on screen until the next one
        // is louder (see `MixFrom`); any other play switches at once.
        let mixing = self.pending_mix.is_some();
        if !mixing {
            self.mix_from = None;
        }
        // Picking up mid-song (a resume, a download that broke off): the position is the
        // song version's when one was playing, so that goes on; a music video stays one.
        let carried = if at.is_zero() { None } else { self.playing_as().cloned() };
        self.queue.started();
        self.resume_at = None;
        self.resume_counts = false;
        self.generation += 1;
        self.audio.status.wanted_gen.store(self.generation, Ordering::Relaxed);
        self.audio_info = None;
        // The new song's stream stats arrive with its own decoder; a background
        // upgrade under way was for the song before it.
        self.stream_stats = None;
        self.upgrading_quality = false;
        self.upgrade_asked = false;
        let shown = carried.clone().unwrap_or_else(|| track.clone());
        self.audio.status.duration_ms.store(shown.duration.map_or(0, |d| d as u64 * 1000), Ordering::Relaxed);
        // A song running into the next crossfades (so does Next, when that's switched
        // on); going back or picking a song cuts over.
        self.audio.send(if self.handoff && at.is_zero() { AudioCmd::Handoff } else { AudioCmd::Stop });
        self.audio.send(AudioCmd::Warm);
        self.buffering = true;
        self.seek_drag = None;
        // A carried song version goes with its music video, which plays if the song won't.
        let song_of = if carried.is_none() && (at.is_zero() || as_song) { self.song_of(&track) } else { None };
        self.playing_as = carried.clone().map(|song| PlayingAs { generation: self.generation, listed: track.id.clone(), song });
        self.backend.request(Req::Play { video_id: track.id.clone(), generation: self.generation, at, report, song_of, song: carried, track: Some(track.clone()) });
        if mixing {
            // The outgoing song keeps the art, lyrics and media info it still deserves.
        } else {
            self.show_in_media(&shown);
            // Background art follows the track; lyrics and segments, what plays.
            self.request_art();
            self.refresh_lyrics(shown);
        }
        if let Some(m) = &mut self.media {
            m.start(at);
        }
        self.top_up();
    }

    /// The playing track's title, artist and length for the system's media controls,
    /// with the listed track's art (what the app shows too).
    pub(super) fn show_in_media(&mut self, shown: &Track) {
        let Some(track) = self.current().cloned() else { return };
        let Some(m) = &mut self.media else { return };
        // Windows fetches a cover URL on its own, through its own proxy setting only.
        let by_url = matches!(crate::proxy::active(), crate::proxy::Setting::System);
        // A local song's cover is a file already: the OS reads it from where it is.
        let local = crate::local::cover_file(&track.id).map(|p| format!("file://{}", p.display()));
        let art = backend::art_url(&track);
        let cover = local.as_deref().or_else(|| art.as_deref().filter(|_| by_url));
        m.set_track(&shown.title, &shown.artist_line(), shown.album.as_ref().map(|a| a.name.as_str()), cover, shown.duration);
        if !by_url && local.is_none() && let Some(url) = art {
            self.fetch_media_cover(track.id.clone(), url);
        }
    }

    /// Lyrics and SponsorBlock segments for `playing`, which timing is its own.
    pub(super) fn refresh_lyrics(&mut self, playing: Track) {
        self.lyrics = None;
        self.lyrics_extras = None;
        self.extras_asked = None;
        self.lyric_view.reset();
        self.backend.request(Req::Lyrics(playing));
        self.segments = None;
        self.request_segments();
    }

    /// Keep the queue topped up so autoplay never stalls at the end; a playlist still
    /// coming in goes first. The seed drifts — the newest queue song this session
    /// hasn't grown a radio from yet, so the queue keeps moving forward instead of
    /// orbiting one mix (Mochy's infinite queue does the same); an answer that lands
    /// nothing cools off first, so a dead station can't become a request a frame.
    pub(super) fn top_up(&mut self) {
        if let Some(at) = self.radio_retry_at
            && std::time::Instant::now() < at
        {
            return;
        }
        if self.autoplay && !self.radio_pending && !self.jam.follows() && self.queue.fill.is_none() && self.queue.at_end() {
            let banned = &self.listening.banned;
            let seed = self
                .queue
                .tracks
                .iter()
                .rev()
                .find(|t| !crate::local::is_local(&t.id) && !self.used_radio_seeds.contains(&t.id) && !banned.contains(&t.id))
                .cloned()
                .or_else(|| self.current().cloned().filter(|t| !crate::local::is_local(&t.id)));
            if let Some(t) = seed {
                self.radio_pending = true;
                self.used_radio_seeds.push_back(t.id.clone());
                if self.used_radio_seeds.len() > 100 {
                    self.used_radio_seeds.pop_front();
                }
                // the queue's own radio: picked from the taste, the way Home's rows are
                let taste = crate::recommend::TasteLite::of(&self.listening, self.hide_explicit);
                self.backend.request(Req::AutoRadio { song_of: self.song_of(&t), seed: t.id, taste });
            }
        }
    }

    /// Every frame: a jam starting or ending changes it too. Normal in a jam, where
    /// everyone hears the same song at the same moment.
    pub(super) fn sync_tempo(&self) {
        self.audio.set_speed(if self.jam.active() || !self.tempo_on { 1.0 } else { self.speed });
        self.audio.set_keep_pitch(self.keep_pitch);
    }

    pub(crate) fn set_crossfade(&self) {
        // Automix is a crossfade with its own shape, so it needs the length too.
        self.audio.set_automix(self.automix);
        self.audio.set_crossfade(if self.crossfade || self.automix { self.crossfade_secs } else { 0 });
    }

    pub(crate) fn set_eq(&self) {
        self.audio.set_eq(self.eq_on, &self.eq_gains, self.eq_no_clip);
    }

    /// `t` is explicit while those are hidden: it won't play, and the toast says why.
    pub(super) fn explicit_hidden(&mut self, t: &Track) -> bool {
        let hidden = self.hide_explicit && t.explicit;
        if hidden {
            self.toast("Skipped an explicit song");
        }
        hidden
    }

    /// Repeat One replays the song, unless it's an explicit one left to finish when those
    /// were hidden.
    pub(super) fn repeats_one(&self) -> bool {
        self.queue.repeat == Repeat::One && !self.current().is_some_and(|t| self.hide_explicit && t.explicit)
    }

    /// Settings › Playback › Show podcasts: Home and the library are asked for again, with
    /// or without them.
    pub(super) fn set_show_podcasts(&mut self) {
        crate::ytm::set_podcasts(self.show_podcasts);
        if !self.show_podcasts && self.page == super::Page::Library(super::LibTab::Podcasts) {
            self.page = super::Page::Library(super::LibTab::Playlists);
        }
        self.search = None;
        self.home_mood = false;
        self.backend.request(Req::Home(None));
        if self.logged_in {
            self.backend.request(Req::Library);
        }
    }

    /// Explicit songs hidden or shown again: queued ones go (all but the one playing; a
    /// follower's queue is the room's), and YouTube's Restricted Mode applies to what
    /// loads from now on.
    pub(super) fn set_hide_explicit(&mut self) {
        self.queue.hide_explicit = self.hide_explicit;
        if !self.jam.follows() {
            self.queue.drop_explicit();
            // What was fetched ahead may have gone.
            self.prefetch_next();
        }
        self.backend.set_hide_explicit(self.hide_explicit, self.restricted_mode);
    }

    /// Ask SponsorBlock about the playing song, when that's switched on.
    pub(super) fn request_segments(&mut self) {
        if !self.sponsorblock || self.segments.is_some() {
            return;
        }
        // What plays: a song version's segments are its own.
        let Some(video_id) = self.playing_id().map(str::to_owned) else { return };
        self.backend.request(Req::Segments { video_id });
    }

    /// Jump over a SponsorBlock segment once playback is inside it; one that runs to the
    /// end finishes the song. A member who doesn't drive the party follows it, and the one
    /// who does skips for everyone.
    pub(super) fn skip_segments(&mut self, ctx: &egui::Context, now: f64) {
        if !self.sponsorblock || self.jam.follows() || self.resume_at.is_some() || self.buffering || !self.playing() {
            return;
        }
        let Some((id, segs)) = &self.segments else { return };
        // A seek takes a moment to show in the position: don't send it twice.
        if self.playing_id() != Some(id.as_str()) || now - self.segment_skipped < 1.5 {
            return;
        }
        // The stream's own length: the song row's can be another cut's (the song's,
        // where the video plays), and a segment marked on another cut skips wrong.
        let Some(duration) = self.audio.status.duration().map(|d| d.as_secs_f64()) else { return };
        let pos = self.audio.status.position().as_secs_f64();
        let mut segs = segs.iter().filter(|s| s.fits(duration));
        let Some(seg) = segs.clone().find(|s| pos >= s.start && pos < s.end - 0.3).copied() else {
            // Wake up for the next one even when nothing else repaints.
            if let Some(next) = segs.find(|s| s.start > pos) {
                ctx.request_repaint_after(Duration::from_secs_f64(next.start - pos + 0.05));
            }
            return;
        };
        self.segment_skipped = now;
        log::info!("sponsorblock: skipping {:.1}–{:.1} s", seg.start, seg.end);
        if seg.end >= duration - 1.0 {
            if self.repeats_one() {
                self.play_current();
            } else {
                self.next();
            }
        } else {
            self.audio.send(AudioCmd::Seek(Duration::from_secs_f64(seg.end)));
        }
    }

    /// Fetch whatever plays after the current track so skipping is instant.
    pub(super) fn prefetch_next(&self) {
        if let Some(t) = self.queue.upcoming() {
            self.backend.request(Req::Prefetch { video_id: t.id.clone(), song_of: self.song_of(t), track: Some(t.clone()) });
        }
    }

    pub(super) fn next(&mut self) {
        if self.jam.locked() {
            return self.toast("Only the host can change the party");
        }
        if !self.queue.advance() {
            return;
        }
        self.play_current();
    }

    pub(super) fn prev(&mut self) {
        if self.jam.locked() {
            return self.toast("Only the host can change the party");
        }
        if self.audio.status.position() > Duration::from_secs(3) {
            self.seek_to(Duration::ZERO);
            return;
        }
        if self.queue.back() {
            self.play_current();
        }
    }

    pub(super) fn seek_to(&mut self, at: Duration) {
        self.seek_wheel_seeked(at.as_secs_f32());
        // A needle this device moved is the room's to hear about; `publish` keeps telling
        // it until the room agrees. Only hand seeks come through here — the sync's own
        // corrections seek the player directly, so drift never gets reported as a move.
        if self.jam.active() {
            self.jam.pending_seek = Some(at.as_millis() as u64);
        }
        if self.resume_at.is_some() {
            // Restored but not loaded yet (nothing for the player to seek): move the start point instead.
            self.resume_at = Some(at);
            self.audio.status.position_ms.store(at.as_millis() as u64, Ordering::Relaxed);
        } else {
            self.audio.send(AudioCmd::Seek(at));
        }
    }

    pub(super) fn toggle(&mut self) {
        if self.queue.index.is_none() {
            return;
        }
        if self.jam.active() {
            if self.jam.locked() && !self.playing() {
                super::jam::rejoin(self);
                return;
            }
            // Play and pause are the room's to know, so say so here first: that keeps the
            // sync from walking us straight back to the room's still-unmoved state while
            // our own ask is on its way. In a room only the host drives, a play stays the
            // room's alone, and a pause is ours.
            self.jam.intent = Some(!self.playing());
        }
        if let Some(at) = self.resume_at {
            // Last session's song, already counted when it started then (unless it was
            // another device's).
            let report = std::mem::take(&mut self.resume_counts);
            self.play_current_at(at, report);
            return;
        }
        let playing = self.playing();
        // The media controls catch up in `logic` once the player has switched.
        self.audio.send(if playing { AudioCmd::Pause } else { AudioCmd::Resume });
    }

    /// Shuffle up next, or back to the original order: a toggle, so a playlist or album
    /// can return to its sequence.
    pub(super) fn shuffle_toggle(&mut self) {
        if self.jam.locked() {
            return self.toast("Only the host can shuffle the party");
        }
        match self.queue.shuffle_toggle() {
            Some(true) => {
                self.toast("Shuffled up next");
                self.prefetch_next();
            }
            Some(false) => {
                self.toast("Original order restored");
                self.prefetch_next();
            }
            None => {}
        }
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// A position moved by `ms` milliseconds, which can be negative.
pub(super) fn shifted(pos: Duration, ms: i64) -> Duration {
    if ms >= 0 {
        pos + Duration::from_millis(ms as u64)
    } else {
        pos.saturating_sub(Duration::from_millis(ms.unsigned_abs()))
    }
}
