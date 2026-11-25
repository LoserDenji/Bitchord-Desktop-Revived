//! What you listen to, kept on this computer: one event per song played, the songs
//! you skip, the ones you've asked out of recommendations, and your liked songs.
//! Home's local rows and the recommendation rows grow from these — the port of
//! Mochy's listening history (its `event` table and `ListeningSignals`).
//!
//! Recording lives on the UI thread (`App::watch_listening`): the player already
//! tells it what's current and playing, so what a song got of actual listening
//! is a per-frame sum there, settled the moment the song stops being current.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use crate::model::Track;

/// One song played: what played, when it started, and how much of it actually did.
/// The taste reads the timestamp (recent plays count more); the local rows read
/// the seconds (most played, forgotten favorites).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Event {
    pub track: Track,
    /// Unix seconds, when the song started playing.
    pub at: u64,
    /// Seconds of it that played (pauses don't count).
    pub played: u32,
}

/// A song left in its first half, kept for a fortnight so it stays out of
/// recommendations; an artist showing up in three of them sits them out too.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Skip {
    pub song: String,
    pub artists: Vec<String>,
    pub at: u64,
}

/// The store, and the answers Home asks of it. One plain JSON file per account in
/// the data dir's `listening/`, and one for signed-out listening, written off the
/// UI thread the way `HomeCache` is.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Listening {
    /// Every play, oldest first; the cap keeps the file small (Mochy's table grows,
    /// but forgotten favorites only reach a few months back anyway).
    pub events: VecDeque<Event>,
    /// The newest `MAX_SKIPS` skips.
    #[serde(default, skip_serializing_if = "VecDeque::is_empty")]
    pub skips: VecDeque<Skip>,
    /// Songs named "Not recommended" from a song's menu; they never come back.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub banned: Vec<String>,
    /// Liked songs, newest first (the taste counts only the newest fifty).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub liked: Vec<Track>,
    /// Whether this account's history and likes were pulled in once, on its first
    /// sign-in here — the store is the account's own, so a different one seeds its
    /// own store, never blocked by this.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub seeded: bool,
    /// What autoplay last served, newest last, kept so fresh recommendations don't
    /// hand the same songs straight back. A played one leaves by becoming heard, a
    /// skipped one by its skip; the rest age out of the cap.
    #[serde(default, skip_serializing_if = "VecDeque::is_empty")]
    pub recent_reco: VecDeque<String>,
    /// Plays and skips since Home's recommendation rows were last rebuilt; crossing
    /// either threshold refreshes them (see `refresh_due`).
    #[serde(skip)]
    plays_since_refresh: u32,
    #[serde(skip)]
    skips_since_refresh: u32,
}

/// A skip older than this is forgotten.
const SKIP_DAYS: u64 = 14;
const MAX_EVENTS: usize = 4000;
const MAX_SKIPS: usize = 200;
/// How long what autoplay served stays out of recommendations; a radio asks far
/// less often than it plays through its picks, so a handful of landings covers it.
const MAX_RECENT_RECO: usize = 60;

/// The skip test, as Mochy's `onTrackLeft` runs it: a song dropped in its first
/// half and half a minute. Queued but never heard (<0.3 s), and anything too short
/// to say something about (a jingle), count for neither a skip nor a play.
fn noted(listened: f32, duration: Option<u32>) -> Option<bool> {
    if listened < 0.3 {
        return None;
    }
    let Some(d) = duration else { return Some(listened < 30.0) };
    if d < 40 {
        return None;
    }
    Some(listened < 30.0 && listened < d as f32 / 2.0)
}

impl Listening {
    /// A song stopped being current after `listened` seconds of actual playing.
    /// Either an event for it (a play) or a skip, per the rule above.
    pub fn note_left(&mut self, track: Track, started_at: u64, listened: f32) {
        match noted(listened, track.duration) {
            Some(true) => {
                self.skips.push_back(Skip { song: track.id, artists: track.artists.iter().map(|a| a.name.clone()).collect(), at: started_at });
                while self.skips.len() > MAX_SKIPS {
                    self.skips.pop_front();
                }
                self.skips_since_refresh += 1;
            }
            Some(false) => {
                self.events.push_back(Event { track, at: started_at, played: listened.max(0.0) as u32 });
                while self.events.len() > MAX_EVENTS {
                    self.events.pop_front();
                }
                self.plays_since_refresh += 1;
            }
            None => {}
        }
        self.prune();
    }

