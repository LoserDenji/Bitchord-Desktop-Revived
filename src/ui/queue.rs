//! The play queue: its songs, the one playing, repeat, and the songs of a playlist
//! still coming in. Only the state: the app starts the songs these pick, prefetches,
//! and asks for radio.

use crate::model::Track;

#[derive(Clone, Copy, PartialEq)]
pub enum Repeat {
    Off,
    All,
    One,
}

/// A playlist's load whose next pages also go to the queue, and whether that queue
/// is shuffled (they're then mixed into what's still to come). `ran_out`: playback
/// reached the end of what had come, and carries on when more does (or the load ends).
#[derive(Clone)]
pub struct QueueFill {
    pub id: String,
    pub load: u64,
    pub shuffled: bool,
    pub ran_out: bool,
}

/// More of a playlist joined the queue.
pub struct Filled {
    /// Playback was waiting for it: go on to the next song.
    pub ran_out: bool,
    /// The song playing was the last one before.
    pub was_at_end: bool,
}

pub struct Queue {
    pub tracks: Vec<Track>,
    /// The song playing (or paused, or loading).
    pub index: Option<usize>,
    pub repeat: Repeat,
    /// The queue is a playlist played while its songs were still coming in: the rest
    /// join the queue as they arrive.
    pub fill: Option<QueueFill>,
    /// Explicit songs are hidden: none join, and a queue started with some leaves them out.
    pub hide_explicit: bool,
    /// Shuffle is a toggle: what played up next, in order, before the tail was shuffled.
    /// Set on the first shuffle, and taken back to restore the original order (see
    /// [`Queue::shuffle_toggle`]). Songs queued by hand while it's on stay at the end.
    pub shuffle_original: Option<Vec<Track>>,
    /// Songs added to the queue by hand (play next, add to queue, a radio's picks) rather
    /// than played from the list the queue was started from.
    elsewhere: std::collections::HashSet<String>,
}

/// The queue as it was before a removal or a clear, for Undo: it comes back only onto
/// the queue that change left, untouched since.
pub struct Saved {
    tracks: Vec<Track>,
    elsewhere: std::collections::HashSet<String>,
    index: Option<usize>,
    fill: Option<QueueFill>,
    /// Shuffle as it was: the order the tail goes back to.
    shuffle_original: Option<Vec<Track>>,
    /// What the change left: its songs and the one playing.
    left: (Vec<String>, Option<usize>),
    /// Pages of the playlist it was filling from that came in since (and whether more
    /// were to come): they join it as it comes back.
    pages: Vec<(Vec<Track>, bool)>,
}

/// How many songs before and after the current one a saved session keeps: liked-songs
/// queues can run to 1000+ songs.
const SAVED_BEFORE: usize = 50;
const SAVED_AFTER: usize = 150;

impl Queue {
    pub fn new(repeat: Repeat) -> Self {
        Self { tracks: Vec::new(), elsewhere: Default::default(), index: None, repeat, fill: None, hide_explicit: false, shuffle_original: None }
    }

    /// The song playing was added by hand, not played from the list the queue was
    /// started from.
    pub fn plays_elsewhere(&self) -> bool {
        self.current().is_some_and(|t| self.elsewhere.contains(&t.id))
    }

    fn note_added(&mut self, id: &str) {
        if !self.tracks.iter().any(|t| t.id == id) {
            self.elsewhere.insert(id.to_owned());
        }
    }

    pub fn current(&self) -> Option<&Track> {
        self.index.and_then(|i| self.tracks.get(i))
    }

    /// The song playing is the last one.
    pub fn at_end(&self) -> bool {
        self.index.is_some_and(|i| i + 1 >= self.tracks.len())
    }

    /// Play `tracks` from `start`; `fill`: a playlist still coming in, whose other songs
    /// follow. False (and nothing changes) when there are none. With explicit songs hidden
    /// they're left out, and an explicit `start` plays the next song that isn't.
    pub fn replace(&mut self, mut tracks: Vec<Track>, mut start: usize, fill: Option<QueueFill>) -> bool {
        if self.hide_explicit {
            start = tracks.iter().take(start).filter(|t| !t.explicit).count();
            tracks.retain(|t| !t.explicit);
        }
        if tracks.is_empty() {
            return false;
        }
        self.fill = fill;
        self.index = Some(start.min(tracks.len() - 1));
        self.tracks = tracks;
        self.shuffle_original = None;
        self.elsewhere.clear();
        true
    }

