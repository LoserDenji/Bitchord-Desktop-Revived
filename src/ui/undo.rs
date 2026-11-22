//! Undo for what has no button to take it back: songs out of the queue, the queue
//! cleared, songs out of your playlist, a like taken away, a playlist deleted. The toast
//! saying so has the Undo (Ctrl+Z too, while it shows). YouTube Music is only told once
//! the toast goes, the next one comes or the app closes: until then it's all here, and
//! Undo just puts it back as it was.

use crate::backend::Req;
use crate::model::{Card, Target, Track};

use super::lists::{LIKED_MUSIC, Taken, Unliked};
use super::{App, LibTab, Page, queue};

pub enum Undo {
    Queue(queue::Saved),
    /// Likes taken away: the songs liked before, what to put back in Liked Music, and the
    /// request that takes them away there.
    Unlike { liked: Vec<String>, was: Unliked, taste: Vec<(usize, Track)>, send: Req },
    /// Rows taken out of a playlist.
    Removal { id: String, rows: Vec<Taken> },
    /// A playlist deleted, and its card with its place in the library.
    Delete { id: String, card: Option<(usize, Card)> },
}

impl Undo {
    /// What tells YouTube Music, now that it stands.
    fn into_req(self) -> Option<Req> {
        match self {
            Undo::Queue(_) => None,
            Undo::Unlike { send, .. } => Some(send),
            Undo::Removal { id, rows } => Some(Req::RemoveFromPlaylist { items: items(&rows), playlist_id: id }),
            Undo::Delete { id, .. } => Some(Req::DeletePlaylist(id)),
        }
    }
}

/// Rows taken out, as YouTube Music is told of them.
fn items(rows: &[Taken]) -> Vec<(String, String)> {
    rows.iter().map(|t| (t.track.id.clone(), t.set_id.clone().unwrap_or_default())).collect()
}

impl App {
    pub(super) fn toast_undo(&mut self, msg: impl Into<String>, undo: Undo) {
        self.toast(msg);
        self.undo = Some(undo);
    }

    /// The toast's Undo is gone (or about to be): what it could take back stands, and
    /// YouTube Music is told.
    pub(super) fn settle(&mut self) {
        let Some(req) = self.undo.take().and_then(Undo::into_req) else { return };
        // Hidden through loads until YouTube Music lists them gone: it's only told now.
        if let Req::RemoveFromPlaylist { playlist_id, items } = &req {
            self.lists.edits.remove(playlist_id, items.iter().map(|(_, set)| set.clone()));
        }
        self.backend.request(req);
    }

    /// The same, waited for: the app closing, or signing out or switching channel,
    /// can't leave it to go after.
    pub(super) fn settle_now(&mut self) {
        if let Some(req) = self.undo.take().and_then(Undo::into_req) {
            self.backend.request_now(req);
        }
    }

    /// Likes of `tracks` taken away here at once, and from YouTube Music (by `send`) once
    /// the Undo is gone.
    pub(super) fn unlike_soon(&mut self, tracks: Vec<Track>, send: Req) {
        // Save the actual taste entries and their positions; undo isn't a new like.
        let taste = self.listening.liked.iter().enumerate()
            .filter(|(_, t)| !crate::local::is_local(&t.id) && tracks.iter().any(|removed| removed.id == t.id))
            .map(|(i, t)| (i, t.clone()))
            .collect();
        let liked = tracks.iter().filter(|t| self.liked.contains(&t.id)).map(|t| t.id.clone()).collect();
        let was = self.lists.unliking(&tracks);
        self.set_liked(&tracks, false);
        self.toast_undo("Removed from liked songs", Undo::Unlike { liked, was, taste, send });
    }

    /// The like of `id` waits for its Undo to be taken away.
    pub(super) fn unliking_soon(&self, id: &str) -> bool {
        matches!(&self.undo, Some(Undo::Unlike { liked, .. }) if liked.iter().any(|l| l == id))
    }