    /// A song was named "Not recommended": it and its skips leave.
    pub fn ban(&mut self, id: &str) {
        if !self.banned.iter().any(|b| b == id) {
            self.banned.push(id.to_owned());
        }
        self.skips.retain(|s| s.song != id);
        self.events.retain(|e| e.track.id != id);
    }

    /// An unlike takes the song out of the likes the taste reads.
    pub fn unliked(&mut self, id: &str) {
        self.liked.retain(|t| t.id != id);
    }

    /// A like counts newest first.
    pub fn liked(&mut self, t: Track) {
        self.liked.retain(|old| old.id != t.id);
        self.liked.insert(0, t);
        self.liked.truncate(50);
    }

    fn prune(&mut self) {
        let since = unix_now() - SKIP_DAYS * 86_400;
        self.skips.retain(|s| s.at >= since);
    }

    /// The songs to keep out of recommendations: named ones, and the recently skipped.
    pub fn avoid_songs(&self) -> Vec<String> {
        let mut out = self.banned.clone();
        for s in &self.skips {
            if !out.contains(&s.song) {
                out.push(s.song.clone());
            }
        }
        out
    }

    /// Artists kept out of recommendations: three or more recent skips each.
    pub fn avoid_artists(&self) -> Vec<String> {
        let mut counts: std::collections::HashMap<String, u32> = Default::default();
        for s in &self.skips {
            for a in &s.artists {
                *counts.entry(a.to_lowercase()).or_default() += 1;
            }
        }
        counts.into_iter().filter(|(_, n)| *n >= 3).map(|(a, _)| a).collect()
    }

    /// The most played songs in the last `days`, by seconds listened; `offset`
    /// skips the very top, so "Keep listening" doesn't repeat Most played's head.
    pub fn most_played(&self, days: u64, offset: usize, limit: usize) -> Vec<Track> {
        let since = unix_now() - days * 86_400;
        // Seconds listened is the weight: the way Mochy's play-count query sums playTime.
        let mut sums: Vec<(&Track, u64)> = Default::default();
        let mut by_id: std::collections::HashMap<&str, usize> = Default::default();
        for e in self.events.iter().filter(|e| e.at >= since) {
            let i = *by_id.entry(e.track.id.as_str()).or_insert(sums.len());
            if i == sums.len() {
                sums.push((&e.track, e.played as u64));
            } else {
                sums[i].1 += e.played as u64;
            }
        }
        sums.sort_by(|a, b| b.1.cmp(&a.1));
        sums.into_iter().skip(offset).take(limit).map(|(t, _)| t.clone()).collect()
    }

    /// The artists of what played in the last `days`, by seconds listened, with
    /// each artist's own share.
    pub fn top_artists(&self, days: u64, limit: usize) -> Vec<(crate::model::Link, u64)> {
        let since = unix_now() - days * 86_400;
        let mut sums: std::collections::HashMap<String, (crate::model::Link, u64)> = Default::default();
        for e in self.events.iter().filter(|e| e.at >= since) {
            if let Some(a) = e.track.artists.iter().find(|a| a.id.is_some()) {
                let slot = sums.entry(a.id.clone().unwrap_or_default()).or_insert((a.clone(), 0));
                slot.1 += e.played as u64;
            }
        }
        let mut out: Vec<_> = sums.into_values().collect();
        out.sort_by(|a, b| b.1.cmp(&a.1));
        out.into_iter().take(limit).collect()
    }

    /// The albums of what played in the last `days`, by seconds listened, a cover
    /// off one of their songs; `offset` skips the very top, as the songs' do.
    pub fn most_played_albums(&self, days: u64, offset: usize, limit: usize) -> Vec<(crate::model::Link, Vec<crate::model::Thumb>)> {
        let since = unix_now() - days * 86_400;
        let mut sums: Vec<(crate::model::Link, u64, Vec<crate::model::Thumb>)> = Default::default();
        let mut by_id: std::collections::HashMap<&str, usize> = Default::default();
        for e in self.events.iter().filter(|e| e.at >= since) {
            let Some(album) = e.track.album.as_ref().filter(|a| a.id.is_some()) else { continue };
            let i = *by_id.entry(album.id.as_deref().unwrap_or_default()).or_insert(sums.len());
            if i == sums.len() {
                sums.push((album.clone(), e.played as u64, e.track.thumbs.clone()));
            } else {
                sums[i].1 += e.played as u64;
            }
        }
        sums.sort_by(|a, b| b.1.cmp(&a.1));
        sums.into_iter().skip(offset).take(limit).map(|(a, _, t)| (a, t)).collect()
    }