    /// Take up a queue from elsewhere (last session, another device), at `index`, which
    /// the caller has checked is one of `tracks`.
    pub fn adopt(&mut self, tracks: Vec<Track>, index: usize) {
        self.tracks = tracks;
        self.index = Some(index);
        self.fill = None;
        self.shuffle_original = None;
        self.elsewhere.clear();
        self.drop_explicit();
    }

    /// With explicit songs hidden, take them out, all but the one playing.
    pub fn drop_explicit(&mut self) {
        if !self.hide_explicit {
            return;
        }
        let mut i = 0;
        let (cur, mut index) = (self.index, self.index);
        self.tracks.retain(|t| {
            let keep = !t.explicit || Some(i) == cur;
            if !keep && cur.is_some_and(|c| i < c) {
                index = index.map(|c| c - 1);
            }
            i += 1;
            keep
        });
        self.index = index;
    }

    /// Next, or the end of a song: true when there's a song to play. At the end of a
    /// playlist still coming in, playback waits for its next songs. (Repeating one song
    /// is the caller's: a song's end replays it rather than coming here.)
    pub fn advance(&mut self) -> bool {
        let Some(i) = self.index else { return false };
        if i + 1 < self.tracks.len() {
            self.index = Some(i + 1);
        } else if let Some(fill) = &mut self.fill {
            fill.ran_out = true;
            return false;
        } else if self.repeat == Repeat::All && !self.tracks.is_empty() {
            self.index = Some(0);
        } else {
            return false;
        }
        // An explicit song left to finish when those were hidden goes once it has.
        self.drop_explicit();
        !self.current().is_some_and(|t| self.hide_explicit && t.explicit)
    }

    /// Back to the song before: false at the first.
    pub fn back(&mut self) -> bool {
        match self.index.filter(|&i| i > 0) {
            Some(i) => {
                self.index = Some(i - 1);
                self.drop_explicit();
                true
            }
            None => false,
        }
    }

    /// What plays after the current song, to fetch ahead.
    pub fn upcoming(&self) -> Option<&Track> {
        let i = self.index?;
        match self.repeat {
            Repeat::One => None,
            Repeat::All => self.tracks.get(i + 1).or(self.tracks.first()),
            Repeat::Off => self.tracks.get(i + 1),
        }
    }

    /// A song started: it no longer waits for a playlist's next songs.
    pub fn started(&mut self) {
        if let Some(fill) = &mut self.fill {
            fill.ran_out = false;
        }
    }

    /// Shuffle what's still to come, or put it back in order: a toggle, so a playlist or
    /// album can return to its original sequence. True when the tail is now shuffled,
    /// false when the original order was restored, and `None` when there's nothing to
    /// shuffle. Songs queued by hand while it was on keep their place, at the end.
    pub fn shuffle_toggle(&mut self) -> Option<bool> {
        if self.shuffle_original.is_some() {
            self.unshuffle();
            return Some(false);
        }
        let start = self.index.map_or(0, |i| i + 1);
        if start >= self.tracks.len() {
            return None;
        }
        self.shuffle_original = Some(self.tracks[start..].to_vec());
        super::shuffle(&mut self.tracks[start..]);
        if let Some(fill) = &mut self.fill {
            fill.shuffled = true;
        }
        Some(true)
    }

    /// Restore the tail's original order: the songs still there from it, in it, then any
    /// that joined while it was shuffled (queued by hand, or a radio that went on) in
    /// their order now. The song playing never moves.
    fn unshuffle(&mut self) {
        let Some(original) = self.shuffle_original.take() else { return };
        let start = self.index.map_or(0, |i| i + 1).min(self.tracks.len());
        // Matches are taken out of the tail as they're claimed, so a song that's in the
        // queue twice matches once each, and what's left is what was added meanwhile.
        let mut pool: Vec<Track> = self.tracks[start..].to_vec();
        let mut tail: Vec<Track> = Vec::with_capacity(pool.len());
        for want in &original {
            if let Some(at) = pool.iter().position(|t| t.id == want.id) {
                tail.push(pool.remove(at));
            }
        }
        tail.extend(pool);
        self.tracks.truncate(start);
        self.tracks.extend(tail);
        if let Some(fill) = &mut self.fill {
            fill.shuffled = false;
        }
    }

    /// Whether the tail is shuffled now (and clicking the button would restore the order).
    pub fn shuffled(&self) -> bool {
        self.shuffle_original.is_some()
    }