    /// A re-like cancels the pending unlike (the whole selection for a bulk unlike).
    /// An unlike of other songs is sent before the new rating.
    pub(super) fn rating(&mut self, reliked: impl Fn(&str) -> bool) -> bool {
        let restore = match &self.undo {
            Some(Undo::Unlike { send: Req::Unlike(id), .. }) => reliked(id),
            Some(Undo::Unlike { send: Req::LikeMany { video_ids, .. }, .. }) => video_ids.iter().any(|id| reliked(id)),
            _ => false,
        };
        if restore {
            self.undo();
        } else if matches!(&self.undo, Some(Undo::Unlike { .. })) {
            self.settle();
        }
        restore
    }

    /// Gone from here at once, and from YouTube Music once the Undo is: it can't bring
    /// a deleted playlist back.
    pub(super) fn delete_soon(&mut self, id: String) {
        let card = self.playlist_cards().and_then(|cards| {
            let i = cards.iter().position(|c| matches!(&c.target, Target::Playlist(p) if *p == id))?;
            Some((i, cards.remove(i)))
        });
        self.leave_playlist(&id);
        self.toast_undo("Playlist deleted", Undo::Delete { id, card });
    }

    /// Off a deleted playlist's page, and Back and Forward skip it.
    pub(super) fn leave_playlist(&mut self, id: &str) {
        let gone = Page::Playlist(id.to_owned());
        if self.page == gone {
            self.go(Page::Library(LibTab::Playlists));
        }
        self.back.retain(|p| *p != gone);
        self.back.dedup();
        self.forward.retain(|p| *p != gone);
        self.forward.dedup();
    }

    /// Playlist `id` waits to be deleted: a library load still listing it leaves it out.
    pub(super) fn deleting_soon(&self, id: &str) -> bool {
        matches!(&self.undo, Some(Undo::Delete { id: d, .. }) if d == id)
    }

    pub(super) fn undo(&mut self) {
        let Some(undo) = self.undo.take() else { return };
        match undo {
            Undo::Queue(_) if self.jam.locked() => self.toast("Only the host can change the party"),
            Undo::Queue(saved) => {
                let source = &self.queue_source;
                let Some(go_on) = self.queue.restore(saved, |id| matches!(source, Some(Target::Playlist(p)) if p == id)) else {
                    return self.toast("The queue changed since, so that can't be undone");
                };
                // Its load ended or gave way to another meanwhile: nothing more comes.
                let live = self.queue.fill.as_ref().is_some_and(|f| self.receiving(&f.id) && self.opened(&f.id).is_some_and(|o| o.load == f.load));
                if !live {
                    self.queue.fill = None;
                }
                self.toast = None;
                if go_on {
                    self.next();
                } else {
                    self.prefetch_next();
                }
            }
            Undo::Unlike { liked, was, taste, .. } => {
                self.toast = None;
                self.liked.extend(liked);
                self.lists.unlike_undone(was);
                if !taste.is_empty() {
                    // First-run seeding may have already put some of these back.
                    for (i, t) in taste {
                        let liked = &mut self.listening.liked;
                        if !liked.iter().any(|old| old.id == t.id) {
                            liked.insert(i.min(liked.len()), t);
                        }
                    }
                    self.listening.store(self.backend.data_dir());
                }
                self.after_put_back(LIKED_MUSIC);
            }
            Undo::Removal { id, rows } => {
                self.toast = None;
                self.lists.put_back(&id, rows);
                self.after_put_back(&id);
            }
            Undo::Delete { card, .. } => {
                self.toast = None;
                match (self.playlist_cards(), card) {
                    (Some(cards), Some((i, card))) => cards.insert(i.min(cards.len()), card),
                    // Not among the cards shown: it comes back with the library.
                    _ => self.request_library(),
                }
            }
        }
    }

    /// The playlists the sidebar and Library show: the library's, or last launch's.
    fn playlist_cards(&mut self) -> Option<&mut Vec<Card>> {
        self.library.as_mut().map(|l| &mut l.playlists).or(self.sidebar_cache.as_mut())
    }

    /// Rows came back into `list`: picks (by row number) on its page.
    fn after_put_back(&mut self, list: &str) {
        if self.page_list() == Some(list) {
            self.selection.clear();
        }
    }
}