    /// Old favourites drifted from: what you played a lot of before the last month
    /// is five times what you've played of it since. Mochy's forgotten-favourites query.
    pub fn forgotten_favorites(&self) -> Vec<Track> {
        let month_ago = unix_now() - 30 * 86_400;
        let mut old: std::collections::HashMap<&str, u64> = Default::default();
        let mut recent: std::collections::HashMap<&str, u64> = Default::default();
        let mut song: std::collections::HashMap<&str, &Track> = Default::default();
        for e in &self.events {
            song.entry(e.track.id.as_str()).or_insert(&e.track);
            let slot = if e.at < month_ago { &mut old } else { &mut recent };
            *slot.entry(e.track.id.as_str()).or_default() += e.played as u64;
        }
        let mut out: Vec<Track> = old
            .into_iter()
            .filter(|(id, before)| 0.2 * *before as f64 > recent.get(id).copied().unwrap_or(0) as f64)
            .filter_map(|(id, _)| song.get(id).map(|t| (*t).clone()))
            .collect();
        out.truncate(100);
        out
    }

    /// The last songs played, each once, newest first.
    pub fn recently_played(&self, limit: usize) -> Vec<Track> {
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for e in self.events.iter().rev() {
            if seen.insert(e.track.id.clone()) {
                out.push(e.track.clone());
                if out.len() == limit {
                    break;
                }
            }
        }
        out
    }

    /// Whether Home's rows grew stale behind the listener: two skips or four plays
    /// since they were last asked for.
    pub fn refresh_due(&self) -> bool {
        self.skips_since_refresh >= 2 || self.plays_since_refresh >= 4
    }

    pub fn mark_refreshed(&mut self) {
        self.plays_since_refresh = 0;
        self.skips_since_refresh = 0;
    }

    /// What autoplay just served: each sits out of recommendations a while (see
    /// `recent_reco`), so the queue and Home's rows don't hand back what the last
    /// one just did. Not one per user-started radio — the listener chose to hear it.
    pub fn note_reco(&mut self, tracks: &[Track]) {
        let fresh: Vec<String> = tracks.iter().map(|t| t.id.clone()).filter(|id| !self.recent_reco.contains(id)).collect();
        self.recent_reco.extend(fresh);
        while self.recent_reco.len() > MAX_RECENT_RECO {
            self.recent_reco.pop_front();
        }
    }

    /// First run with an account: history stands in for events (a play each, spread
    /// back over the window so decay reads them as recent), likes come newest first.
    pub fn seed_from(&mut self, history: Vec<Track>, liked: Vec<Track>) {
        if self.seeded {
            return;
        }
        let at = unix_now() - 1;
        for (i, t) in history.into_iter().enumerate() {
            // One event each: the list only names songs, not how often they played.
            self.events.push_back(Event { at: at - i as u64 * 60, played: t.duration.unwrap_or(180), track: t });
        }
        while self.events.len() > MAX_EVENTS {
            self.events.pop_front();
        }
        self.liked = liked.into_iter().take(50).collect();
        self.seeded = true;
    }