    /// Play `t` after the current song: true when nothing was playing, so it starts now.
    pub fn play_next(&mut self, t: Track) -> bool {
        let at = self.index.map_or(0, |i| i + 1).min(self.tracks.len());
        self.note_added(&t.id);
        self.tracks.insert(at, t);
        if self.index.is_some() {
            return false;
        }
        self.index = Some(0);
        true
    }

    /// Add `t` at the end: true when nothing was playing, so it starts now.
    pub fn enqueue(&mut self, t: Track) -> bool {
        self.note_added(&t.id);
        self.tracks.push(t);
        if self.index.is_some() {
            return false;
        }
        self.index = Some(self.tracks.len() - 1);
        true
    }

    /// Take song `i` out, unless it's the one playing: the queue before, for Undo.
    pub fn remove(&mut self, i: usize) -> Option<Saved> {
        if i >= self.tracks.len() || Some(i) == self.index {
            return None;
        }
        let before = self.save();
        self.tracks.remove(i);
        if let Some(cur) = self.index.filter(|&c| c > i) {
            self.index = Some(cur - 1);
        }
        Some(self.left(before))
    }

    /// Everything but the song playing goes, the songs played before it too, and a
    /// playlist still coming in stops joining: the queue before, for Undo. `None` when
    /// there's nothing to take out.
    pub fn clear(&mut self) -> Option<Saved> {
        let keep = self.current().cloned();
        if self.tracks.len() <= usize::from(keep.is_some()) && self.fill.is_none() {
            return None;
        }
        let before = self.save();
        self.tracks = keep.into_iter().collect();
        self.index = self.index.map(|_| 0);
        self.fill = None;
        Some(self.left(before))
    }

    /// The queue as it was before a change, if nothing has changed it since: `None`
    /// (and nothing changes) otherwise. True when playback was waiting at the end for more
    /// and now has a next song to go on to.
    /// `playing_it`: what's playing is the playlist it was filling from.
    pub fn restore(&mut self, s: Saved, playing_it: impl Fn(&str) -> bool) -> Option<bool> {
        if self.fingerprint() != s.left {
            return None;
        }
        let waiting = self.fill.as_ref().is_some_and(|f| f.ran_out);
        (self.tracks, self.elsewhere, self.index, self.fill, self.shuffle_original) =
            (s.tracks, s.elsewhere, s.index, s.fill, s.shuffle_original);
        for (tracks, more) in s.pages {
            let Some(f) = &self.fill else { break };
            let (id, load) = (f.id.clone(), f.load);
            self.fill_from(&id, load, tracks, more, playing_it(&id));
        }
        if !waiting {
            return Some(false);
        }
        if self.at_end() && let Some(f) = &mut self.fill {
            f.ran_out = true;
        }
        Some(!self.at_end())
    }

    /// [`Self::fill_from`], with `saved` (the queue before a change, for Undo) getting
    /// the page too when it was filling from that load. Joining this queue then isn't a
    /// change that keeps it from coming back.
    pub fn fill_saved(&mut self, saved: Option<&mut Saved>, id: &str, load: u64, tracks: Vec<Track>, more: bool, playing_it: bool) -> Option<Filled> {
        let Some(s) = saved.filter(|s| s.fill.as_ref().is_some_and(|f| f.id == id && f.load == load)) else {
            return self.fill_from(id, load, tracks, more, playing_it);
        };
        let untouched = self.fingerprint() == s.left;
        s.pages.push((tracks.clone(), more));
        let filled = self.fill_from(id, load, tracks, more, playing_it);
        if untouched {
            s.left = self.fingerprint();
        }
        filled
    }

    fn save(&self) -> Saved {
        Saved {
            tracks: self.tracks.clone(),
            elsewhere: self.elsewhere.clone(),
            index: self.index,
            fill: self.fill.clone(),
            shuffle_original: self.shuffle_original.clone(),
            left: Default::default(),
            pages: Vec::new(),
        }
    }

    fn left(&self, mut before: Saved) -> Saved {
        before.left = self.fingerprint();
        before
    }

    fn fingerprint(&self) -> (Vec<String>, Option<usize>) {
        (self.tracks.iter().map(|t| t.id.clone()).collect(), self.index)
    }

    /// Move a song to another place in the queue (drag and drop). The song playing
    /// stays the one playing, wherever it lands.
    pub fn move_track(&mut self, from: usize, to: usize) {
        let n = self.tracks.len();
        if from >= n || to >= n || from == to {
            return;
        }
        let t = self.tracks.remove(from);
        self.tracks.insert(to, t);
        if let Some(cur) = self.index {
            self.index = Some(if cur == from {
                to
            } else if from < cur && to >= cur {
                cur - 1
            } else if from > cur && to <= cur {
                cur + 1
            } else {
                cur
            });
        }
    }

    /// Radio songs for `seed` join the end, without the seed or songs already queued.
    /// Returns whether the song playing was the last one.
    pub fn add_radio(&mut self, seed: &str, tracks: Vec<Track>) -> bool {
        let have: std::collections::HashSet<String> = self.tracks.iter().map(|t| t.id.clone()).collect();
        let was_at_end = self.at_end();
        let added: Vec<Track> = tracks.into_iter().filter(|t| t.id != seed && !have.contains(&t.id) && !(self.hide_explicit && t.explicit)).collect();
        self.elsewhere.extend(added.iter().map(|t| t.id.clone()));
        self.tracks.extend(added);
        was_at_end
    }

    /// The next page of the playlist the queue fills from (`id`, `load`) joins it; `more`:
    /// pages still follow. `playing_it`: the queue still plays that playlist. `None` when
    /// the page doesn't apply (though the last one still ends the fill); an empty last
    /// page that does still says whether playback was waiting for it.
    pub fn fill_from(&mut self, id: &str, load: u64, tracks: Vec<Track>, more: bool, playing_it: bool) -> Option<Filled> {
        let fill = self.fill.as_mut().filter(|f| f.id == id && f.load == load)?;
        let (shuffled, ran_out) = (fill.shuffled, std::mem::take(&mut fill.ran_out));
        if !more {
            self.fill = None;
        }
        if !playing_it {
            return None;
        }
        let was_at_end = self.at_end();
        let hide = self.hide_explicit;
        self.tracks.extend(tracks.into_iter().filter(|t| !(hide && t.explicit)));
        if shuffled {
            // Mixed into what's still to come, past the next song (it may be loading already).
            let from = self.index.map_or(0, |i| i + if was_at_end { 1 } else { 2 }).min(self.tracks.len());
            super::shuffle(&mut self.tracks[from..]);
        }
        Some(Filled { ran_out, was_at_end })
    }

    /// The part of the queue a saved session keeps, and the current song's place in it.
    pub fn saved(&self) -> Option<(&[Track], usize)> {
        let i = self.index?;
        let from = i.saturating_sub(SAVED_BEFORE);
        let to = (i + SAVED_AFTER).min(self.tracks.len());
        Some((&self.tracks[from..to], i - from))
    }
}

#[cfg(test)]
mod tests {
    use super::{Queue, QueueFill, Repeat, Track};

    fn track(id: &str) -> Track {
        Track { id: id.into(), title: id.into(), artists: vec![], album: None, duration: None, thumbs: vec![], track_nr: None, plays: None, ..Default::default() }
    }

    fn queue(songs: &[&str], index: usize, repeat: Repeat) -> Queue {
        let mut q = Queue::new(repeat);
        assert!(q.replace(songs.iter().map(|s| track(s)).collect(), index, None));
        q
    }

    fn ids(q: &Queue) -> Vec<&str> {
        q.tracks.iter().map(|t| t.id.as_str()).collect()
    }

    fn filling(id: &str, load: u64, shuffled: bool) -> Option<QueueFill> {
        Some(QueueFill { id: id.into(), load, shuffled, ran_out: false })
    }

    #[test]
    fn next_and_back_follow_repeat() {
        let mut q = queue(&["a", "b"], 0, Repeat::Off);
        assert!(q.advance());
        assert_eq!(q.index, Some(1));
        // At the end: stays there, unless repeating all.
        assert!(!q.advance());
        assert_eq!(q.index, Some(1));
        q.repeat = Repeat::All;
        assert!(q.advance());
        assert_eq!(q.index, Some(0));
        // Next moves on even when repeating one song (the song's own end replays it).
        q.repeat = Repeat::One;
        assert!(q.advance());
        assert_eq!(q.index, Some(1));
        assert!(q.back());
        assert!(!q.back());
        assert_eq!(q.index, Some(0));
        // Nothing queued: nothing to do.
        assert!(!Queue::new(Repeat::All).advance());
    }