    /// Read the store for `account` from the data dir; missing or unreadable is an
    /// empty one (a fresh install, or an older version's file that didn't parse).
    pub fn load(dir: &Path, account: Option<&str>) -> Self {
        take_over(dir, account);
        std::fs::read(file(dir, account))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// Write it off the UI thread: a later write always lands, and a half-written
    /// file never shows (the `HomeCache::store` dance).
    pub fn store(&self, dir: &Path, account: Option<&str>) {
        static STARTED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        static WRITTEN: std::sync::Mutex<u64> = std::sync::Mutex::new(0);
        let n = STARTED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        let path = file(dir, account);
        let json = serde_json::to_vec(self).unwrap_or_default();
        std::thread::spawn(move || {
            let mut written = WRITTEN.lock().unwrap_or_else(|e| e.into_inner());
            if *written > n {
                return;
            }
            *written = n;
            if let Some(parent) = path.parent() {
                // A scope's first write can be the one that adopts the store.
                let _ = std::fs::create_dir_all(parent);
            }
            let tmp = path.with_extension("json.tmp");
            let _ = std::fs::write(&tmp, json).and_then(|_| std::fs::rename(&tmp, &path));
        });
    }
}

/// Where the stores live: one per signed-in account, named for the one-way key of
/// its session's SAPISID cookie (`webmusic::Session::scope_key`), and one for
/// signed-out listening beside them. What was listened to as one account is that
/// account's taste, so switching accounts switches the store the rows grow from,
/// and signing out leaves the machine's own.
const DIR: &str = "listening";
/// What a version before per-account stores kept: every account's listening in one
/// flat file, which the first scope to load with none of its own takes over — so a
/// listener's existing history isn't lost to the change.
const PRE_ACCOUNT: &str = "listening.json";

/// The store's own file: the account's key names it, and signed-out listening has
/// `out` beside the accounts'. An unusable key (empty, say) falls in with
/// signed-out rather than naming a file after nothing.
fn file(dir: &Path, account: Option<&str>) -> PathBuf {
    let name = match account {
        Some(key) if !key.is_empty() => format!("{key}.json"),
        _ => "out.json".to_owned(),
    };
    dir.join(DIR).join(name)
}

/// The flat store from before the accounts had their own, moved to where `account`'s
/// belongs — once: it's gone afterwards, so every later scope, signed-out included,
/// starts a store of its own.
fn take_over(dir: &Path, account: Option<&str>) {
    let (old, new) = (dir.join(PRE_ACCOUNT), file(dir, account));
    if old.exists()
        && !new.exists()
        && let Some(parent) = new.parent()
    {
        let _ = std::fs::create_dir_all(parent).and_then(|_| std::fs::rename(&old, &new));
    }
}

pub fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Link, Thumb};

    fn song(id: &str, artist: &str, duration: Option<u32>) -> Track {
        Track { id: id.into(), title: id.into(), artists: vec![Link { name: artist.into(), id: Some(artist.into()) }], duration, thumbs: vec![], ..Default::default() }
    }

    fn at(days_ago: f64) -> u64 {
        unix_now() - (days_ago * 86_400.0) as u64
    }

    #[test]
    fn a_song_left_early_is_a_skip() {
        let mut l = Listening::default();
        let t = song("a", "X", Some(200));
        l.note_left(t.clone(), at(0.0), 5.0);
        assert!(l.events.is_empty());
        assert_eq!(l.skips.len(), 1);
        assert_eq!(l.avoid_songs(), vec!["a".to_owned()]);
    }

    #[test]
    fn a_song_left_late_is_a_play() {
        let mut l = Listening::default();
        l.note_left(song("a", "X", Some(200)), at(0.0), 40.0);
        assert_eq!(l.events.len(), 1);
        assert_eq!(l.events[0].played, 40);
    }

    #[test]
    fn short_songs_and_never_heard_ones_count_for_neither() {
        let mut l = Listening::default();
        l.note_left(song("jingle", "X", Some(12)), at(0.0), 8.0);
        l.note_left(song("touched", "X", Some(200)), at(0.0), 0.1);
        assert!(l.events.is_empty() && l.skips.is_empty());
    }

    #[test]
    fn skipping_an_artist_three_times_sits_them_out() {
        let mut l = Listening::default();
        for i in 0..3 {
            l.note_left(song(&format!("s{i}"), "Skipme", Some(200)), at(0.0), 4.0);
        }
        assert_eq!(l.avoid_artists(), vec!["skipme".to_owned()]);
    }

    #[test]
    fn most_played_sums_seconds_within_the_window() {
        let mut l = Listening::default();
        l.note_left(song("old", "X", Some(100)), at(40.0), 90.0);
        for _ in 0..3 {
            l.note_left(song("new", "X", Some(100)), at(1.0), 60.0);
        }
        l.note_left(song("newer", "X", Some(100)), at(1.0), 100.0);
        let ids: Vec<String> = l.most_played(30, 0, 3).into_iter().map(|t| t.id).collect();
        assert_eq!(ids, vec!["new".to_owned(), "newer".to_owned()]);
        // Offset skips the very top: what "Keep listening" wants.
        let ids: Vec<String> = l.most_played(30, 1, 3).into_iter().map(|t| t.id).collect();
        assert_eq!(ids, vec!["newer".to_owned()]);
    }

    #[test]
    fn forgotten_favorites_are_the_drifted_from() {
        let mut l = Listening::default();
        for _ in 0..10 {
            l.note_left(song("fav", "X", Some(100)), at(45.0), 90.0);
        }
        l.note_left(song("fav", "X", Some(100)), at(2.0), 30.0);
        l.note_left(song("current", "X", Some(100)), at(2.0), 90.0);
        let ids: Vec<String> = l.forgotten_favorites().into_iter().map(|t| t.id).collect();
        assert_eq!(ids, vec!["fav".to_owned()]);
    }

    #[test]
    fn banning_a_song_takes_its_events_and_skips_out() {
        let mut l = Listening::default();
        l.note_left(song("a", "X", Some(200)), at(1.0), 90.0);
        l.note_left(song("a", "X", Some(200)), at(0.0), 4.0);
        l.ban("a");
        assert!(l.events.is_empty() && l.skips.is_empty() && l.avoid_songs().len() == 1);
    }

    #[test]
    fn seeding_from_history_spreads_events_back_without_reseeding() {
        let mut l = Listening::default();
        l.seed_from(vec![song("h1", "X", None), song("h2", "X", None)], vec![song("l1", "X", None)]);
        assert!(l.seeded);
        let before = l.events.len();
        l.seed_from(vec![song("h3", "X", None)], vec![]);
        assert_eq!(l.events.len(), before);
        assert_eq!(l.liked.len(), 1);
    }

    #[test]
    fn likes_come_newest_first_and_cap_at_fifty() {
        let mut l = Listening::default();
        for i in 0..60 {
            l.liked(song(&format!("l{i}"), "X", None));
        }
        assert_eq!(l.liked.len(), 50);
        assert_eq!(l.liked[0].id, "l59");
        l.liked(song("l59", "X", None)); // again, still once
        l.unliked("l58");
        assert!(!l.liked.iter().any(|t| t.id == "l58"));
    }

    #[test]
    fn what_autoplay_serves_sits_out_of_recommendations_a_while() {
        let mut l = Listening::default();
        l.note_reco(&[song("r1", "X", None), song("r2", "X", None)]);
        l.note_reco(&[song("r2", "X", None), song("r3", "X", None)]); // r2 once only
        assert_eq!(l.recent_reco, vec!["r1".to_owned(), "r2".to_owned(), "r3".to_owned()]);
        for i in 0..70 {
            l.note_reco(&[song(&format!("n{i}"), "X", None)]);
        }
        assert_eq!(l.recent_reco.len(), 60, "the cap holds");
        assert!(!l.recent_reco.contains(&"r1".to_owned()), "the oldest aged out");
        // what a round trip carries
        let back: Listening = serde_json::from_slice(&serde_json::to_vec(&l).unwrap()).unwrap();
        assert_eq!(back.recent_reco.len(), 60);
    }

    #[test]
    fn refresh_crosses_at_two_skips_or_four_plays() {
        let mut l = Listening::default();
        l.note_left(song("a", "X", Some(200)), at(0.0), 4.0);
        l.note_left(song("b", "X", Some(200)), at(0.0), 4.0);
        assert!(l.refresh_due());
        l.mark_refreshed();
        assert!(!l.refresh_due());
        for i in 0..4 {
            l.note_left(song(&format!("p{i}"), "X", Some(200)), at(0.0), 60.0);
        }
        assert!(l.refresh_due());
    }

    #[test]
    fn a_track_with_a_thumb_survives_the_round_trip() {
        let mut l = Listening::default();
        l.note_left(Track { id: "x".into(), title: "X".into(), artists: vec![], thumbs: vec![Thumb { url: "https://example/t".into(), width: 120, height: 120 }], duration: Some(100), ..Default::default() }, at(0.0), 50.0);
        let json = serde_json::to_vec(&l).unwrap();
        let back: Listening = serde_json::from_slice(&json).unwrap();
        assert_eq!(back.events.len(), 1);
        assert_eq!(back.events[0].track.thumbs[0].url, "https://example/t");
    }

    #[test]
    fn each_scope_keeps_its_own_store_and_the_flat_one_is_taken_over_once() {
        let dir = std::env::temp_dir().join(format!("bitchord-listening-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // One store per scope: an account's is its own file, signed-out another.
        assert_eq!(file(&dir, Some("a1b2")), dir.join(DIR).join("a1b2.json"));
        assert_eq!(file(&dir, None), dir.join(DIR).join("out.json"));
        // The flat store from before them is taken over by the first scope with none
        // of its own — here the signed-in account — and by no scope after that.
        let mut l = Listening::default();
        l.note_left(song("a", "X", Some(200)), at(1.0), 90.0);
        l.seeded = true;
        std::fs::write(dir.join(PRE_ACCOUNT), serde_json::to_vec(&l).unwrap()).unwrap();
        let a = Listening::load(&dir, Some("a1b2"));
        assert_eq!(a.events.len(), 1);
        assert!(!dir.join(PRE_ACCOUNT).exists());
        // A different account, and signed out: each starts empty, not seeded by A's.
        assert!(Listening::load(&dir, Some("c3d4")).events.is_empty());
        let out = Listening::load(&dir, None);
        assert!(out.events.is_empty() && !out.seeded);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