    #[test]
    fn what_plays_next_is_fetched_ahead() {
        let q = queue(&["a", "b"], 1, Repeat::Off);
        assert!(q.upcoming().is_none());
        let q = queue(&["a", "b"], 1, Repeat::All);
        assert_eq!(q.upcoming().unwrap().id, "a");
        let q = queue(&["a", "b"], 0, Repeat::One);
        assert!(q.upcoming().is_none());
        let q = queue(&["a", "b"], 0, Repeat::Off);
        assert_eq!(q.upcoming().unwrap().id, "b");
    }

    #[test]
    fn playing_nothing_leaves_the_queue_as_it_was() {
        let mut q = Queue::new(Repeat::Off);
        q.replace(vec![track("a")], 0, filling("PL1", 1, false));
        assert!(!q.replace(vec![], 0, None));
        assert_eq!(ids(&q), ["a"]);
        assert_eq!(q.fill.as_ref().unwrap().id, "PL1");
        // A start past the end plays the last song.
        assert!(q.replace(vec![track("x"), track("y")], 9, None));
        assert_eq!(q.current().unwrap().id, "y");
    }

    #[test]
    fn songs_added_by_hand() {
        let mut q = Queue::new(Repeat::Off);
        // Into an empty queue: it starts.
        assert!(q.enqueue(track("a")));
        assert_eq!(q.index, Some(0));
        assert!(!q.enqueue(track("c")));
        assert!(!q.play_next(track("b")));
        assert_eq!(ids(&q), ["a", "b", "c"]);
        let mut empty = Queue::new(Repeat::Off);
        assert!(empty.play_next(track("x")));
        assert_eq!(empty.current().unwrap().id, "x");

        // Taking one out keeps the song playing where it is; the one playing stays.
        q.index = Some(2);
        assert!(q.remove(0).is_some());
        assert_eq!(ids(&q), ["b", "c"]);
        assert_eq!(q.current().unwrap().id, "c");
        assert!(q.remove(1).is_none());
        assert!(q.remove(5).is_none());
        assert_eq!(ids(&q), ["b", "c"]);
    }

    #[test]
    fn undoing_a_removal_brings_back_the_queue_as_it_was() {
        // A song played before the current one: the current one moves up a place.
        let mut q = queue(&["a", "b", "c", "d"], 2, Repeat::Off);
        q.enqueue(track("e"));
        let saved = q.remove(0).unwrap();
        assert_eq!((ids(&q), q.index), (vec!["b", "c", "d", "e"], Some(1)));
        assert!(q.restore(saved, |_| true).is_some());
        assert_eq!((ids(&q), q.index), (vec!["a", "b", "c", "d", "e"], Some(2)));
        // "e" still counts as added, not as what the queue was started from.
        q.index = Some(4);
        assert!(q.plays_elsewhere());
    }

    #[test]
    fn undoing_a_clear_brings_back_what_played_and_whats_to_come() {
        let mut q = queue(&["a", "b", "c", "d"], 1, Repeat::Off);
        q.fill = filling("PL1", 7, true);
        let saved = q.clear().unwrap();
        assert_eq!((ids(&q), q.index, q.fill.is_none()), (vec!["b"], Some(0), true));
        assert!(q.clear().is_none(), "only the song playing is left");
        assert!(q.restore(saved, |_| true).is_some());
        assert_eq!((ids(&q), q.index), (vec!["a", "b", "c", "d"], Some(1)));
        let fill = q.fill.as_ref().unwrap();
        assert_eq!((fill.id.as_str(), fill.load, fill.shuffled), ("PL1", 7, true));
        // With nothing playing, all of it goes.
        q.index = None;
        q.clear().unwrap();
        assert!(q.tracks.is_empty() && q.index.is_none());
        assert!(Queue::new(Repeat::Off).clear().is_none());
    }

    #[test]
    fn pages_that_come_in_while_an_undo_waits_join_the_queue_it_brings_back() {
        // Cleared: the page goes nowhere now, and joins the queue back from Undo.
        let mut q = queue(&["a", "b"], 0, Repeat::Off);
        q.fill = filling("PL1", 7, false);
        let mut saved = q.clear().unwrap();
        assert!(q.fill_saved(Some(&mut saved), "PL2", 7, vec![track("x")], true, true).is_none());
        assert!(q.fill_saved(Some(&mut saved), "PL1", 7, vec![track("c")], true, true).is_none());
        assert!(q.restore(saved, |_| true).is_some());
        assert_eq!(ids(&q), ["a", "b", "c"]);
        assert!(q.fill.is_some(), "more is to come");
        // Taken out of: the page joins now, and isn't a change that stops the Undo.
        let mut saved = q.remove(1).unwrap();
        q.fill_saved(Some(&mut saved), "PL1", 7, vec![track("d")], false, true).unwrap();
        assert_eq!(ids(&q), ["a", "c", "d"]);
        assert!(q.restore(saved, |_| true).is_some());
        assert_eq!(ids(&q), ["a", "b", "c", "d"]);
        assert!(q.fill.is_none(), "that was the last page");
    }

    #[test]
    fn playback_waiting_at_the_end_still_waits_or_goes_on_after_an_undo() {
        // The song after it taken out, then playback reached the end and waits: Undo brings
        // that song back to go on to.
        let mut q = queue(&["a", "b", "c"], 1, Repeat::Off);
        q.fill = filling("PL1", 7, false);
        let saved = q.remove(2).unwrap();
        assert!(!q.advance());
        assert_eq!(q.restore(saved, |_| true), Some(true));
        assert_eq!(ids(&q), ["a", "b", "c"]);
        // An earlier song taken out: still at the end, so it waits on for the next page.
        let mut q = queue(&["a", "b", "c"], 2, Repeat::Off);
        q.fill = filling("PL1", 7, false);
        let saved = q.remove(0).unwrap();
        assert!(!q.advance());
        assert_eq!(q.restore(saved, |_| true), Some(false));
        let f = q.fill_from("PL1", 7, vec![track("d")], false, true).unwrap();
        assert!(f.ran_out);
    }

    #[test]
    fn a_queue_changed_since_isnt_undone() {
        let mut q = queue(&["a", "b", "c"], 0, Repeat::Off);
        let saved = q.remove(2).unwrap();
        q.enqueue(track("x"));
        assert!(q.restore(saved, |_| true).is_none());
        assert_eq!(ids(&q), ["a", "b", "x"]);
        // Playback moving on counts too: the song playing would be another.
        let saved = q.clear().unwrap();
        q.enqueue(track("y"));
        q.advance();
        assert!(q.restore(saved, |_| true).is_none());
        assert_eq!((ids(&q), q.index), (vec!["a", "y"], Some(1)));
    }

    #[test]
    fn radio_adds_only_new_songs() {
        let mut q = queue(&["seed", "a"], 1, Repeat::Off);
        assert!(q.add_radio("seed", vec![track("seed"), track("a"), track("b"), track("c")]));
        assert_eq!(ids(&q), ["seed", "a", "b", "c"]);
        assert!(!q.add_radio("seed", vec![track("d")]));
    }

    #[test]
    fn moving_songs_keeps_the_song_playing() {
        let mut q = queue(&["a", "b", "c", "d"], 2, Repeat::Off);
        // The playing song moves: it stays the one playing.
        q.move_track(2, 0);
        assert_eq!(ids(&q), ["c", "a", "b", "d"]);
        assert_eq!(q.index, Some(0));
        // A song before the playing one leaves: the playing one slides up.
        let mut q = queue(&["a", "b", "c", "d"], 2, Repeat::Off);
        q.move_track(0, 3);
        assert_eq!(ids(&q), ["b", "c", "d", "a"]);
        assert_eq!(q.index, Some(1));
        // A song after the playing one jumps ahead of it: the playing one slides down.
        let mut q = queue(&["a", "b", "c", "d"], 1, Repeat::Off);
        q.move_track(3, 0);
        assert_eq!(ids(&q), ["d", "a", "b", "c"]);
        assert_eq!(q.index, Some(2));
        // A song next to the playing one moved over it: the playing one slides.
        let mut q = queue(&["a", "b", "c", "d"], 1, Repeat::Off);
        q.move_track(2, 0);
        assert_eq!(ids(&q), ["c", "a", "b", "d"]);
        assert_eq!(q.index, Some(2));
        // A song moved within one side of the playing song leaves it alone.
        let mut q = queue(&["a", "b", "c", "d"], 2, Repeat::Off);
        q.move_track(0, 1);
        assert_eq!(ids(&q), ["b", "a", "c", "d"]);
        assert_eq!(q.index, Some(2));
        // No-ops don't disturb anything.
        let mut q = queue(&["a", "b"], 0, Repeat::Off);
        q.move_track(0, 0);
        q.move_track(1, 5);
        q.move_track(5, 0);
        assert_eq!(ids(&q), ["a", "b"]);
        assert_eq!(q.index, Some(0));
    }

    #[test]
    fn a_playlist_still_coming_in_fills_the_queue() {
        let mut q = Queue::new(Repeat::Off);
        q.replace(vec![track("a"), track("b")], 0, filling("PL1", 3, false));
        // Another playlist's page, or another load's, doesn't join.
        assert!(q.fill_from("PL2", 3, vec![track("x")], true, true).is_none());
        assert!(q.fill_from("PL1", 2, vec![track("x")], true, true).is_none());
        let f = q.fill_from("PL1", 3, vec![track("c")], true, true).unwrap();
        assert!(!f.ran_out && !f.was_at_end);
        assert_eq!(ids(&q), ["a", "b", "c"]);

        // Playback reached the end before the next page: it waits, then carries on.
        q.index = Some(2);
        assert!(!q.advance());
        assert!(q.fill.as_ref().unwrap().ran_out);
        let f = q.fill_from("PL1", 3, vec![track("d")], false, true).unwrap();
        assert!(f.ran_out && f.was_at_end);
        assert!(q.fill.is_none());
        assert!(q.advance());
        assert_eq!(q.current().unwrap().id, "d");
    }

    #[test]
    fn waiting_for_a_playlist_comes_before_repeating_it() {
        let mut q = Queue::new(Repeat::All);
        q.replace(vec![track("a"), track("b")], 1, filling("PL1", 1, false));
        // At the end of what came: it waits rather than starting over.
        assert!(!q.advance());
        assert_eq!(q.index, Some(1));
        // The load ends with nothing more: playback goes on, and now starts over.
        let f = q.fill_from("PL1", 1, vec![], false, true).unwrap();
        assert!(f.ran_out && f.was_at_end);
        assert!(q.fill.is_none());
        assert!(q.advance());
        assert_eq!(q.index, Some(0));
    }

    #[test]
    fn filling_stops_when_something_else_plays() {
        let mut q = Queue::new(Repeat::Off);
        q.replace(vec![track("a")], 0, filling("PL1", 1, false));
        // The queue plays something else now: the page doesn't join, and the last one ends the fill.
        assert!(q.fill_from("PL1", 1, vec![track("b")], true, false).is_none());
        assert!(q.fill.is_some());
        assert!(q.fill_from("PL1", 1, vec![track("c")], false, false).is_none());
        assert!(q.fill.is_none());
        assert_eq!(ids(&q), ["a"]);
        // A song starting means it no longer waits.
        q.replace(vec![track("a")], 0, filling("PL1", 2, false));
        q.advance();
        q.started();
        assert!(!q.fill.as_ref().unwrap().ran_out);
    }

    #[test]
    fn shuffling_up_next_leaves_what_played() {
        let songs: Vec<String> = (0..40).map(|i| i.to_string()).collect();
        let refs: Vec<&str> = songs.iter().map(String::as_str).collect();
        let mut q = queue(&refs, 5, Repeat::Off);
        q.fill = filling("PL1", 1, false);
        assert_eq!(q.shuffle_toggle(), Some(true));
        assert!(q.fill.as_ref().unwrap().shuffled);
        assert_eq!(ids(&q)[..6], refs[..6]);
        let mut rest: Vec<&str> = ids(&q)[6..].to_vec();
        // 34 songs coming back in their order by chance is too unlikely to happen.
        assert_ne!(rest, refs[6..]);
        rest.sort_by_key(|s| s.parse::<u32>().unwrap());
        assert_eq!(rest, refs[6..]);
        // At the last song there's nothing to shuffle.
        let mut q = queue(&["a", "b"], 1, Repeat::Off);
        assert_eq!(q.shuffle_toggle(), None);
    }

    #[test]
    fn shuffling_twice_restores_the_original_order() {
        let songs: Vec<String> = (0..40).map(|i| i.to_string()).collect();
        let refs: Vec<&str> = songs.iter().map(String::as_str).collect();
        let mut q = queue(&refs, 5, Repeat::Off);
        assert_eq!(q.shuffle_toggle(), Some(true));
        assert!(q.shuffled());
        assert_eq!(q.shuffle_toggle(), Some(false));
        assert!(!q.shuffled());
        assert_eq!(ids(&q), refs);
        assert_eq!(q.index, Some(5));
        // The third click shuffles again, with a fresh order to come back to.
        assert_eq!(q.shuffle_toggle(), Some(true));
        assert!(q.shuffle_toggle().is_some());
        assert_eq!(ids(&q), refs);
        // Nothing playing: the whole list shuffles, and comes back.
        let mut q = queue(&["a", "b", "c"], 0, Repeat::Off);
        q.index = None;
        assert_eq!(q.shuffle_toggle(), Some(true));
        assert_eq!(q.shuffle_toggle(), Some(false));
        assert_eq!(ids(&q), ["a", "b", "c"]);
        // A new queue forgets the toggle.
        let mut q = queue(&refs[..10], 2, Repeat::Off);
        assert_eq!(q.shuffle_toggle(), Some(true));
        assert!(q.replace(vec![track("x")], 0, None));
        assert!(!q.shuffled());
    }

    #[test]
    fn unshuffling_keeps_songs_queued_while_shuffled() {
        let mut q = queue(&["a", "b", "c", "d"], 0, Repeat::Off);
        assert_eq!(q.shuffle_toggle(), Some(true));
        // Songs queued by hand land in the shuffled tail; they stay at the end, in order.
        q.play_next(track("x"));
        q.enqueue(track("y"));
        // A song taken out while shuffled doesn't come back.
        let at = q.tracks.iter().position(|t| t.id == "c").unwrap();
        q.remove(at);
        assert_eq!(q.shuffle_toggle(), Some(false));
        assert_eq!(ids(&q), ["a", "b", "d", "x", "y"]);
        assert!(!q.tracks.iter().any(|t| t.id == "c"), "removed song stays out");
        // A song in the queue twice matches once each (play next of the playing song);
        // the hand-queued copy is an extra, so it moves to the end of the restored tail.
        let mut q = queue(&["a", "b"], 0, Repeat::Off);
        assert_eq!(q.shuffle_toggle(), Some(true));
        q.play_next(track("a"));
        assert_eq!(q.shuffle_toggle(), Some(false));
        assert_eq!(ids(&q), ["a", "b", "a"]);
        // The song playing never moves, even at the tail's start.
        let mut q = queue(&["a", "b", "c", "d", "e"], 3, Repeat::Off);
        assert_eq!(q.shuffle_toggle(), Some(true));
        assert_eq!(q.current().unwrap().id, "d");
        assert_eq!(q.shuffle_toggle(), Some(false));
        assert_eq!(q.index, Some(3));
        assert_eq!(ids(&q), ["a", "b", "c", "d", "e"]);
        // A playlist still coming in stops shuffling its next pages.
        let mut q = queue(&["a", "b"], 0, Repeat::Off);
        q.fill = filling("PL1", 1, false);
        assert_eq!(q.shuffle_toggle(), Some(true));
        assert!(q.fill.as_ref().unwrap().shuffled);
        assert_eq!(q.shuffle_toggle(), Some(false));
        assert!(!q.fill.as_ref().unwrap().shuffled);
    }

    #[test]
    fn a_shuffled_playlists_next_pages_mix_in_after_the_next_song() {
        let songs: Vec<String> = (0..40).map(|i| i.to_string()).collect();
        let refs: Vec<&str> = songs.iter().map(String::as_str).collect();
        // The song after the current one stays: it may be loading already.
        let mut q = queue(&refs[..10], 2, Repeat::Off);
        q.fill = filling("PL1", 1, true);
        let more: Vec<Track> = (10..40).map(|i| track(&i.to_string())).collect();
        q.fill_from("PL1", 1, more, true, true).unwrap();
        assert_eq!(ids(&q)[..4], refs[..4]);
        let mut rest: Vec<&str> = ids(&q)[4..].to_vec();
        assert_ne!(rest, refs[4..]);
        rest.sort_by_key(|s| s.parse::<u32>().unwrap());
        assert_eq!(rest, refs[4..]);
    }

    #[test]
    fn a_saved_session_keeps_the_songs_around_the_current_one() {
        let songs: Vec<String> = (0..300).map(|i| i.to_string()).collect();
        let refs: Vec<&str> = songs.iter().map(String::as_str).collect();
        let q = queue(&refs, 120, Repeat::Off);
        let (saved, i) = q.saved().unwrap();
        assert_eq!((saved.len(), i), (200, 50));
        assert_eq!(saved[i].id, "120");
        let q = queue(&refs[..10], 3, Repeat::Off);
        let (saved, i) = q.saved().unwrap();
        assert_eq!((saved.len(), i), (10, 3));
        assert!(Queue::new(Repeat::Off).saved().is_none());
    }
}
