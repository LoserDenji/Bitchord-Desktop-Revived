//! Network side: a small tokio runtime talking to YouTube Music (`ytmapi`/`ytm`).
//!
//! The UI sends `Req`s and drains `Resp`s once per frame; every response wakes the UI.
//! Playback resolution also lives here: resolve stream → start progressive download →
//! build the decoder on a blocking thread → hand it to the audio engine.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use anyhow::Context;
use crossbeam_channel::Sender;
use tokio::runtime::Runtime;

use crate::art::Art;
use crate::audio::{AudioCmd, AudioHandle};
use crate::lyrics::{self, LyricsSet};
use crate::model::{self, Card, Target, Thumb, Track, sized};
use crate::stream::{self, Shared};
use crate::{hires, innertube, jsc, providers, webmusic, ytdlp, ytm, ytmapi};

pub enum Req {
    /// Home feed, optionally filtered by a mood chip's params.
    Home(Option<String>),
    HomeMore(String),
    MoreQuickPicks(String),
    /// The account's history and its fifty newest likes, to start the local taste
    /// from on a first run (see `crate::listening`).
    SeedTaste,
    /// Home's "Because you like <song>" rows: each seed song's radio, picked. The
    /// taste parts travel along — the picking runs here, the listening lives on
    /// the UI thread.
    BecauseYouLike { seeds: Vec<Track>, taste: crate::recommend::TasteLite },
    /// Home's "More like <artist>" rows: each artist's radio minus the artist,
    /// one song per act. `artists`: (browse id, name), three of the top eight.
    MoreLike { artists: Vec<(String, String)>, taste: crate::recommend::TasteLite },
    /// Home's genre rows: the ones your top artists are filed under — iTunes knows
    /// a genre, YouTube doesn't — and a few you don't play yet. `artists`:
    /// (name, seconds listened) of the top twelve.
    GenreRows { artists: Vec<(String, u64)>, taste: crate::recommend::TasteLite },
    /// This year's and last's releases from the artists you play most, their ids.
    NewFromArtists(Vec<String>),
    /// The hero's discoveries: one unheard song (if there is one) off each seed
    /// song's radio, with what grew it.
    DailyDiscover { seeds: Vec<Track>, taste: crate::recommend::TasteLite },
    /// "All time hits": the charts page's most played.
    AllTimeHits,
    /// "From the community": playlists off the pages of `artists` — (browse id,
    /// name), three of the top ten — that aren't YouTube's own.
    CommunityPlaylists(Vec<(String, String)>),
    /// "Live shows" and "Music videos for you": the video shelves off the pages
    /// of `artists` — (browse id, name), the six most listened.
    ArtistVideos(Vec<(String, String)>),
    Explore(Vec<String>),
    /// A Moods & genres category, by its browse params.
    Mood(String),
    Search(String),
    /// A search of only your library or only your uploads.
    SearchIn { query: String, scope: ytm::Scope },
    /// Suggestions for a query; `round` comes back with them, so stale ones can be dropped.
    Suggest { query: String, round: u64 },
    /// Remove a past search (its token) from the account's history.
    ForgetSearch(String),
    /// `load` comes back with the album, so an older answer can't replace a newer one.
    Album { id: String, load: u64 },
    Artist(String),
    /// A shelf's Show all: the artist's full discography for that shelf, as YouTube
    /// lists it behind its own More button (browseId and its params). `artist` and
    /// `title` come back with the answer, so a stale one can't fill a newer shelf.
    ArtistShelf { browse_id: String, params: String, artist: String, title: String },
    /// `stream`: send the first page as soon as it's in and the rest as they come,
    /// rather than all of it at the end.
    Playlist { id: String, load: u64, stream: bool },
    Library,
    History,
    /// Songs the account uploaded to YouTube Music.
    Uploads,
    /// The signed-in account's name and photo.
    Account,
    /// The channels the signed-in Google account can act as.
    Channels,
    /// Act as another of them from now on: a brand channel's page id, or `None` for the
    /// account's own channel. `epoch`: the list's, picked from.
    SwitchChannel { page_id: Option<String>, epoch: u64 },
    /// The queue another device left off with (YouTube Music's own cross-device queue).
    ServerQueue,
    /// Upload audio files to the account's library.
    Upload(Vec<PathBuf>),
    /// Save a song's audio to the downloads folder. `for_video`: the music video it's the
    /// song version of, which then plays it from the file (offline too).
    Download { track: Track, for_video: Option<String> },
    /// Download the songs of an album or playlist (`target`; fetched whole when `tracks`
    /// is `None`) or of a selection (`target` `None`). Songs already downloaded are
    /// skipped. More than `LARGE_DOWNLOAD` to go asks first (`Resp::ConfirmDownload`)
    /// unless `confirmed`.
    DownloadMany { target: Option<Target>, title: String, tracks: Option<Vec<Track>>, confirmed: bool },
    Lyrics(Track),
    /// SponsorBlock's parts of a video to skip.
    Segments { video_id: String },
    /// Romanization and translation for the lyric lines of one source.
    LyricsExtras { video_id: String, source: String, lines: Vec<String>, lang: Option<String>, target: String },
    /// Process cover art for the background; `key` is echoed back.
    Art { key: String, url: String },
    /// The animated cover of a song, the way ytm-rs finds it: Apple Music's clip for
    /// its release, with the lookup and its answer cached on disk.
    Canvas { track: Track },
    /// Spotify's canvas for the playing song; `key` is what the UI asked for.
    SpotifyCanvas { key: String, title: String, artist: String, duration_ms: Option<u64> },
    /// A handwritten signature for `name` from the service at `endpoint`; `key` is
    /// what the UI asked for.
    ArtistSignature { key: String, endpoint: String, name: String },
    /// Radio seeded from a track (`seed`, its id); appended to the queue. `song_of`: the
    /// track, when it's a music video to play as its song: the song's radio is asked for.
    /// The taste travels along — what the listening has ruled out never lines up around
    /// a song the listener chose, though the choice itself plays whatever it is.
    Radio { seed: String, song_of: Option<Track>, taste: crate::recommend::TasteLite },
    /// The queue topping itself up at its end: the same fetch and cascade as `Radio`,
    /// but picked rather than kept in order — autoplay grows from the taste the way
    /// Home's rows do, and what it serves is noted back into it.
    AutoRadio { seed: String, song_of: Option<Track>, taste: crate::recommend::TasteLite },
    /// A song's first page of comments (now playing's Comments panel).
    Comments { video_id: String },
    /// The next page of a comments list (more comments, a comment's replies, another
    /// sort), from its `next` continuation token.
    CommentsPage { token: String },
    /// A comment call as the account: `comment/create_comment`, `comment/create_comment_reply`
    /// or `comment/perform_comment_action` (like, unlike). `tag` comes back as the
    /// `CommentsPage` token.
    CommentAction { endpoint: String, body: serde_json::Value, tag: String },
    /// Fetch an album/playlist and play it from the start.
    PlayCollection { target: Target },
    /// `at`: start position (non-zero when resuming last session's song). `report`: tell
    /// YouTube about the play (not when picking up a song already counted). `song_of`:
    /// the track, when it's a music video to play as its song version if one is found.
    /// `song`: the song version already playing for the music video `video_id` (picked up
    /// mid-song): played without a lookup, and the video if it won't. `track`: the song as
    /// the UI lists it, for a hi-res source's search.
    Play { video_id: String, generation: u64, at: std::time::Duration, report: bool, song_of: Option<Track>, song: Option<Track>, track: Option<Track> },
    /// Resolve and download a track ahead of time so skipping to it is instant.
    Prefetch { video_id: String, song_of: Option<Track>, track: Option<Track> },
    /// A video's stream, for showing it: the tallest at most `max_height`.
    VideoStream { video_id: String, max_height: u32 },
    /// A song's music video, for Now playing's Song/Video switch.
    MusicVideo(Track),
    Like { video_id: String, like: bool },
    /// Take away just this like, not the other half's.
    Unlike(String),
    /// Several likes (or unlikes) one after another, for a selection.
    LikeMany { video_ids: Vec<String>, like: bool },
    CreatePlaylist { title: String, video_ids: Vec<String> },
    EditPlaylist { id: String, title: String, description: String, privacy: Option<String> },
    AddToPlaylist { playlist_id: String, video_ids: Vec<String> },
    /// `(videoId, setVideoId)` entries to take out of the playlist.
    RemoveFromPlaylist { playlist_id: String, items: Vec<(String, String)> },
    DeletePlaylist(String),
    /// A playlist's collaboration settings and invite link.
    Collaboration(String),
    SetCollaboration { info: ytm::Collaboration, change: ytm::CollabChange },
    SaveCollection { id: String, save: bool },
    /// Save one of your playlist's sort orders to it ([`PlaylistSort::actions`]).
    SortPlaylist { id: String, actions: serde_json::Value },
    Subscribe { channel_id: String, on: bool },
    SetCookie(String),
    Logout,
}

impl Req {
    fn name(&self) -> &'static str {
        match self {
            Req::Home(_) | Req::HomeMore(_) => "Loading home",
            Req::MoreQuickPicks(_) => "Loading more picks",
            Req::SeedTaste => "Loading your taste",
            Req::BecauseYouLike { .. } => "Growing your song rows",
            Req::MoreLike { .. } => "Growing your artist rows",
            Req::GenreRows { .. } => "Growing your genre rows",
            Req::NewFromArtists(_) => "Loading new releases",
            Req::DailyDiscover { .. } => "Growing the hero's picks",
            Req::AllTimeHits => "Loading all-time hits",
            Req::CommunityPlaylists(_) => "Finding community playlists",
            Req::ArtistVideos(_) => "Finding live shows and videos",
            Req::Explore(_) => "Loading new",
            Req::Mood(_) => "Loading the category",
            Req::Search(_) | Req::SearchIn { .. } => "Search",
            Req::Suggest { .. } => "Suggest",
            Req::ForgetSearch(_) => "Removing the search",
            Req::Album { .. } => "Loading album",
            Req::Artist(_) => "Loading artist",
            Req::ArtistShelf { .. } => "Loading the artist's releases",
            Req::Playlist { .. } => "Loading playlist",
            Req::Library => "Loading library",
            Req::History => "Loading history",
            Req::Uploads => "Loading uploads",
            Req::Account => "Account",
            Req::Channels => "Channels",
            Req::SwitchChannel { .. } => "Switching channel",
            Req::ServerQueue => "Queue from other devices",
            Req::Upload(_) => "Upload",
            Req::Download { .. } | Req::DownloadMany { .. } => "Download",
            Req::Lyrics(_) => "Lyrics",
            Req::Segments { .. } => "SponsorBlock",
            Req::LyricsExtras { .. } => "Translation",
            Req::Art { .. } => "Art",
            Req::Canvas { .. } => "Animated cover",
            Req::SpotifyCanvas { .. } => "Spotify canvas",
            Req::ArtistSignature { .. } => "Artist signature",
            Req::Radio { .. } | Req::AutoRadio { .. } => "Loading radio",
            Req::Comments { .. } | Req::CommentsPage { .. } | Req::CommentAction { .. } => "Comments",
            Req::PlayCollection { .. } => "Playback",
            Req::Play { .. } => "Playback",
            Req::Prefetch { .. } => "Prefetch",
            Req::VideoStream { .. } | Req::MusicVideo(_) => "Video",
            Req::Like { .. } | Req::LikeMany { .. } | Req::Unlike(_) => "Like",
            Req::CreatePlaylist { .. } => "Create playlist",
            Req::EditPlaylist { .. } => "Edit playlist",
            Req::AddToPlaylist { .. } => "Add to playlist",
            Req::RemoveFromPlaylist { .. } => "Remove from playlist",
            Req::DeletePlaylist(_) => "Delete playlist",
            Req::Collaboration(_) => "Loading collaboration",
            Req::SetCollaboration { .. } => "Collaboration",
            Req::SaveCollection { .. } => "Save to library",
            Req::SortPlaylist { .. } => "Sort playlist",
            Req::Subscribe { .. } => "Subscribe",
            Req::SetCookie(_) => "Sign in",
            Req::Logout => "Sign out",
        }
    }

    /// Background requests fail quietly (logged, no toast).
    fn quiet(&self) -> bool {
        matches!(self, Req::Suggest { .. } | Req::Account | Req::Channels | Req::ServerQueue | Req::Art { .. } | Req::Canvas { .. } | Req::SpotifyCanvas { .. } | Req::ArtistSignature { .. } | Req::Prefetch { .. } | Req::HomeMore(_) | Req::SeedTaste | Req::BecauseYouLike { .. } | Req::MoreLike { .. } | Req::GenreRows { .. } | Req::NewFromArtists(_) | Req::DailyDiscover { .. } | Req::AllTimeHits | Req::CommunityPlaylists(_) | Req::ArtistVideos(_) | Req::MusicVideo(_) | Req::AutoRadio { .. } | Req::Lyrics(_) | Req::Segments { .. } | Req::LyricsExtras { .. } | Req::Comments { .. } | Req::CommentsPage { .. } | Req::CommentAction { .. })
    }
}

pub struct Explore {
    pub new_albums: Vec<Card>,
    pub charts: Vec<Card>,
    pub top: Vec<Track>,
    /// Moods & genres: the categories it opens.
    pub moods: Vec<ytm::MoodGroup>,
}

pub struct SearchResults {
    pub query: String,
    /// All of YouTube Music, or just your library or your uploads.
    pub scope: ytm::Scope,
    pub tracks: Vec<Track>,
    /// Videos, including uploads that have no song version (remixes, live sets).
    pub videos: Vec<Track>,
    pub albums: Vec<Card>,
    pub artists: Vec<Card>,
    pub playlists: Vec<Card>,
    /// Podcast shows and episodes, when podcasts are shown.
    pub podcasts: Vec<Card>,
    pub episodes: Vec<Track>,
}

/// An album or playlist page.
pub struct Collection {
    pub id: String,
    pub is_album: bool,
    /// Album's playlist ID for saving it to the account library.
    pub save_id: Option<String>,
    pub title: String,
    pub subtitle: String,
    /// Album artists / playlist owner named in the subtitle, clickable.
    pub artists: Vec<model::Link>,
    pub description: Option<String>,
    pub thumbs: Vec<Thumb>,
    pub tracks: Vec<Track>,
    /// Each track's playlist entry id (`setVideoId`), for removing it; empty for albums.
    pub set_ids: Vec<Option<String>>,
    /// How many songs the playlist's header counts (unplayable ones too, so it can be more
    /// than `tracks`); kept in step with removals and unlikes made here.
    pub count: Option<u64>,
    /// The account's own playlist (editable), not one saved from someone else.
    pub owned: bool,
    /// Your own playlist's sort orders, from YouTube Music's menu. Empty for anything else.
    pub sorts: Vec<PlaylistSort>,
}

/// One of YouTube Music's orders for your own playlist. Picking one saves it to the
/// playlist, as YouTube Music does, so it loads in that order everywhere. Manual ordering
/// brings the custom order back as it was.
pub struct PlaylistSort {
    pub name: String,
    /// The order it loads in now.
    pub on: bool,
    /// The custom order, the one songs can be dragged around in.
    pub manual: bool,
    /// What picking it sends to `browse/edit_playlist`.
    pub actions: serde_json::Value,
}

impl Collection {
    /// A podcast show: its page lists episodes.
    pub fn is_show(&self) -> bool {
        ytm::list_id(&self.id) != self.id
    }

    /// "songs", or what this lists instead.
    pub fn things(&self) -> &'static str {
        if self.is_show() { "episodes" } else { "songs" }
    }

    /// The subtitle, with the song count for a playlist.
    pub fn byline(&self) -> String {
        let things = self.things();
        match self.count {
            Some(n) if self.subtitle.is_empty() => format!("{n} {things}"),
            Some(n) => format!("{} • {n} {things}", self.subtitle),
            None => self.subtitle.clone(),
        }
    }

    /// Take rows out (sorted indices), and out of the count.
    pub fn remove_rows(&mut self, rows: &[usize]) {
        for &i in rows.iter().rev() {
            if i < self.tracks.len() {
                self.tracks.remove(i);
                if i < self.set_ids.len() {
                    self.set_ids.remove(i);
                }
                self.count = self.count.map(|n| n.saturating_sub(1));
            }
        }
    }
}

pub struct ArtistPage {
    pub id: String,
    pub name: String,
    pub banner: Vec<Thumb>,
    /// The channel's own square picture (the microformat's), for the About card.
    pub avatar: Vec<Thumb>,
    pub subscribers: Option<u64>,
    /// The account subscribes to it (false signed out).
    pub subscribed: bool,
    /// What subscribing takes; it isn't always the page's id.
    pub channel_id: Option<String>,
    /// When it was asked for: a subscribe change answered since outranks `subscribed`.
    pub asked: std::time::Instant,
    pub description: Option<String>,
    pub top: Vec<Track>,
    /// "Top songs" playlist behind the shelf's Show all button.
    pub top_playlist: Option<String>,
    pub albums: Vec<Card>,
    /// Their songs in your library ("From your library"); signed in only.
    pub library: Vec<Card>,
    /// The "Singles & EPs" shelf, kept apart from the albums since BitChord's
    /// artist page gives each its own Show all.
    pub singles: Vec<Card>,
    pub playlists: Vec<Card>,
    pub similar: Vec<Card>,
    /// The videos and live performances shelves' cards (music videos and other
    /// uploads), for Home's "Live shows" and "Music videos for you" rows.
    pub videos: Vec<Card>,
    /// Where a shelf's Show all goes: YouTube's own discography browse for that
    /// shelf (browseId with its params), which lists everything the artist has.
    pub albums_more: Option<ShelfLink>,
    pub singles_more: Option<ShelfLink>,
}

/// A shelf's More button, as its navigation endpoint carries it.
#[derive(Clone, Debug, PartialEq)]
pub struct ShelfLink {
    pub browse_id: String,
    pub params: String,
}

pub struct Library {
    pub playlists: Vec<Card>,
    pub albums: Vec<Card>,
    /// Saved podcast shows, when podcasts are shown.
    pub podcasts: Vec<Card>,
    /// Subscribed artists.
    pub artists: Vec<Card>,
    /// That part didn't load (YouTube Music refused it, say): the one shown stays.
    pub playlists_failed: bool,
    pub albums_failed: bool,
    pub artists_failed: bool,
}

/// The signed-in account's channels.
pub struct Channels {
    pub list: Vec<ytm::Channel>,
    /// The page id of the one requests go as (`None`: the account's own).
    pub current: Option<String>,
    /// [`ytmapi::Api::epoch`] when listed: a switch picked from here applies only then.
    pub epoch: u64,
}

pub enum Resp {
    ForListening(Option<String>, Box<Resp>),
    Home(ytm::HomePage),
    HomeMore(ytm::HomePage),
    QuickPicksMore { seed: String, tracks: Vec<Track> },
    Explore(Explore),
    Mood(ytm::MoodPage),
    Search(SearchResults),
    /// A search of your library or your uploads (`SearchResults::scope`).
    SearchIn(SearchResults),
    /// `None`: this video can't be shown; the picture stays.
    VideoStream { video_id: String, max_height: u32, stream: Option<innertube::VideoStream> },
    Suggest { query: String, round: u64, terms: Vec<ytm::Suggestion> },
    /// An album or playlist; `more`: the rest of its songs follow in `CollectionMore`.
    Collection { load: u64, c: Collection, more: bool },
    /// The next page of a playlist sent as `Collection` with the same `load`.
    CollectionMore { id: String, load: u64, tracks: Vec<Track>, set_ids: Vec<Option<String>>, more: bool },
    Artist(ArtistPage),
    /// What a `Req::ArtistShelf` brought back: the shelf's cards, all of them.
    ArtistShelf { artist: String, title: String, cards: Vec<Card> },
    Library(Library),
    History(Vec<Track>),
    /// The account's history and newest likes, to seed the local taste from.
    SeedTaste { history: Vec<Track>, liked: Vec<Track> },
    /// Rows for Home, grown from the listening (see `crate::recommend`); a failed
    /// fetch answers with nothing, and the rows already on screen stay.
    RecoRows(Vec<crate::recommend::Row>),
    /// "New from your artists": this year's and last's releases, at least three
    /// of them (fewer answered nothing, and the row stays off).
    NewFromArtists(Vec<Card>),
    /// The hero's discoveries, and what each grew from; a failure answers with
    /// nothing, keeping what's already showing.
    DailyDiscover(Vec<(Track, Track)>),
    /// "All time hits" as song cards, and the community playlists and the live/
    /// music-video rows; failures keep what's already on screen.
    AllTimeHits(Vec<Card>),
    CommunityPlaylists(Vec<Card>),
    ArtistVideos { live: Vec<Card>, videos: Vec<Card> },
    Uploads(Vec<Track>),
    Account { name: Option<String>, photo: Option<String> },
    Channels(Channels),
    /// A `Req::SwitchChannel` is done (requests go as `epoch` now), or `error` says why it isn't.
    SwitchedChannel { page_id: Option<String>, epoch: u64, error: Option<String> },
    ServerQueue(ytm::ServerQueue),
    /// Upload finished: how many files went up, and which failed.
    Uploaded { done: usize, failed: Vec<String> },
    Downloaded { title: String, path: PathBuf },
    /// Every album, playlist or selection being downloaded, after one of them moved on.
    /// A finished one (`DownloadRun::finished`) is listed this once.
    Downloads(Vec<DownloadRun>),
    /// More than `LARGE_DOWNLOAD` songs (`count`) aren't downloaded yet: asked before
    /// sending the same `Req::DownloadMany` again, `confirmed`.
    ConfirmDownload { target: Option<Target>, title: String, tracks: Vec<Track>, count: usize },
    /// Every source's lyrics for a song (empty when nobody has them).
    Lyrics { video_id: String, lyrics: std::sync::Arc<LyricsSet> },
    Segments { video_id: String, segments: Vec<crate::sponsorblock::Segment> },
    /// Romanization/translation for one source's lines, arriving after them.
    LyricsExtras { video_id: String, source: String, target: String, extras: crate::translate::Extras },
    Art { key: String, art: Art },
    /// The animated cover found for a song (`None`: Apple has none); the UI hands the
    /// song to Spotify's canvas then.
    CanvasUrl { id: String, url: Option<String> },
    /// Spotify's canvas for the playing song; `key` is what the UI asked it for.
    SpotifyCanvas { key: String, url: Option<String> },
    /// A signature for an artist, already whitened for the header.
    ArtistSignature { key: String, signature: Option<crate::artist_signature::Signature> },
    Radio { seed: String, tracks: Vec<Track>, autoplay: bool },
    /// A comments response (`Req::Comments`, `CommentsPage` or `CommentAction`) by its tag;
    /// `None`: it failed.
    CommentsPage { token: String, page: Option<serde_json::Value> },
    /// What was asked to play (a `Req::PlayCollection`), its name, and its songs.
    PlayQueue { target: Target, title: String, tracks: Vec<Track> },
    Liked { video_id: String, like: bool },
    /// A run of likes or unlikes: `done` of `total` tried.
    LikeProgress { done: usize, total: usize, like: bool },
    /// The songs YouTube Music took, and the ones it refused.
    LikedMany { video_ids: Vec<String>, failed: Vec<String>, like: bool },
    /// The song `song`'s music video (`None`: it has none, or none was sure).
    MusicVideo { song: String, video: Option<ytm::MusicVideo> },
    /// A subscribe or unsubscribe went through (`ok`), or didn't.
    Subscribed { channel_id: String, on: bool, ok: bool },
    PlaylistCreated { id: String, songs: usize },
    PlaylistChanged { id: String, message: &'static str },
    /// A playlist wasn't deleted, though it's left the library here already.
    DeleteFailed { error: String },
    Collaboration(ytm::Collaboration),
    /// Playback for `generation` failed before reaching the audio engine.
    PlayError { generation: u64, msg: String, offer_backup_at: Option<std::time::Duration> },
    /// What `generation` is playing from a hi-res source (codec, rate, depth, size).
    AudioInfo { generation: u64, info: crate::hires::Info },
    /// Whether `generation` is actively attempting to upgrade quality from hi-res addons.
    UpgradingQuality { generation: u64, active: bool },
    /// What `generation`'s decoder is being fed, whatever the source (stats for nerds).
    StreamStats { generation: u64, stats: stream::Stats },
    /// Play `generation` is the song version of the music video that was asked for
    /// (`None`: that song wouldn't start, and the music video plays after all).
    PlayingAs { generation: u64, listed: String, song: Option<Track> },
    LoggedIn(bool),
    /// A past search left the account's history; the list already shows it gone.
    SearchForgotten,
    Error(String),
}

impl Resp {
    /// An answer that belongs to the signed-in account and channel it was asked as.
    fn is_account_scoped(&self) -> bool {
        matches!(
            self,
            Resp::Home(_)
                | Resp::HomeMore(_)
                | Resp::QuickPicksMore { .. }
                | Resp::Suggest { .. }
                | Resp::Library(_)
                | Resp::History(_)
                | Resp::SeedTaste { .. }
                | Resp::RecoRows(_)
                | Resp::NewFromArtists(_)
                | Resp::DailyDiscover(_)
                | Resp::AllTimeHits(_)
                | Resp::CommunityPlaylists(_)
                | Resp::ArtistVideos { .. }
                | Resp::Uploads(_)
                | Resp::Account { .. }
                | Resp::Channels(_)
                | Resp::ServerQueue(_)
                | Resp::SearchIn(_)
                | Resp::Liked { .. }
                | Resp::LikeProgress { .. }
                | Resp::LikedMany { .. }
                | Resp::Artist(_)
                | Resp::Subscribed { .. }
        )
    }

    /// An album or playlist load that ends without more songs: also sent when it fails,
    /// so the page stops waiting for it.
    fn ended(id: String, load: u64) -> Self {
        Resp::CollectionMore { id, load, tracks: Vec::new(), set_ids: Vec::new(), more: false }
    }
}

/// Likes and unlikes go one after another, this far apart. YouTube Music answers faster
/// ones as taken and then drops many: on a test account 9 of 20 unlikes sent back to back
/// stayed liked, against 1 of 20 sent 400 ms apart; on a limited account 576 of 1,296
/// likes sent back to back were dropped, and liking those again took as long as pacing.
pub const LIKE_GAP: std::time::Duration = std::time::Duration::from_millis(400);

/// One run of many likes at a time (a selection here, or a Spotify transfer): run side by
/// side, one undoes the other (a transfer skips songs as liked that an unlike run then
/// takes out).
pub static LIKES: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// How long one like or unlike may take. Taken ones answer in about a quarter second;
/// ones YouTube Music refuses while it limits the account are held 8–9 s before the 403
/// (two of ten unlikes cost 17 of 27 s; in a transfer one like in six or so hangs).
/// Sending it again is harmless (liking a liked song changes nothing) and usually goes
/// straight through.
pub const LIKE_WAIT: std::time::Duration = std::time::Duration::from_millis(1500);

/// Like or unlike one song; whether YouTube Music took it. Refusals are retried by the
/// API (after its shared pause while the account is limited); one that hangs or fails is
/// sent again, up to three times in all.
async fn like_one(api: &ytmapi::Api, id: &str, like: bool) -> bool {
    for tries in 0..3 {
        match tokio::time::timeout(LIKE_WAIT, ytm::set_like(api, id, like)).await {
            Ok(Ok(())) => return true,
            Ok(Err(e)) => log::debug!("like={like} {id}: {e:#}"),
            // Not refused yet: again right away.
            Err(_) => {
                log::debug!("like={like} {id}: no answer in {LIKE_WAIT:?}; sending it again");
                continue;
            }
        }
        if tries < 2 {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    }
    false
}

/// Like or unlike a selection, one at a time (paced, see `LIKE_GAP`), then check
/// Liked Music and try again what didn't take. `progress(taken, tried)` after each song.
/// Returns the ones taken and the ones refused; unlikes still listed once checking stops
/// getting anywhere count as refused, likes don't (songs that can't be played are liked but not listed).
/// One run at a time, transfers included (`LIKES`); `progress(None, 0)` while waiting its turn.
pub async fn like_many(api: &ytmapi::Api, ids: Vec<String>, like: bool, mut progress: impl FnMut(Option<&str>, usize)) -> (Vec<String>, Vec<String>) {
    let epoch = api.epoch();
    let _turn = match LIKES.try_lock() {
        Ok(turn) => turn,
        Err(_) => {
            progress(None, 0);
            LIKES.lock().await
        }
    };
    // A channel switch went first: these were picked in the other channel's lists.
    if api.epoch() != epoch {
        log::info!("likes: {} not sent, the account or channel changed while they waited", ids.len());
        return (Vec::new(), ids);
    }
    let total = ids.len();
    let mut failed = Vec::new();
    for (i, id) in ids.iter().enumerate() {
        let taken = like_one(api, id, like).await;
        if !taken {
            failed.push(id.clone());
        }
        progress(taken.then_some(id.as_str()), i + 1);
        tokio::time::sleep(LIKE_GAP).await;
    }
    // Check and try again while it gets anywhere: a throttled account takes a few rounds.
    let (mut before, mut stuck) = (usize::MAX, 0);
    for round in 0..10 {
        if total < 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        let listed: std::collections::HashSet<String> = match ytm::playlist(api, "LM", true, None).await {
            Ok(lm) => lm.tracks.into_iter().map(|t| t.id).collect(),
            Err(e) => {
                log::warn!("checking likes: {e:#}");
                break;
            }
        };
        let left: Vec<String> = ids.iter().filter(|id| listed.contains(*id) != like && !failed.contains(*id)).cloned().collect();
        log::debug!("like={like} check {round}: Liked Music lists {}, {} of {total} not taken yet", listed.len(), left.len());
        if left.is_empty() {
            break;
        }
        stuck = if left.len() < before { 0 } else { stuck + 1 };
        before = left.len();
        if stuck == 2 || round == 9 {
            if !like {
                failed.extend(left);
            }
            break;
        }
        // The second go is paced either way.
        for id in &left {
            like_one(api, id, like).await;
            tokio::time::sleep(LIKE_GAP).await;
        }
    }
    let done: Vec<String> = ids.into_iter().filter(|id| !failed.contains(id)).collect();
    (done, failed)
}

/// Songs of an album or playlist downloaded at once (each over `stream::WORKERS`
/// connections, behind the playing song).
const DOWNLOADS_AT_ONCE: usize = 2;
/// More songs than this to download in one go are asked about first.
pub const LARGE_DOWNLOAD: usize = 100;

/// An album, playlist or selection being downloaded.
#[derive(Clone, Debug, Default)]
pub struct DownloadRun {
    /// The album's or playlist's id, or `selection-N` for a selection.
    pub key: String,
    /// Empty for a selection.
    pub title: String,
    /// All its songs (those downloaded already count as done from the start), and how many are through.
    pub total: usize,
    pub done: usize,
    pub saved: usize,
    /// Already downloaded (or already on the way for another run).
    pub skipped: usize,
    /// Titles of the songs that couldn't be downloaded.
    pub failed: Vec<String>,
    /// Their ids, in the same order: queued again, the run tries them again.
    pub failed_ids: Vec<String>,
    pub cancelled: bool,
}

impl DownloadRun {
    pub fn finished(&self) -> bool {
        self.done >= self.total
    }
}

/// Songs waiting to download, one queue for every run, worked through by up to
/// `DOWNLOADS_AT_ONCE` tasks.
#[derive(Default)]
struct DownloadQueue {
    waiting: std::collections::VecDeque<Track>,
    /// Each song waiting or downloading, with the keys of the runs that want it: it
    /// downloads once, for all of them.
    wanted: std::collections::HashMap<String, Vec<String>>,
    /// Unfinished runs.
    runs: Vec<DownloadRun>,
    workers: usize,
    /// Songs being saved now, by any download.
    saving: std::collections::HashSet<String>,
    /// Selections queued this session, for their keys.
    selections: usize,
}

impl DownloadQueue {
    /// Every run as it stands; a finished one is listed this once.
    fn snapshot(&mut self) -> Resp {
        let runs = self.runs.clone();
        self.runs.retain(|r| !r.finished());
        Resp::Downloads(runs)
    }

    /// Queue the songs `todo` of a list of `listed` songs (the others are downloaded
    /// already) for the run `key`, or a new selection's. One on its way for another run
    /// already downloads for this one too; one on its way for this run isn't queued
    /// twice. Queued again, a run takes on just the songs it didn't have, and tries again
    /// the ones that failed (they're its songs already: not counted twice). Returns the
    /// run's key and how many songs were queued for it.
    fn add(&mut self, key: Option<String>, title: String, listed: usize, todo: Vec<Track>) -> (String, usize) {
        let skipped = listed.saturating_sub(todo.len());
        let key = key.unwrap_or_else(|| {
            self.selections += 1;
            format!("selection-{}", self.selections)
        });
        let at = self.runs.iter().position(|r| r.key == key);
        let (mut queued, mut added) = (0, 0);
        for t in todo {
            match self.wanted.get_mut(&t.id) {
                Some(keys) if keys.contains(&key) => continue,
                Some(keys) => keys.push(key.clone()),
                None => {
                    self.wanted.insert(t.id.clone(), vec![key.clone()]);
                    self.waiting.push_back(t.clone());
                }
            }
            queued += 1;
            let run = at.map(|i| &mut self.runs[i]);
            match run.and_then(|r| Some((r.failed_ids.iter().position(|id| *id == t.id)?, r))) {
                Some((n, run)) => {
                    run.failed_ids.remove(n);
                    run.failed.remove(n);
                    run.done -= 1;
                }
                None => added += 1,
            }
        }
        // The ones on disk already count as done, so the progress (and the cover's veil)
        // starts from how much of the list is there.
        match at.map(|i| &mut self.runs[i]) {
            Some(run) => {
                run.total += added;
                run.cancelled = false;
            }
            None => self.runs.push(DownloadRun { key: key.clone(), title, total: added + skipped, done: skipped, skipped, ..Default::default() }),
        }
        (key, queued)
    }

    /// The next song to download, or none: then the worker asking stops.
    fn next(&mut self) -> Option<Track> {
        let next = self.waiting.pop_front();
        if next.is_none() {
            self.workers -= 1;
        }
        next
    }

    /// How many more workers to start for the songs waiting.
    fn more_workers(&mut self) -> usize {
        let start = DOWNLOADS_AT_ONCE.min(self.workers + self.waiting.len()).saturating_sub(self.workers);
        self.workers += start;
        start
    }

    /// A song is through, for every run that wanted it: `saved`, there already
    /// (`Ok(false)`), or failed.
    fn finish<E>(&mut self, t: &Track, result: &Result<bool, E>) {
        for key in self.wanted.remove(&t.id).unwrap_or_default() {
            let Some(run) = self.runs.iter_mut().find(|r| r.key == key) else { continue };
            run.done += 1;
            match result {
                Ok(true) => run.saved += 1,
                Ok(false) => run.skipped += 1,
                Err(_) => {
                    run.failed.push(t.title.clone());
                    run.failed_ids.push(t.id.clone());
                }
            }
        }
    }

    /// Stop the run `key`: its waiting songs are dropped (unless another run wants them
    /// too); the ones downloading now finish. False when there's no such run.
    fn cancel(&mut self, key: &str) -> bool {
        let mut dropped = 0;
        let DownloadQueue { waiting, wanted, .. } = self;
        waiting.retain(|t| {
            let Some(keys) = wanted.get_mut(&t.id) else { return true };
            let before = keys.len();
            keys.retain(|k| k != key);
            dropped += before - keys.len();
            let keep = !keys.is_empty();
            if !keep {
                wanted.remove(&t.id);
            }
            keep
        });
        let Some(run) = self.runs.iter_mut().find(|r| r.key == key) else { return false };
        run.total -= dropped;
        run.cancelled = true;
        true
    }
}

/// A song claimed for saving (`DownloadQueue::saving`), let go when dropped.
struct Saving(Arc<std::sync::Mutex<DownloadQueue>>, String);

impl Drop for Saving {
    fn drop(&mut self) {
        self.0.lock().unwrap().saving.remove(&self.1);
    }
}

/// The song's file if it's downloaded already (and then, given `for_video`, that music
/// video plays it too): disk work, off the async workers.
async fn reuse_download(song_id: &str, for_video: Option<String>) -> Option<PathBuf> {
    let id = song_id.to_owned();
    tokio::task::spawn_blocking(move || crate::downloads::reuse(&id, for_video.as_deref())).await.ok().flatten()
}

/// Downloaded already: the song, or a music video's song version.
pub fn is_downloaded(t: &Track) -> bool {
    crate::downloads::path(&t.id).is_some() || (t.music_video && crate::downloads::song_for(&t.id).is_some())
}

/// A video id and its stream buffer.
type StreamSlot = Arc<std::sync::Mutex<Option<(String, Arc<Shared>)>>>;
type SongCell = Arc<tokio::sync::OnceCell<Option<Track>>>;

/// A music video's stream opened while its song was looked up. Left unused, it stops.
struct EarlyVideo(Option<tokio::task::JoinHandle<anyhow::Result<Opened>>>);

type DeferredAddons = Option<(Vec<Arc<hires::Client>>, Track)>;
type Opened = (Arc<Shared>, DeferredAddons);
type PrefetchSlot = Arc<std::sync::Mutex<Option<(String, Opened)>>>;

impl Drop for EarlyVideo {
    fn drop(&mut self) {
        if let Some(open) = self.0.take()
            && let Ok(rt) = tokio::runtime::Handle::try_current()
        {
            rt.spawn(async move {
                if let Ok(Ok((s, _))) = open.await {
                    s.cancel();
                }
            });
        }
    }
}

/// How long a play waits for a music video's song version before playing the video.
const PLAY_SONG_WAIT: std::time::Duration = std::time::Duration::from_millis(2500);
/// A lookup still going after this starts the music video's own stream alongside, so
/// it's ready to play when no song turns up.
const EARLY_VIDEO_AFTER: std::time::Duration = std::time::Duration::from_millis(200);
/// A prefetch has until the song before it ends.
const PREFETCH_SONG_WAIT: std::time::Duration = std::time::Duration::from_secs(20);
/// How deep one radio fetch goes; the queue's own pick keeps pace with it.
const RADIO_FETCH: usize = 50;
/// Fewer usable songs than this from a source, and the next one is asked (Mochy's
/// rows drop under four; a shorter queue than that barely tops anything up).
const RADIO_MIN: usize = 4;
/// How long playback waits for the hi-res sources before falling back to YouTube. Every
/// source is asked at once and a miss is quick (the addon answers its search fast); this
/// bounds the ones that are slow, not the ones that don't have the track. It matches the
/// Android app's five-second play budget: past that the listener is better on YouTube
/// with an upgrade in the background than waiting on a cold addon resolve.
const HIRES_WAIT: std::time::Duration = std::time::Duration::from_secs(8);
/// The background upgrade runs with sound already playing, so it can afford the slower
/// window a cold addon resolve needs; nothing waits on this but the swap itself.
const UPGRADE_WAIT: std::time::Duration = std::time::Duration::from_secs(20);
/// The addon upgrade's own window, past the provider one: a manifest rendition is
/// fetched whole on this pass, and a self-hosted addon paces that download for
/// its origin's sake (three lanes, not six — see `dash`), so the window has to
/// cover a ~35 MB rendition walked at that pace, not just a cold resolve.
const UPGRADE_ADDON_WAIT: std::time::Duration = std::time::Duration::from_secs(45);

#[derive(Clone)]
pub struct Backend {
    rt: Arc<Runtime>,
    api: Arc<ytmapi::Api>,
    tx: Sender<Resp>,
    audio: AudioHandle,
    wake: Arc<dyn Fn() + Send + Sync>,
    data_dir: PathBuf,
    lyrics: Arc<lyrics::Client>,
    translate: Arc<crate::translate::Client>,
    /// The upcoming track, resolved and downloading ahead of time.
    prefetched: PrefetchSlot,
    /// The stream most recently started, kept so a replay of a fully downloaded song
    /// (repeat one, restart) doesn't download it again.
    current: StreamSlot,
    jsc: Arc<jsc::Solver>,
    /// Whether the signed-in account has Premium; checked once per sign-in. A failed
    /// check is retried no sooner than `PREMIUM_RETRY` later.
    premium: Arc<tokio::sync::Mutex<Result<bool, Option<std::time::Instant>>>>,
    /// High quality: with Premium, every song goes through the signed-in web player
    /// for its 256 kbps stream instead of the anonymous 128 kbps one.
    hq: Arc<std::sync::atomic::AtomicBool>,
    /// The backup stream is on: a bot-check refusal offers Safari's HLS (Settings › Network).
    backup_stream: Arc<std::sync::atomic::AtomicBool>,
    backup_stream_revision: Arc<std::sync::atomic::AtomicU64>,
    /// Explicit songs are hidden: a music video's song version has to be clean.
    clean: Arc<std::sync::atomic::AtomicBool>,
    /// The visionOS player's file URLs were refused past 1 MiB this session (YouTube
    /// wants a PO token for them on this network): songs play from its HLS segments.
    visionos_hls: Arc<std::sync::atomic::AtomicBool>,
    /// The listener's hi-res addons, in the order their links were given.
    hires: Arc<std::sync::RwLock<Vec<Arc<hires::Client>>>>,
    /// Music providers, ported from Mochy-Music: Monochrome's FLAC and JioSaavn's AAC.
    monochrome: Arc<providers::Monochrome>,
    jiosaavn: Arc<providers::JioSaavn>,
    /// The sources before YouTube, in the order Settings › Music sources lists them,
    /// with each one's switch: the first one that has the song plays its copy.
    sources: Arc<std::sync::RwLock<Vec<(MusicSource, bool)>>>,
    /// Ask the addons for Dolby Atmos renditions when they offer one.
    atmos: Arc<std::sync::atomic::AtomicBool>,
    /// Play YouTube first and upgrade to hi-res in the background (see
    /// `set_disable_direct_stream`): the addons are still asked, but only once the
    /// song is already playing, and their copy takes over in place when one answers.
    disable_direct: Arc<std::sync::atomic::AtomicBool>,
    /// Songs the listener sent back to YouTube audio: no addon copy until they leave.
    reverted: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    /// Song versions of music videos looked up this session (`None`: there's none), by
    /// video and whether the account has Premium (which gets YouTube's own pairs). One
    /// cell each, so a prefetch and the play of the same video share a lookup.
    songs: Arc<std::sync::Mutex<std::collections::HashMap<String, SongCell>>>,
    /// Albums and playlists being downloaded.
    batch: Arc<std::sync::Mutex<DownloadQueue>>,
    /// Spotify's web-player client for canvas lookups, kept for the sign-in it was built
    /// with: its tokens and query hashes cost several round trips.
    canvas_spotify: Arc<tokio::sync::Mutex<Option<(String, Arc<crate::transfer::spotify::Client>)>>>,
    /// Canvas answers per song (`None`: Spotify has none, or nobody's signed in),
    /// shedding the oldest when full rather than everything at once.
    canvas_cache: Arc<tokio::sync::Mutex<CanvasCache>>,
}

/// Spotify canvas answers by song, remembering arrival order so a full cache sheds its
/// oldest entries (a clear-all refetched the whole recent room's worth at once).
#[derive(Default)]
struct CanvasCache {
    answers: std::collections::HashMap<String, (Option<String>, std::time::Instant)>,
    order: std::collections::VecDeque<String>,
}

const CANVAS_CACHE: usize = 128;

impl CanvasCache {
    fn get(&self, key: &str) -> Option<&Option<String>> {
        self.answers.get(key).filter(|(url, at)| url.is_some() || at.elapsed() < std::time::Duration::from_secs(120)).map(|(url, _)| url)
    }

    fn insert(&mut self, key: &str, answer: Option<String>) {
        if !self.answers.contains_key(key) {
            self.order.push_back(key.to_owned());
            while self.order.len() > CANVAS_CACHE {
                if let Some(oldest) = self.order.pop_front() {
                    self.answers.remove(&oldest);
                }
            }
        }
        self.answers.insert(key.to_owned(), (answer, std::time::Instant::now()));
    }
}

impl Backend {
    pub fn new(
        storage: PathBuf,
        tx: Sender<Resp>,
        audio: AudioHandle,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> anyhow::Result<Self> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("net")
            .enable_all()
            .build()?;
        std::fs::create_dir_all(&storage)?;
        let data_dir = storage;
        let http = crate::proxy::http();
        let lyrics = Arc::new(lyrics::Client::new(http.clone(), &data_dir));
        let translate = Arc::new(crate::translate::Client::new(http, &data_dir));
        let jsc = Arc::new(jsc::Solver::new(&data_dir));
        let api = Arc::new(ytmapi::Api::new(&data_dir));
        if crate::proxy::needs_relay()
            && let Err(e) = rt.block_on(crate::proxy::start_relay())
        {
            log::warn!("proxy relay: {e:#}");
        }
        let backend = Self { rt: Arc::new(rt), api, tx, audio, wake, prefetched: Default::default(), current: Default::default(), data_dir, lyrics, translate, jsc, premium: Arc::new(tokio::sync::Mutex::new(Err(None))), hq: Default::default(), backup_stream: Default::default(), backup_stream_revision: Default::default(), clean: Default::default(), visionos_hls: Default::default(), hires: Default::default(), monochrome: Arc::new(providers::Monochrome::new()), jiosaavn: Arc::new(providers::JioSaavn::new()), sources: Arc::new(std::sync::RwLock::new(DEFAULT_SOURCES.to_vec())), atmos: Default::default(), disable_direct: Default::default(), reverted: Default::default(), songs: Default::default(), batch: Default::default(), canvas_spotify: Default::default(), canvas_cache: Default::default() };
        backend.keep_signed_in();
        Ok(backend)
    }

    /// Refresh the session's cookies every few minutes, as an open YouTube tab does:
    /// Google signs out a copy that stops taking new ones.
    fn keep_signed_in(&self) {
        const EVERY: std::time::Duration = std::time::Duration::from_secs(10 * 60);
        let this = self.clone();
        self.rt.spawn(async move {
            loop {
                if this.api.account().is_some() {
                    this.api.ensure_visitor().await;
                    match this.api.rotate().await {
                        Ok(()) => log::debug!("session cookies refreshed"),
                        Err(e) if e.is::<webmusic::Revoked>() => this.check_session().await,
                        Err(e) => log::info!("refreshing session cookies failed: {e:#}"),
                    }
                }
                tokio::time::sleep(EVERY).await;
            }
        });
    }

    /// Something suggested Google signed the session out. When YouTube confirms it no
    /// longer knows the session, sign out here too and say so, rather than every
    /// signed-in page failing on its own.
    async fn check_session(&self) {
        // Several requests answered at once ask together; one check is enough.
        static CHECKING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
        let _one = CHECKING.lock().await;
        if self.api.account().is_none() {
            return;
        }
        // Signing out deletes the session, so only once youtube.com shows it signed out
        // twice, a minute apart (each after a rotation, which can bring it back). One
        // signed-out answer can be a hiccup (throttling, a consent or "unusual traffic"
        // page), and a refused rotation alone isn't proof either: the clients whose
        // sessions last never sign out on their own.
        for attempt in 0..2 {
            if let Err(e) = self.api.rotate().await {
                log::info!("rotating cookies while checking the session: {e:#}");
            }
            let Some(account) = self.api.account() else { return };
            match self.api.session_from_cookie(&account.cookie).await {
                Ok(_) => return,
                Err(e) if e.is::<ytmapi::NotSignedIn>() => {
                    log::warn!("signed-in session looks signed out ({}): {e:#}", if attempt == 0 { "checking again in a minute" } else { "twice" });
                    if attempt == 0 {
                        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                    }
                }
                // Offline, throttled or similar: nothing learned.
                Err(e) => {
                    log::info!("couldn't check the session: {e:#}");
                    return;
                }
            }
        }
        log::warn!("signed-in session no longer valid");
        self.forget_session().await;
        let _ = self.tx.send(Resp::LoggedIn(false));
        let _ = self.tx.send(Resp::Error("Your YouTube sign-in expired. Sign in again to get your library back".into()));
        self.wake();
    }

    /// Act as another of the account's channels. Not while likes of a selection or a
    /// transfer are running: they'd go on as the new channel.
    async fn switch_channel(&self, page_id: Option<String>, epoch: u64) -> anyhow::Result<u64> {
        let _likes = LIKES.try_lock().map_err(|_| anyhow::anyhow!("Wait until the likes going through are done"))?;
        // A Premium check running now is for the old channel: let it finish, then forget it.
        let mut premium = self.premium.lock().await;
        let epoch = self.api.switch_channel(page_id, epoch)?;
        *premium = Err(None);
        drop(premium);
        self.warm_signed_in();
        log::info!("switched channel");
        Ok(epoch)
    }

    async fn forget_session(&self) {
        self.api.sign_out();
        *self.premium.lock().await = Err(None);
        let profile = self.login_profile();
        let _ = tokio::task::spawn_blocking(move || crate::login::forget(&profile)).await;
    }

    /// Spotify's web-player client for canvas lookups, rebuilt when the sign-in changed.
    async fn canvas_client(&self) -> anyhow::Result<Arc<crate::transfer::spotify::Client>> {
        let sp_dc = crate::transfer::vault_cookie(&self.data_dir).context("not signed in to Spotify")?;
        let mut held = self.canvas_spotify.lock().await;
        if let Some((cookie, client)) = held.as_ref()
            && *cookie == sp_dc
        {
            return Ok(client.clone());
        }
        let client = Arc::new(crate::transfer::spotify::Client::connect(Some(&sp_dc), &self.data_dir).await?);
        *held = Some((sp_dc, client.clone()));
        Ok(client)
    }

    /// The playing song's canvas (Spotify's own looping cover video), the way the web
    /// player looks it up: search the song, take the best match, ask for its clip.
    /// Answered once per song; asked again only after the cache is dropped.
    pub(crate) async fn spotify_canvas(&self, key: &str, title: &str, artist: &str, duration_ms: Option<u64>) -> Option<String> {
        // Player and Discord may ask about the same song on the same frame.
        let mut cache = self.canvas_cache.lock().await;
        if let Some(hit) = cache.get(key) {
            return hit.clone();
        }
        let hit = self.spotify_canvas_inner(title, artist, duration_ms).await;
        cache.insert(key, hit.clone());
        hit
    }

    async fn spotify_canvas_inner(&self, title: &str, artist: &str, duration_ms: Option<u64>) -> Option<String> {
        let client = self.canvas_client().await.inspect_err(|e| log::info!("canvas: {e:#}")).ok()?;
        let hits = client.search_tracks(&format!("{title} {artist}")).await.inspect_err(|e| log::info!("canvas: {e:#}")).ok()?;
        let query = crate::providers::Query {
            title,
            artists: artist.split(',').map(str::trim).filter(|a| !a.is_empty()).map(str::to_owned).collect(),
            duration_ms,
        };
        let candidates = hits
            .into_iter()
            .map(|h| crate::providers::Catalog {
                id: h.id,
                artist: h.track.artist_line(),
                title: h.track.name,
                album: None,
                duration_ms: h.track.duration.map(|s| s as u64 * 1000),
            })
            .collect();
        let best = crate::providers::ranked(&query, candidates).into_iter().next()?;
        client.canvas(&best.id).await.inspect_err(|e| log::info!("canvas: {e:#}")).ok()?
    }

    pub fn api(&self) -> Arc<ytmapi::Api> {
        self.api.clone()
    }

    /// The shared client (see `proxy::http`).
    pub fn http(&self) -> reqwest::Client {
        crate::proxy::http()
    }

    pub fn spawn<F: Future<Output = ()> + Send + 'static>(&self, f: F) {
        self.rt.spawn(f);
    }

    /// Like `spawn`, but hands the task's handle back, so it can be aborted later
    /// (Discord's idle close, a party leaving).
    pub fn spawn_handle<F: Future<Output = ()> + Send + 'static>(&self, f: F) -> tokio::task::JoinHandle<()> {
        self.rt.spawn(f)
    }

    pub fn wake(&self) {
        (self.wake)()
    }

    pub fn request(&self, req: Req) {
        self.request_scoped(req, None);
    }

    pub fn request_scoped(&self, req: Req, scope: Option<Option<String>>) {
        let (what, quiet) = (req.name(), req.quiet());
        let play_gen = match &req {
            Req::Play { generation, .. } => Some(*generation),
            _ => None,
        };
        let collection = match &req {
            Req::Album { id, load } | Req::Playlist { id, load, .. } => Some((id.clone(), *load)),
            _ => None,
        };
        // A subscribe always answers as `Resp::Subscribed`, which lets the next one go.
        let subscribe = match &req {
            Req::Subscribe { channel_id, on } => Some((channel_id.clone(), *on)),
            _ => None,
        };
        let this = self.clone();
        let t0 = std::time::Instant::now();
        let epoch = self.api.epoch();
        if scope.as_ref().is_some_and(|s| *s != self.listening_scope()) {
            return;
        }
        let task = self.rt.spawn(async move {
            let r = this.handle(req).await;
            log::debug!("{what}: done in {:?}", t0.elapsed());
            r
        });
        let this = self.clone();
        self.rt.spawn(async move {
            // A panic in a handler (or a library it calls) fails just this request,
            // reported like any other error.
            let resp = match task.await {
                Ok(resp) => resp,
                Err(e) if e.is_panic() => {
                    log::error!("{what}: request panicked");
                    match (play_gen, subscribe) {
                        (Some(generation), _) => Some(Resp::PlayError { generation, msg: "YouTube didn't respond properly".into(), offer_backup_at: None }),
                        (_, Some((channel_id, on))) => Some(Resp::Subscribed { channel_id, on, ok: false }),
                        _ => (!quiet).then(|| Resp::Error(format!("{what} failed: YouTube didn't respond properly"))),
                    }
                }
                Err(_) => None,
            };
            // An album or playlist load that failed ends too, so its page stops waiting.
            if let Some((id, load)) = collection
                && !matches!(resp, Some(Resp::Collection { .. } | Resp::CollectionMore { .. }))
            {
                let _ = this.tx.send(Resp::ended(id, load));
            }
            // Asked as another sign-in or channel than the one now used: it isn't this one's.
            let resp = resp.filter(|r| {
                let stale = this.api.epoch() != epoch && r.is_account_scoped();
                if stale {
                    log::debug!("{what}: answer for the previous account or channel dropped");
                }
                !stale
            });
            if let Some(resp) = resp {
                let resp = match scope {
                    Some(scope) => Resp::ForListening(scope, Box::new(resp)),
                    None => resp,
                };
                let _ = this.tx.send(resp);
                this.wake();
            }
            if this.api.take_signed_out() {
                this.check_session().await;
            }
        });
    }

    /// A request answered before this returns (or given up on after a few seconds), its
    /// answer dropped: for the app closing, which can't wait for one.
    pub fn request_now(&self, req: Req) {
        let what = req.name();
        // In the runtime: a timeout made outside it panics.
        if self.rt.block_on(async { tokio::time::timeout(std::time::Duration::from_secs(3), self.handle(req)).await }).is_err() {
            log::warn!("{what}: no answer before closing");
        }
    }

    async fn handle(&self, req: Req) -> Option<Resp> {
        let api = &*self.api;
        // Personalised pages ask as the account when signed in, like music.youtube.com.
        let me = self.is_logged_in();
        let (what, quiet) = (req.name(), req.quiet());
        let res: anyhow::Result<Option<Resp>> = async {
            Ok(Some(match req {
                Req::Home(params) => Resp::Home(ytm::home(api, params.as_deref(), me).await?),
                Req::HomeMore(token) => Resp::HomeMore(ytm::home_more(api, &token, me).await?),
                Req::MoreQuickPicks(seed) => {
                    let tracks = ytm::radio(api, &seed, me, 0).await?;
                    Resp::QuickPicksMore { seed, tracks }
                }
                Req::Explore(preferred_artists) => self.explore(&preferred_artists, me).await?,
                Req::Mood(params) => Resp::Mood(ytm::mood(api, &params, me).await?),
                Req::Search(query) => self.search(query, me).await?,
                Req::SearchIn { query, scope } => Resp::SearchIn(Self::search_in(ytm::search_in(api, &query, scope).await?, query, scope)),
                Req::Suggest { query, round } => {
                    let terms = ytm::suggestions(api, &query, me).await?.into_iter().take(8).collect();
                    Resp::Suggest { query, round, terms }
                }
                Req::ForgetSearch(token) => {
                    ytm::forget_search(api, &token).await?;
                    Resp::SearchForgotten
                }
                Req::Album { id, load } => Resp::Collection { load, c: ytm::album(api, &id).await?, more: false },
                Req::Playlist { id, load, stream: false } => Resp::Collection { load, c: ytm::playlist(api, &id, me, None).await?, more: false },
                Req::Playlist { id, load, stream: true } => self.stream_playlist(id, load, me).await?,
                Req::Artist(id) => Resp::Artist(ytm::artist(api, &id, me).await?),
                Req::ArtistShelf { browse_id, params, artist, title } => Resp::ArtistShelf { artist, title, cards: ytm::discography(api, &browse_id, &params).await? },
                Req::Library => self.library().await?,
                Req::History => Resp::History(ytm::history(api).await?),
                Req::SeedTaste => {
                    // A miss on the likes seeds the history alone; a miss on the history
                    // fails the request, so the next launch tries again.
                    let (history, liked) = tokio::join!(ytm::history(api), ytm::playlist(api, "LM", true, Some(50)));
                    Resp::SeedTaste { history: history?, liked: liked.map(|c| c.tracks).unwrap_or_default() }
                }
                Req::BecauseYouLike { seeds, taste } => {
                    let hide = taste.hide_explicit;
                    let taste = crate::recommend::Taste::from_parts(&taste);
                    // one row per seed; a seed that fails leaves its row out, not the lot
                    let rows = futures_util::future::join_all(seeds.iter().map(|seed| {
                        let taste = &taste;
                        async move {
                            let queue = ytm::radio(api, &seed.id, me, 25).await.ok()?;
                            let tracks = crate::recommend::pick(queue, taste, hide, 10, 2, &mut crate::recommend::Rng::new());
                            (!tracks.is_empty()).then(|| crate::recommend::Row {
                                kind: crate::recommend::Kind::BecauseYouLike,
                                title: seed.title.clone(),
                                thumb: model::pick(&seed.thumbs, 544).map(str::to_owned),
                                tracks,
                                endpoint: None,
                            })
                        }
                    }))
                    .await
                    .into_iter()
                    .flatten()
                    .collect();
                    Resp::RecoRows(rows)
                }
                Req::MoreLike { artists, taste } => {
                    let hide = taste.hide_explicit;
                    let taste = crate::recommend::Taste::from_parts(&taste);
                    let rows = futures_util::future::join_all(artists.iter().map(|(id, name)| {
                        let taste = &taste;
                        let (id, name) = (id.as_str(), name.as_str());
                        async move {
                            let page = ytm::artist(api, id, me).await.ok()?;
                            // the artist's radio, minus the artist themselves
                            let queue = ytm::radio_playlist(api, &format!("RDATAR{id}"), me, 40).await.ok()?;
                            let others: Vec<Track> = queue.into_iter().filter(|song| song.artists.iter().all(|a| a.id.as_deref() != Some(id) && !a.name.eq_ignore_ascii_case(name))).collect();
                            let tracks = crate::recommend::pick(others, taste, hide, 12, 1, &mut crate::recommend::Rng::new());
                            (!tracks.is_empty()).then(|| crate::recommend::Row {
                                kind: crate::recommend::Kind::MoreLike,
                                title: name.to_owned(),
                                thumb: model::pick(&page.banner, 120).map(str::to_owned),
                                tracks,
                                endpoint: Some(crate::recommend::Endpoint::Artist(id.to_owned())),
                            })
                        }
                    }))
                    .await
                    .into_iter()
                    .flatten()
                    .collect();
                    Resp::RecoRows(rows)
                }
                Req::GenreRows { artists, taste } => self.genre_rows(&artists, &taste, me).await?,
                Req::NewFromArtists(ids) => match self.new_from_artists(&ids, me).await? {
                    Some(cards) => Resp::NewFromArtists(cards),
                    None => return Ok(None),
                },
                Req::DailyDiscover { seeds, taste } => {
                    // anything already heard isn't much of a discovery: the first unheard
                    // shuffled song off each seed's radio, else any of them
                    let heard: std::collections::HashSet<&str> = taste.heard.iter().map(String::as_str).collect();
                    let hidden = &taste.avoid_songs;
                    let hide = taste.hide_explicit;
                    let found = futures_util::future::join_all(seeds.iter().map(|seed| {
                        let heard = &heard;
                        let hidden = hidden;
                        async move {
                            let queue = ytm::radio(api, &seed.id, me, 25).await.ok()?;
                            let candidates: Vec<&Track> = queue
                                .iter()
                                .filter(|t| t.id != seed.id && !hidden.contains(&t.id) && !(hide && t.explicit) && !t.has_video())
                                .collect();
                            let unseen = candidates.iter().filter(|t| !heard.contains(t.id.as_str())).cloned().collect::<Vec<_>>();
                            let pool = if unseen.is_empty() { candidates } else { unseen };
                            let found = crate::recommend::Rng::new().unit();
                            pool.get((found * pool.len() as f64) as usize).map(|t| (seed.clone(), (*t).clone()))
                        }
                    }))
                    .await
                    .into_iter()
                    .flatten()
                    .collect();
                    Resp::DailyDiscover(found)
                }
                Req::AllTimeHits => {
                    let charts = ytm::charts(api).await?;
                    let hits: Vec<Card> = charts
                        .top
                        .iter()
                        .take(20)
                        .map(|t| Card {
                            title: t.title.clone(),
                            subtitle: t.artist_line(),
                            thumbs: t.thumbs.clone(),
                            target: Target::Song(t.clone()),
                            round: false,
                            links: t.artists.clone(),
                        })
                        .collect();
                    Resp::AllTimeHits(hits)
                }
                Req::CommunityPlaylists(artists) => {
                    // playlists off their pages that aren't YouTube's own mixes and
                    // albums, or the seed artist's
                    let pages = futures_util::future::join_all(artists.iter().map(|(id, _)| async move { ytm::artist(api, id, me).await.ok() })).await;
                    let mut candidates: Vec<Card> = pages
                        .iter()
                        .flatten()
                        .flat_map(|p| p.playlists.clone())
                        .filter(|c| {
                            let author = c.subtitle.split('•').last().map(str::trim).unwrap_or_default();
                            let a = author.to_lowercase();
                            !a.is_empty()
                                && a != "youtube music"
                                && a != "youtube"
                                && a != "playlist"
                                && !authors_matches(&artists, author)
                        })
                        .filter(|c| {
                            !matches!(&c.target, Target::Playlist(id) if id.starts_with("RD") || id.starts_with("OLAK"))
                        })
                        .collect();
                    let mut seen = std::collections::HashSet::new();
                    candidates.retain(|c| seen.insert(c.title.clone()));
                    let mut rng = crate::recommend::Rng::new();
                    rng.shuffle(&mut candidates);
                    candidates.truncate(5);
                    Resp::CommunityPlaylists(candidates)
                }
                Req::ArtistVideos(artists) => {
                    // a few of each artist's videos and live performances, dealt out
                    // one artist at a time
                    let pages = futures_util::future::join_all(artists.iter().map(|(id, _)| async move { ytm::artist(api, id, me).await.ok() })).await;
                    let mut per_artist: Vec<(Vec<Card>, Vec<Card>)> = Vec::new();
                    for page in pages.iter().flatten() {
                        let mut live: Vec<Card> = Vec::new();
                        let mut videos: Vec<Card> = Vec::new();
                        let mut pool = page.videos.clone();
                        crate::recommend::Rng::new().shuffle(&mut pool);
                        for c in pool.into_iter().take(6) {
                            if c.title.to_lowercase().contains("live") {
                                live.push(c);
                            } else {
                                videos.push(c);
                            }
                        }
                        per_artist.push((live.into_iter().take(3).collect(), videos.into_iter().take(3).collect()));
                    }
                    let dealt = interleave(
                        per_artist.iter().map(|(l, _)| l.iter().cloned()).collect(),
                        12,
                    );
                    let live_ids: std::collections::HashSet<&str> = dealt.iter().filter_map(|c| match &c.target { Target::Song(t) => Some(t.id.as_str()), _ => None }).collect();
                    let videos = interleave(
                        per_artist.iter().map(|(l, v)| v.iter().filter(|c| !live_ids.contains(match &c.target { Target::Song(t) => t.id.as_str(), _ => "" })).cloned()).collect(),
                        12,
                    );
                    Resp::ArtistVideos { live: dealt, videos }
                }
                Req::Uploads => Resp::Uploads(ytm::uploads(api).await?),
                Req::Account => {
                    let (name, photo) = ytm::account(api).await?;
                    Resp::Account { name, photo }
                }
                Req::Channels => {
                    let epoch = api.epoch();
                    let list = ytm::channels(api).await?;
                    Resp::Channels(Channels { list, current: api.account().and_then(|a| a.channel().map(str::to_owned)), epoch })
                }
                Req::SwitchChannel { page_id, epoch } => match self.switch_channel(page_id.clone(), epoch).await {
                    Ok(epoch) => Resp::SwitchedChannel { page_id, epoch, error: None },
                    Err(e) => {
                        log::warn!("switching channel failed: {e:#}");
                        Resp::SwitchedChannel { page_id, epoch, error: Some(e.to_string()) }
                    }
                },
                Req::ServerQueue => match ytm::server_queue(api).await? {
                    Some(q) => Resp::ServerQueue(q),
                    None => return Ok(None),
                },
                Req::Upload(files) => self.upload(&files).await?,
                Req::Lyrics(track) => {
                    let video_id = track.id.clone();
                    Resp::Lyrics { lyrics: self.lyrics_for(&track).await, video_id }
                }
                Req::Comments { video_id } => {
                    let page = ytm::comments(api, &video_id, me).await.map(|p| p.unwrap_or_else(|| serde_json::json!({ "commentsOff": true })));
                    if let Err(e) = &page {
                        log::warn!("comments for {video_id}: {e:#}");
                    }
                    Resp::CommentsPage { token: format!("video:{video_id}"), page: page.ok() }
                }
                Req::CommentAction { endpoint, body, tag } => {
                    // As the account: only ever from a click (Comment, Reply, the thumb).
                    let page = api.call_signed_in(&endpoint, body).await;
                    if let Err(e) = &page {
                        log::warn!("comments {endpoint}: {e:#}");
                    }
                    Resp::CommentsPage { token: tag, page: page.ok() }
                }
                Req::CommentsPage { token } => {
                    let page = api.call("next", serde_json::json!({ "continuation": token }), me).await;
                    if let Err(e) = &page {
                        log::warn!("comments page: {e:#}");
                    }
                    Resp::CommentsPage { token, page: page.ok() }
                }
                Req::Segments { video_id } => {
                    let segments = crate::sponsorblock::segments(&self.http(), &video_id).await?;
                    Resp::Segments { video_id, segments }
                }
                Req::LyricsExtras { video_id, source, lines, lang, target } => {
                    let key = format!("{video_id}_{source}");
                    let Some(extras) = self.translate.extras(&key, &lines, lang.as_deref(), &target).await else { return Ok(None) };
                    Resp::LyricsExtras { video_id, source, target, extras }
                }
                Req::Art { key, url } => {
                    // A local song's cover is a file, not a fetch (see `local.rs`).
                    let art = if let Some(path) = url.strip_prefix(crate::local::COVER).and_then(|name| crate::local::cover_in(&self.data_dir.join("local-covers"), name)) {
                        let bytes = tokio::task::spawn_blocking(move || std::fs::read(path)).await??;
                        tokio::task::spawn_blocking(move || crate::art::process(&bytes)).await??
                    } else {
                        // The backdrop is a 64 px blur; fetch near that, not the 544 px art.
                        let url = model::sized(&url, 2 * crate::art::SIZE);
                        let bytes = self.http().get(&url).send().await?.error_for_status()?.bytes().await?;
                        tokio::task::spawn_blocking(move || crate::art::process(&bytes)).await??
                    };
                    Resp::Art { key, art }
                }
                Req::Canvas { track } => {
                    let url = crate::canvas::find(&self.http(), &self.data_dir, &track).await;
                    Resp::CanvasUrl { id: track.id, url }
                }
                Req::SpotifyCanvas { key, title, artist, duration_ms } => {
                    let url = self.spotify_canvas(&key, &title, &artist, duration_ms).await;
                    if url.is_none() {
                        log::debug!("canvas: none for '{title}' by '{artist}'");
                    }
                    Resp::SpotifyCanvas { key, url }
                }
                Req::ArtistSignature { key, endpoint, name } => {
                    let signature = crate::artist_signature::lookup(&self.http(), &endpoint, &name).await;
                    log::info!("artist signature: {}", match &signature {
                        Some(s) => format!("found one for '{name}' ({}x{})", s.width, s.height),
                        None => format!("none for '{name}'"),
                    });
                    Resp::ArtistSignature { key, signature }
                }
                Req::Radio { seed, song_of, taste } => self.radio(seed, song_of, taste, false).await?,
                Req::AutoRadio { seed, song_of, taste } => self.radio(seed, song_of, taste, true).await?,
                Req::PlayCollection { target } => {
                    let (title, tracks) = match &target {
                        Target::Album(id) => ytm::album(api, id).await.map(|c| (c.title, c.tracks))?,
                        Target::Playlist(id) => ytm::playlist(api, id, me, None).await.map(|c| (c.title, c.tracks))?,
                        Target::Artist(id) => ytm::artist(api, id, false).await.map(|a| (a.name, a.top))?,
                        Target::Song(t) => (t.title.clone(), vec![t.clone()]),
                    };
                    Resp::PlayQueue { target, title, tracks }
                }
                Req::Download { track: t, for_video } => {
                    let (path, _) = self.download(&t, for_video, false).await?;
                    Resp::Downloaded { title: t.title, path }
                }
                Req::DownloadMany { target, title, tracks, confirmed } => match self.queue_downloads(target, title, tracks, confirmed, me).await? {
                    Some(r) => r,
                    None => return Ok(None),
                },
                Req::Play { video_id, generation, at, report, song_of, song, track } => match self.start_playback(&video_id, song_of.as_ref(), song, track.as_ref(), generation, at, report).await {
                    Ok(()) => return Ok(None),
                    Err(e) => {
                        log::warn!("play {video_id}: {e:#}");
                        Resp::PlayError { generation, msg: format!("{e:#}"), offer_backup_at: None }
                    }
                },
                Req::Prefetch { video_id, song_of, track } => {
                    self.prefetch(video_id, song_of, track).await?;
                    return Ok(None);
                }
                Req::VideoStream { video_id, max_height } => {
                    let found = async { innertube::video_stream(&self.http(), &video_id, &api.visitor_data(false).await?, max_height.into()).await }.await;
                    if let Err(e) = &found {
                        log::warn!("video of {video_id}: {e:#}");
                    }
                    Resp::VideoStream { video_id, max_height, stream: found.ok() }
                }
                Req::Like { video_id, like } => {
                    ytm::set_like(api, &video_id, like).await?;
                    Resp::Liked { video_id, like }
                }
                Req::Unlike(video_id) => match ytm::set_like(api, &video_id, false).await {
                    Ok(_) => Resp::Liked { video_id, like: false },
                    Err(e) => {
                        log::warn!("unliking {video_id}: {e:#}");
                        Resp::LikedMany { video_ids: Vec::new(), failed: vec![video_id], like: false }
                    }
                },
                Req::LikeMany { video_ids, like } => self.like_many(video_ids, like).await?,
                Req::CreatePlaylist { title, video_ids } => {
                    let id = match video_ids.as_slice() {
                        [] => ytm::create_playlist(api, &title, None).await?,
                        [one] => ytm::create_playlist(api, &title, Some(one)).await?,
                        many => ytm::create_playlist_with(api, &title, many).await?,
                    };
                    Resp::PlaylistCreated { id, songs: video_ids.len() }
                }
                Req::EditPlaylist { id, title, description, privacy } => {
                    ytm::edit_playlist(api, &id, &title, &description, privacy.as_deref()).await?;
                    Resp::PlaylistChanged { id, message: "Playlist updated" }
                }
                Req::AddToPlaylist { playlist_id, video_ids } => {
                    ytm::add_videos(api, &playlist_id, &video_ids).await?;
                    Resp::PlaylistChanged { id: playlist_id, message: "Added to playlist" }
                }
                Req::RemoveFromPlaylist { playlist_id, items } => {
                    ytm::remove_videos(api, &playlist_id, &items).await?;
                    Resp::PlaylistChanged { id: playlist_id, message: "Removed from playlist" }
                }
                Req::DeletePlaylist(id) => match ytm::delete_playlist(api, &id).await {
                    Ok(_) => Resp::PlaylistChanged { id, message: "Playlist deleted" },
                    Err(e) => {
                        log::warn!("deleting playlist {id}: {e:#}");
                        Resp::DeleteFailed { error: format!("Couldn't delete the playlist: {e}") }
                    }
                },
                Req::Collaboration(id) => Resp::Collaboration(ytm::collaboration(api, &id).await?),
                Req::SetCollaboration { info, change } => {
                    ytm::set_collaboration(api, &info, change).await?;
                    Resp::Collaboration(ytm::collaboration(api, &info.playlist_id).await?)
                }
                Req::SaveCollection { id, save } => {
                    ytm::save_collection(api, &id, save).await?;
                    Resp::PlaylistChanged { id, message: if save { "Saved to library" } else { "Removed from library" } }
                }
                // A failure is answered too, so the Sort button comes back.
                Req::SortPlaylist { id, actions } => match ytm::sort_playlist(api, &id, actions).await {
                    Ok(()) => Resp::PlaylistChanged { id, message: "Playlist sorted" },
                    Err(e) => {
                        log::warn!("sort playlist failed: {e:#}");
                        Resp::PlaylistChanged { id, message: "Couldn't sort the playlist" }
                    }
                },
                Req::MusicVideo(song) => Resp::MusicVideo { video: self.music_video(&song).await?, song: song.id },
                Req::Subscribe { channel_id, on } => {
                    let result = ytm::set_subscribed(api, &channel_id, on).await;
                    if let Err(e) = &result {
                        log::warn!("subscribe={on} {channel_id}: {e:#}");
                    }
                    Resp::Subscribed { channel_id, on, ok: result.is_ok() }
                }
                Req::SetCookie(input) => {
                    self.set_cookie(&input).await?;
                    *self.premium.lock().await = Err(None);
                    self.warm_signed_in();
                    log::info!("signed in");
                    Resp::LoggedIn(true)
                }
                Req::Logout => {
                    self.forget_session().await;
                    Resp::LoggedIn(false)
                }
            }))
        }
        .await;
        res.unwrap_or_else(|e| {
            log::warn!("{what} failed: {e:#}");
            (!quiet).then(|| Resp::Error(format!("{what} failed: {e}")))
        })
    }

    /// Explore: the charts, new albums by charting artists or the listener's own, and
    /// Moods & genres.
    async fn explore(&self, preferred_artists: &[String], me: bool) -> anyhow::Result<Resp> {
        let api = &*self.api;
        let (charts, new, moods) = tokio::join!(ytm::charts(api), ytm::new_albums(api, me), ytm::moods(api, me));
        let charts = charts?;
        Ok(Resp::Explore(Explore {
            new_albums: popular_new_albums(&new.unwrap_or_default(), &charts.artists, preferred_artists),
            charts: charts.playlists,
            top: charts.top,
            moods: moods.unwrap_or_default(),
        }))
    }

    /// A radio for `seed`, asked by the listener (`autoplay` false) or by the queue
    /// topping itself up: the sources Mochy's infinite queue falls through — the
    /// song's radio, the watch page's own up-next mix, the artist's songs — first
    /// one that isn't thin wins, and nothing the taste has ruled out survives any
    /// of them. The listener's keeps YouTube's order; the queue's is picked the way
    /// Home's rows are; the artist's own catalog plays as it lies (a spread of one
    /// song per act would gut it, and its order is its own curation).
    async fn radio(&self, seed: String, song_of: Option<Track>, taste: crate::recommend::TasteLite, autoplay: bool) -> anyhow::Result<Resp> {
        // A music video's radio is music videos; its song's is songs, as from the
        // song itself. The lookup is shared with its play's (one per video).
        let song = self.song_version(&seed, song_of.as_ref(), PREFETCH_SONG_WAIT).await;
        let from = song.as_ref().map_or(seed.as_str(), |s| &s.id);
        let artist = song.as_ref().or(song_of.as_ref()).and_then(|t| t.artists.iter().find_map(|a| a.id.clone()));
        let api = &*self.api;
        let mut best: Vec<Track> = Vec::new();
        let mut failure = None;
        for source in 0..3 {
            let fetched: anyhow::Result<Vec<Track>> = match source {
                0 => ytm::radio(api, from, false, RADIO_FETCH).await,
                1 => ytm::up_next(api, from, false, RADIO_FETCH).await,
                _ => match artist.as_deref() {
                    Some(id) => ytm::artist(api, id, false).await.map(|p| p.top),
                    None => Ok(Vec::new()),
                },
            };
            let mut tracks = match fetched {
                Ok(tracks) => tracks,
                Err(e) => {
                    failure.get_or_insert(e);
                    Vec::new()
                }
            };
            // the seed is playing already, or about to
            tracks.retain(|t| t.id != from && t.id != seed);
            tracks = if source == 2 || !autoplay {
                crate::recommend::user_radio(tracks, &taste)
            } else {
                crate::recommend::queue_radio(tracks, &taste, 2, &mut crate::recommend::Rng::new())
            };
            if tracks.len() > best.len() {
                best = tracks;
            }
            if best.len() >= RADIO_MIN {
                break;
            }
        }
        if best.is_empty() {
            // every source failed, or nothing survived the taste's rulings
            let e = failure.take().unwrap_or_else(|| anyhow::anyhow!("nothing to line up after {seed}"));
            return Err(e);
        }
        Ok(Resp::Radio { seed, tracks: best, autoplay })
    }

    /// Home's genre rows: up to three genres your top artists are filed under,
    /// weighted by listening, and the rest ones you don't play yet, each with nine
    /// songs off its page. The taste parts pick them; the artists' genres come from
    /// iTunes, which knows one where YouTube only names tiles.
    async fn genre_rows(&self, artists: &[(String, u64)], taste: &crate::recommend::TasteLite, me: bool) -> anyhow::Result<Resp> {
        let api = &*self.api;
        let hide = taste.hide_explicit;
        let groups = ytm::moods(api, me).await?;
        // the grid whose titles read as genres; the other one is moods
        let genres: Vec<(String, String)> = groups
            .iter()
            .max_by_key(|g| crate::recommend::genre_score(&g.moods.iter().map(|m| m.title.clone()).collect::<Vec<_>>()))
            .map(|g| g.moods.iter().map(|m| (m.title.clone(), m.params.clone())).collect())
            .filter(|g: &Vec<_>| !g.is_empty())
            .ok_or_else(|| anyhow::anyhow!("no genre grid"))?;
        let titles: Vec<String> = genres.iter().map(|(t, _)| t.clone()).collect();

        let http = self.http();
        let found = futures_util::future::join_all(artists.iter().map(|(name, secs)| {
            let http = &http;
            async move { (itunes_genre(http, name).await, *secs) }
        }))
        .await;
        let mut weights: std::collections::HashMap<usize, u64> = Default::default();
        for (genre, secs) in found {
            let Some(genre) = genre else { continue };
            let Some(i) = crate::recommend::match_genre(&genre, &titles) else { continue };
            *weights.entry(i).or_default() += secs.max(1);
        }
        let mut top: Vec<(usize, u64)> = weights.into_iter().collect();
        top.sort_by(|a, b| b.1.cmp(&a.1));
        let yours: Vec<usize> = top.into_iter().take(3).map(|(i, _)| i).collect();
        let mut fresh: Vec<usize> = (0..genres.len()).filter(|i| !yours.contains(i) && crate::recommend::is_genre(&titles[*i])).collect();
        crate::recommend::Rng::new().shuffle(&mut fresh);
        fresh.truncate(5usize.saturating_sub(yours.len()));

        let picks: Vec<(usize, crate::recommend::Kind)> = yours
            .iter()
            .map(|i| (*i, crate::recommend::Kind::YourGenre))
            .chain(fresh.iter().map(|i| (*i, crate::recommend::Kind::NewGenre)))
            .collect();
        let taste = crate::recommend::Taste::from_parts(taste);
        let rows = futures_util::future::join_all(picks.iter().map(|(i, kind)| {
            let taste = &taste;
            let params = genres[*i].1.clone();
            async move {
                let page = ytm::mood(api, &params, me).await.ok()?;
                // every song the page lists, a shuffle of it, then the pick
                let mut songs: Vec<Track> = Vec::new();
                for s in &page.shelves {
                    if let crate::model::ShelfKind::Songs { tracks, .. } = &s.kind {
                        songs.extend(tracks.iter().cloned());
                    }
                }
                songs.truncate(40);
                crate::recommend::Rng::new().shuffle(&mut songs);
                let tracks = crate::recommend::pick(songs, taste, hide, 9, 2, &mut crate::recommend::Rng::new());
                let title = page.title;
                (!tracks.is_empty()).then(|| crate::recommend::Row { kind: *kind, title, tracks, thumb: None, endpoint: Some(crate::recommend::Endpoint::Mood(params)) })
            }
        }))
        .await
        .into_iter()
        .flatten()
        .collect();
        Ok(Resp::RecoRows(rows))
    }

    /// "New from your artists": the newest album and single off each of the top
    /// artists' pages, this year's and last's, at least three of them.
    async fn new_from_artists(&self, ids: &[String], me: bool) -> anyhow::Result<Option<Vec<Card>>> {
        let api = &*self.api;
        let pages = futures_util::future::join_all(ids.iter().map(|id| async move { ytm::artist(api, id, me).await.ok() })).await;
        let last_year = crate::listening::unix_now() / 31_557_600 + 1970 - 1;
        let mut releases: Vec<Card> = pages
            .iter()
            .flatten()
            // "Albums" and "Singles & EPs" list the newest first
            .flat_map(|page| page.albums.iter().chain(&page.singles).take(2))
            .filter(|c| card_year(c).is_some_and(|y| y >= last_year))
            .cloned()
            .collect();
        let mut seen = std::collections::HashSet::new();
        releases.retain(|c| {
            let id = match &c.target {
                Target::Album(id) | Target::Playlist(id) | Target::Artist(id) => Some(id.as_str()),
                Target::Song(_) => None,
            };
            id.map(|id| seen.insert(id.to_owned())).unwrap_or(true)
        });
        releases.sort_by(|a, b| card_year(b).cmp(&card_year(a)));
        releases.truncate(12);
        Ok((releases.len() >= 3).then_some(releases))
    }

    /// Songs, videos, albums, artists and playlists for a search, asked at once (podcast
    /// shows and episodes too when those are shown). Only the songs must arrive.
    async fn search(&self, query: String, me: bool) -> anyhow::Result<Resp> {
        let api = &*self.api;
        let podcasts = async {
            if !ytm::podcasts() {
                return (Vec::new(), Vec::new());
            }
            let (shows, episodes) = tokio::join!(ytm::search_cards(api, &query, ytm::Filter::Podcasts, me), ytm::search_episodes(api, &query, me));
            (shows.unwrap_or_default(), episodes.unwrap_or_default())
        };
        let (t, v, al, ar, pl, (podcasts, episodes)) = tokio::join!(
            ytm::search_songs(api, &query, me),
            ytm::search_videos(api, &query, me),
            ytm::search_cards(api, &query, ytm::Filter::Albums, me),
            ytm::search_cards(api, &query, ytm::Filter::Artists, me),
            ytm::search_cards(api, &query, ytm::Filter::Playlists, me),
            podcasts,
        );
        Ok(Resp::Search(SearchResults {
            scope: ytm::Scope::Everywhere,
            tracks: t?,
            videos: v.unwrap_or_default(),
            albums: al.unwrap_or_default(),
            artists: ar.unwrap_or_default(),
            playlists: pl.unwrap_or_default(),
            podcasts,
            episodes,
            query,
        }))
    }

    /// A search of your library or uploads, laid out as the search page shows any search:
    /// songs apart from videos, and the pages it found by kind.
    fn search_in(found: ytm::ScopedResults, query: String, scope: ytm::Scope) -> SearchResults {
        let (videos, tracks) = found.tracks.into_iter().partition(|t| t.video);
        let mut r = SearchResults { query, scope, tracks, videos, albums: Vec::new(), artists: Vec::new(), playlists: Vec::new(), podcasts: Vec::new(), episodes: found.episodes };
        for c in found.cards {
            match &c.target {
                Target::Artist(_) => r.artists.push(c),
                Target::Album(_) => r.albums.push(c),
                Target::Playlist(id) if ytm::list_id(id) != id => r.podcasts.push(c),
                Target::Playlist(_) => r.playlists.push(c),
                Target::Song(_) => {}
            }
        }
        r
    }

    /// Saved playlists and albums.
    async fn library(&self) -> anyhow::Result<Resp> {
        let api = &*self.api;
        // Liked songs are Liked Music, loaded like any playlist. Each half shows
        // without the other; it only fails when both do.
        let podcasts = async {
            if !ytm::podcasts() {
                return Vec::new();
            }
            ytm::saved_podcasts(api).await.unwrap_or_else(|e| {
                log::warn!("saved podcasts didn't load: {e:#}");
                Vec::new()
            })
        };
        let artists = async {
            ytm::subscribed_artists(api).await.inspect_err(|e| log::warn!("subscribed artists didn't load: {e:#}"))
        };
        let (pls, albums, podcasts, artists) = tokio::join!(ytm::saved_playlists(api), ytm::saved_albums(api), podcasts, artists);
        let pls = match (pls, &albums) {
            (Err(e), Err(_)) => return Err(e),
            (pls, _) => pls,
        };
        if let Err(e) = &pls {
            log::warn!("saved playlists didn't load: {e:#}");
        }
        if let Err(e) = &albums {
            log::warn!("saved albums didn't load: {e:#}");
        }
        Ok(Resp::Library(Library {
            playlists_failed: pls.is_err(),
            albums_failed: albums.is_err(),
            artists_failed: artists.is_err(),
            playlists: pls.map(|p| p.into_iter().filter(saved_playlist_visible).collect()).unwrap_or_default(),
            albums: albums.unwrap_or_default(),
            podcasts,
            artists: artists.unwrap_or_default(),
        }))
    }

    /// Upload songs, then refresh the uploads while YouTube Music processes them.
    async fn upload(&self, files: &[PathBuf]) -> anyhow::Result<Resp> {
        let api = &*self.api;
        let session = api.account().ok_or_else(|| anyhow::anyhow!("sign in to upload music"))?;
        let (mut done, mut failed) = (0, Vec::new());
        for f in files {
            let name = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            match session.upload(&self.http(), f).await {
                Ok(()) => done += 1,
                Err(e) => {
                    log::warn!("upload {name}: {e:#}");
                    failed.push(name);
                }
            }
        }
        if done > 0 {
            // YouTube Music needs a little while to process new uploads.
            let this = self.clone();
            self.rt.spawn(async move {
                for wait in [20, 40, 60] {
                    tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
                    if let Ok(t) = ytm::uploads(&this.api).await {
                        let _ = this.tx.send(Resp::Uploads(t));
                        this.wake();
                    }
                }
            });
        }
        Ok(Resp::Uploaded { done, failed })
    }

    /// Open the next song's stream ahead of time, once the playing one has downloaded.
    /// `song_of`: the music video it stands for, whose song version is what will play.
    /// `track`: the song as listed, for a hi-res source's search.
    async fn prefetch(&self, video_id: String, song_of: Option<Track>, track: Option<Track>) -> anyhow::Result<()> {
        // For the play it was asked during: a skip in the meantime makes it moot, and so
        // does a later prefetch (the queue changed: another song is next now).
        static LATEST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let ticket = LATEST.fetch_add(1, Ordering::Relaxed) + 1;
        let backup_stream_revision = self.backup_stream_revision.load(Ordering::Relaxed);
        let generation = self.audio.status.wanted_gen.load(Ordering::Relaxed);
        // Looked up now, so the play doesn't wait for it.
        let song = self.song_version(&video_id, song_of.as_ref(), PREFETCH_SONG_WAIT).await;
        let meta = song.clone().or_else(|| song_of.clone()).or_else(|| track);
        let video_id = song.map_or(video_id, |s| s.id);
        let moot = || {
            self.audio.status.wanted_gen.load(Ordering::Relaxed) != generation
                || LATEST.load(Ordering::Relaxed) != ticket
                || self.prefetched.lock().unwrap().as_ref().is_some_and(|(id, _)| *id == video_id)
        };
        if moot() {
            return Ok(());
        }
        // The URL is looked up meanwhile, but the playing song finishes downloading
        // first: it gets the whole bandwidth.
        let playing = self.current.lock().unwrap().as_ref().map(|(_, s)| s.clone());
        let (found, ()) = tokio::join!(self.find_stream(&video_id, meta.as_ref(), true), async {
            if let Some(s) = playing {
                s.finished().await;
            }
        });
        if moot() {
            return Ok(());
        }
        let mut prefetched = self.prefetched.lock().unwrap();
        // Even a switch back to the original setting can leave a stream from the other mode.
        if self.backup_stream_revision.load(Ordering::Relaxed) != backup_stream_revision {
            return Ok(());
        }
        log::debug!("prefetch {video_id}");
        let (found, deferred_addons) = found?;
        let shared = self.start_stream(&video_id, found, true);
        if let Some((_, (old, _))) = prefetched.replace((video_id, (shared, deferred_addons))) {
            old.cancel();
        }
        Ok(())
    }

    async fn lyrics_for(&self, t: &Track) -> std::sync::Arc<LyricsSet> {
        let artist = t.artist_line();
        // The playing video's own sources, alongside the outside ones: its captions
        // and its Lyrics tab, straight from YouTube (ForkChord's YouTubeLyrics.kt).
        // A replay's cached copy already carries them (they're folded in below), so
        // the two API calls are skipped.
        let cached = self.lyrics.cached_for(&t.id);
        let have_ytm = cached.as_ref().is_some_and(|s| s.sources.iter().any(|l| matches!(l.provider, lyrics::Provider::YoutubeCaptions | lyrics::Provider::YoutubePlain)));
        let query = lyrics::Query {
            video_id: &t.id,
            song: &t.title,
            artist: &artist,
            album: t.album.as_ref().map(|a| a.name.as_str()),
            duration: match t.duration {
                Some(d) => Some(d),
                None => self.stream_duration(&t.id).await,
            },
        };
        let (res, caps, tab) = if have_ytm {
            (self.lyrics.fetch(&query).await, Vec::new(), Vec::new())
        } else {
            tokio::join!(self.lyrics.fetch(&query), crate::ytm::captions(&self.api, &t.id), crate::ytm::lyrics_tab(&self.api, &t.id))
        };
        let set = res.unwrap_or_else(|e| {
            log::warn!("lyrics failed: {e:#}");
            std::sync::Arc::new(LyricsSet::default())
        });
        let mut extra = caps;
        extra.extend(tab);
        extra.retain(|l| !set.sources.iter().any(|s| s.provider == l.provider));
        let mut set = if extra.is_empty() {
            set
        } else {
            // YouTube's sources arrived after `fetch` cached the song: the grown set
            // replaces the cached one, so replays keep them.
            let mut sources = set.sources.clone();
            sources.extend(extra.iter().cloned());
            sources.sort_by_key(|l| lyrics::Provider::ALL.iter().position(|p| *p == l.provider));
            let grown = std::sync::Arc::new(LyricsSet { sources });
            self.lyrics.replace_cached(&t.id, grown.clone());
            grown
        };
        // Dev hook: YTM_LYRICS_FAIL=<source key>[,<key>…] makes those sources come back empty.
        if let Ok(fail) = std::env::var("YTM_LYRICS_FAIL") {
            let mut filtered = (*set).clone();
            filtered.sources.retain(|l| !fail.split(',').any(|k| k.trim() == l.provider.key()));
            set = std::sync::Arc::new(filtered);
        }
        set
    }

    /// The song's length from its stream, for a track listed without one (some home,
    /// radio and queue rows). Without a duration Better Lyrics only finds plain lyrics,
    /// so wait for playback to resolve the stream, up to 15 s. The player's own duration
    /// counts too: starting a song clears it, and a restored session sets it.
    async fn stream_duration(&self, video_id: &str) -> Option<u32> {
        for _ in 0..150 {
            let ms = self.current.lock().unwrap().as_ref().filter(|(id, _)| id == video_id).map(|(_, s)| s.duration_ms);
            let ms = ms.filter(|ms| *ms > 0).or_else(|| self.audio.status.duration().map(|d| d.as_millis() as u64));
            if let Some(ms) = ms.filter(|ms| *ms > 0) {
                return Some((ms / 1000) as u32);
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        log::info!("lyrics: no duration for {video_id}");
        None
    }

    /// Sign in with the youtube.com cookies the Google sign-in window hands over.
    async fn set_cookie(&self, cookie: &str) -> anyhow::Result<()> {
        let cookie = cookie.trim();
        anyhow::ensure!(!cookie.is_empty(), "no youtube.com cookies found");
        let session = self.api.session_from_cookie(cookie).await.inspect_err(|e| {
            // Names only, never values: enough to see whether SAPISID etc. arrived.
            let names: Vec<&str> = cookie.split(';').filter_map(|kv| kv.trim().split('=').next()).take(40).collect();
            log::warn!("cookie sign-in rejected: {e:#}; cookie names: {names:?}");
        })?;
        self.api.sign_in(session);
        // The sign-in window's copy of the session would go stale as the app takes new
        // cookies, and opening it again would show Google the old ones, which it treats
        // as stolen and signs the session out. Keep only the app's copy.
        let profile = self.login_profile();
        let _ = tokio::task::spawn_blocking(move || crate::login::forget(&profile)).await;
        Ok(())
    }

    async fn start_playback(&self, listed: &str, song_of: Option<&Track>, carried: Option<Track>, track: Option<&Track>, generation: u64, at: std::time::Duration, report: bool) -> anyhow::Result<()> {
        let t0 = std::time::Instant::now();
        // A music video plays as its song when one is found in time (or goes on as the
        // one it was). From here on, everything (prefetch, replay, history) goes by what
        // actually plays.
        let was_carried = carried.is_some();
        let mut early = EarlyVideo(None);
        let mut song = match carried {
            Some(song) => Some(song),
            None => {
                let lookup = self.song_version(listed, song_of, PLAY_SONG_WAIT);
                tokio::pin!(lookup);
                match tokio::time::timeout(EARLY_VIDEO_AFTER, &mut lookup).await {
                    Ok(song) => song,
                    Err(_) => {
                        // Just its start, until it turns out to be the one that plays.
                        let (this, id) = (self.clone(), listed.to_owned());
                        let meta = song_of.cloned().or_else(|| track.cloned());
                        early.0 = Some(tokio::spawn(async move { this.open_stream(&id, true, meta.as_ref()).await }));
                        lookup.await
                    }
                }
            }
        };
        // What a hi-res source is asked for: the song that will play, or the video itself.
        let meta = song.clone().or_else(|| song_of.cloned()).or_else(|| track.cloned());
        if self.audio.status.wanted_gen.load(Ordering::Relaxed) != generation {
            return Ok(());
        }
        let mut video_id = song.as_ref().map_or(listed, |s| s.id.as_str()).to_owned();
        // A reverted song must not replay its buffered hi-res copy: it re-resolves to YouTube.
        let reverted = self.reverted.lock().unwrap().contains(&video_id);
        let replay = self.current.lock().unwrap().take().filter(|(id, s)| !reverted && *id == video_id && s.is_complete()).map(|(_, s)| s);
        let ready = {
            let mut prefetched = self.prefetched.lock().unwrap();
            match prefetched.take() {
                // Not one whose download failed (refused, say): that opens afresh.
                // Nor a file stream fetched before file streams turned out to be cut off.
                Some((id, (s, deferred))) if id == video_id && s.error().is_none() && !(self.visionos_hls.load(Ordering::Relaxed) && s.segments.is_none() && !s.is_complete()) => Some((s, deferred)),
                // Replaying the current song keeps the next one's prefetch.
                other if replay.is_some() => {
                    *prefetched = other;
                    replay.map(|s| (s, None))
                }
                // Playback went elsewhere: that prefetch is moot.
                other => {
                    if let Some((_, (s, _))) = other {
                        s.cancel();
                    }
                    None
                }
            }
        };
        let (shared, deferred_addons) = match ready {
            Some(s) => {
                log::debug!("play {video_id} (gen {generation}): using buffered stream");
                s
            }
            None => {
                if self.audio.status.wanted_gen.load(Ordering::Relaxed) != generation {
                    return Ok(());
                }
                // Taken only for the video; otherwise dropping `early` stops it.
                let opened = match early.0.take_if(|_| song.is_none()) {
                    Some(open) => {
                        log::debug!("play {video_id} (gen {generation}): stream opened during the song lookup");
                        open.await.map_err(anyhow::Error::from).flatten()
                    }
                    None => self.open_stream(&video_id, false, meta.as_ref()).await,
                };
                match opened {
                    Ok(s) => s,
                    // The song won't play: the music video still does.
                    Err(e) if song.is_some() => {
                        log::warn!("{listed}'s song {video_id} won't play ({e:#}); playing the music video");
                        (song, video_id) = (None, listed.to_owned());
                        // The UI took the song as playing already.
                        if was_carried {
                            let _ = self.tx.send(Resp::PlayingAs { generation, listed: listed.to_owned(), song: None });
                        }
                        let opened = match early.0.take() {
                            Some(open) => open.await.ok().and_then(Result::ok),
                            None => None,
                        };
                        match opened {
                            Some(s) => s,
                            None => self.open_stream(listed, false, None).await?,
                        }
                    }
                    Err(e) => return Err(e),
                }
            }
        };
        // If the song's audio won't start after all, the music video plays instead.
        let fallback = song.is_some().then(|| (self.clone(), listed.to_owned(), report));
        if let Some(song) = song {
            log::info!("play {listed} as its {} {video_id}", if song.has_video() { "music video" } else { "song" });
            // Carried over, the UI has it already.
            if !was_carried {
                let _ = self.tx.send(Resp::PlayingAs { generation, listed: listed.to_owned(), song: Some(song) });
                (self.wake)();
            }
        }
        *self.current.lock().unwrap() = Some((video_id.clone(), shared.clone()));
        log::debug!("play {video_id} (gen {generation}): stream open in {:?}", t0.elapsed());
        if self.audio.status.wanted_gen.load(Ordering::Relaxed) != generation {
            shared.cancel();
            return Ok(());
        }

        let audio = self.audio.clone();
        let tx = self.tx.clone();
        let wake = self.wake.clone();
        // A local play is the listener's own file: nothing to report to YouTube, and no
        // addon is asked to better the copy they picked.
        let local = crate::local::is_local(listed);
        let report = (report && !local && self.is_logged_in() && reports_plays()).then(|| (self.clone(), video_id.clone()));
        // The background swap is gated on a strictly higher tier.
        let upgrade = (!local).then(|| (self.clone(), video_id.clone(), meta));
        let this = self.clone();
        self.rt.spawn(async move {
            let mut shared2 = shared;
            let mut decoded = playback_decoder(shared2.clone(), at).await;
            if decoded.is_err()
                && fallback.is_none()
                && audio.status.wanted_gen.load(Ordering::Relaxed) == generation
                && let Some(deferred) = deferred_addons
            {
                shared2.cancel();
                if let Some(found) = this.startup_addon(&video_id, &deferred).await {
                    let retry = this.start_stream(&video_id, found, false);
                    {
                        let mut current = this.current.lock().unwrap();
                        if audio.status.wanted_gen.load(Ordering::Relaxed) != generation
                            || !current.as_ref().is_some_and(|(_, active)| Arc::ptr_eq(active, &shared2))
                        {
                            retry.cancel();
                            return;
                        }
                        *current = Some((video_id.clone(), retry.clone()));
                    }
                    shared2 = retry;
                    decoded = playback_decoder(shared2.clone(), at).await;
                }
            }
            if audio.status.wanted_gen.load(Ordering::Relaxed) != generation {
                shared2.cancel();
                return;
            }
            match decoded {
                Ok(decoder) => {
                    log::debug!("play gen {generation}: decoder ready at {at:?} in {:?}", t0.elapsed());
                    let _ = tx.send(Resp::StreamStats { generation, stats: stream::Stats::of(&shared2, &decoder) });
                    // What a hi-res stream is, as the decoder actually reads it.
                    if let Some(mut info) = shared2.info.clone() {
                        info.sample_rate = Some(rodio::Source::sample_rate(&decoder).get());
                        info.channels = Some(rodio::Source::channels(&decoder).get() as u8);
                        let _ = tx.send(Resp::AudioInfo { generation, info });
                    }
                    wake();
                    audio.send(AudioCmd::Start { generation, decoder, shared: shared2, at });
                    // Skipped past before it started: that one doesn't count.
                    if let Some((this, id)) = report.filter(|_| audio.status.wanted_gen.load(Ordering::Relaxed) == generation) {
                        let rt = this.rt.clone();
                        rt.spawn(async move {
                            // The player request needs the current signatureTimestamp, or YouTube
                            // leaves out the stats URL. Without a solver, try anyway as before.
                            let sts = match this.jsc.player(&this.http()).await {
                                Ok(p) => Some(p.sts),
                                Err(e) => {
                                    log::debug!("report play of {id} without signatureTimestamp: {e:#}");
                                    None
                                }
                            };
                            match ytm::report_play(&this.api, &id, sts).await {
                                Ok(w) => {
                                    log::debug!("reported play of {id}");
                                    this.report_watchtime(&id, generation, w).await;
                                }
                                Err(e) => log::warn!("couldn't report play of {id}: {e:#}"),
                            }
                        });
                    }
                    // Started from YouTube: look for a hi-res copy in the background, and
                    // swap it in place if one answers while the song is still the one wanted.
                    if let Some((this, id, Some(track))) = upgrade.filter(|_| audio.status.wanted_gen.load(Ordering::Relaxed) == generation) {
                        let rt = this.rt.clone();
                        rt.spawn(async move {
                            this.try_upgrade_hires(id, generation, track, false).await;
                        });
                    }
                }
                Err(e) => match fallback {
                    Some((this, listed, report)) => {
                        log::warn!("{listed}'s song won't start ({e}); playing the music video");
                        let _ = tx.send(Resp::PlayingAs { generation, listed: listed.clone(), song: None });
                        this.request(Req::Play { video_id: listed, generation, at, report, song_of: None, song: None, track: None });
                        wake();
                    }
                    None => {
                        let (msg, offer_backup_at) = match shared2.failure() {
                            Some(failure) => (failure.reason, failure.offer_backup.then_some(at)),
                            None => (format!("decode: {e}"), None),
                        };
                        let _ = tx.send(Resp::PlayError { generation, msg, offer_backup_at });
                        wake();
                    }
                },
            }
        });
        Ok(())
    }

    /// Report how much of play `generation` is heard, on the web player's schedule
    /// (10, 20 and 30 s in, then every 40 s), with a last report once it ends or is left.
    /// Each covers the stretch played since the one before; time paused doesn't count.
    async fn report_watchtime(&self, id: &str, generation: u64, w: ytm::Watchtime) {
        // Reported as the sign-in and channel that started it, or not at all.
        let epoch = self.api.epoch();
        let status = &self.audio.status;
        let started = std::time::Instant::now();
        let mut flush = w.flush_at.iter().copied();
        let mut due = flush.next().unwrap_or(w.flush_every);
        // The stretch not yet reported, and where the song was at the last check.
        let pos = || status.position().as_secs_f64();
        let (mut from, mut at) = (pos(), pos());
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            let gone = status.wanted_gen.load(Ordering::Relaxed) != generation;
            if !gone {
                let now = pos();
                // A seek starts a new stretch.
                if now < at || now > at + 5.0 {
                    from = now;
                }
                at = now;
            }
            let elapsed = started.elapsed().as_secs();
            if !gone && elapsed < due {
                continue;
            }
            if self.api.epoch() != epoch {
                log::debug!("stopped reporting listening time of {id}: the account or channel changed");
                return;
            }
            if at > from || gone {
                if let Err(e) = ytm::report_watchtime(&self.api, &w, from, at, started.elapsed().as_secs_f64(), gone).await {
                    log::warn!("couldn't report listening time of {id}: {e:#}");
                }
                from = at;
            }
            if gone {
                log::debug!("reported listening time of {id}");
                return;
            }
            due = flush.next().unwrap_or(due + w.flush_every);
        }
    }

    /// Resolve a stream URL and start downloading it into a new buffer (just its start,
    /// for a prefetch, until playback opens it).
    /// A downloaded song opens from its file.
    /// The song version of the music video `listed`, when `song_of` asks for one: from
    /// this session's lookups, YouTube's own pair (Premium), or a sure signed-out search
    /// match. `None` when there's none, it isn't found within `wait`, or the music video
    /// is downloaded (that file plays, offline too). A failed lookup isn't remembered,
    /// so the next play tries again.
    async fn song_version(&self, listed: &str, song_of: Option<&Track>, wait: std::time::Duration) -> Option<Track> {
        // A local song is never a music video: there's nothing to look up.
        if crate::local::is_local(listed) {
            return None;
        }
        let video = song_of.filter(|t| t.id == listed && t.music_video)?;
        if crate::downloads::path(listed).is_some() {
            return None;
        }
        // Its song version downloaded from Now playing: from the file, no lookup (offline too).
        let clean = self.clean.load(Ordering::Relaxed);
        if let Some(song) = crate::downloads::song_for(listed).filter(|s| !(clean && s.explicit)) {
            return Some(song);
        }
        let start = std::time::Instant::now();
        // Premium gets YouTube's own pair; a search needs the video's length to be sure.
        // The check counts toward the wait (the first one after launch asks YouTube): not
        // back in time, this lookup searches. It finishes anyway, for the next song.
        let premium = match self.api.account() {
            Some(session) => {
                let this = self.clone();
                let check = tokio::spawn(async move { this.is_premium(&session).await });
                tokio::time::timeout(wait, check).await.is_ok_and(|p| p.unwrap_or(false))
            }
            None => false,
        };
        if !premium && (video.duration.is_none() || video.artists.is_empty()) {
            return None;
        }
        let cell = {
            let mut songs = self.songs.lock().unwrap();
            let key = format!("{listed} {premium} {clean}");
            match songs.get(&key) {
                Some(cell) => cell.clone(),
                None => {
                    if songs.len() >= 1000 {
                        // Keeps the lookups still running.
                        songs.retain(|_, c| !c.initialized());
                    }
                    songs.entry(key).or_default().clone()
                }
            }
        };
        // Its own task: when the wait runs out the lookup still finishes, so the next play
        // of this video finds the song in the cell.
        let (this, video) = (self.clone(), video.clone());
        let lookup = tokio::spawn(async move {
            cell.get_or_try_init(|| async {
                let found = this.look_up_song(&video, premium, clean).await?;
                match &found {
                    Some(s) => log::info!("song version of {} \"{}\": {} \"{}\"", video.id, video.title, s.id, s.title),
                    None => log::info!("song version of {} \"{}\": none found", video.id, video.title),
                }
                anyhow::Ok(found)
            })
            .await
            .cloned()
        });
        match tokio::time::timeout(wait.saturating_sub(start.elapsed()), lookup).await {
            Ok(Ok(Ok(found))) => found,
            Ok(Ok(Err(e))) => {
                log::debug!("song version of {listed}: {e:#}");
                None
            }
            Ok(Err(e)) => {
                log::debug!("song version of {listed}: lookup stopped: {e}");
                None
            }
            Err(_) => {
                log::debug!("song version of {listed}: not found in {wait:?}, playing the music video");
                None
            }
        }
    }

    /// clean: only a song that isn't explicit.
    async fn look_up_song(&self, video: &Track, premium: bool, clean: bool) -> anyhow::Result<Option<Track>> {
        if premium {
            match ytm::paired_song(&self.api, &video.id).await {
                // YouTube pairs every music video that has a song; trust it when it says none.
                Ok(found) if !(clean && found.as_ref().is_some_and(|s| s.explicit)) => return Ok(found),
                // Paired with its explicit version: a search may find the clean one.
                Ok(_) => {}
                Err(e) => log::debug!("song pair of {}: {e:#}; searching instead", video.id),
            }
        }
        ytm::searched_song(&self.api, video, clean).await
    }

    async fn open_stream(&self, video_id: &str, prefetch: bool, track: Option<&Track>) -> anyhow::Result<Opened> {
        let (found, deferred) = self.find_stream(video_id, track, prefetch).await?;
        Ok((self.start_stream(video_id, found, prefetch), deferred))
    }

    /// The saved download, a file of this computer's, or a stream URL to download from.
    /// `prefetch`: only somewhere to start is needed, so an addon rendition that has to
    /// be fetched and decoded whole is left for the play.
    async fn find_stream(&self, video_id: &str, track: Option<&Track>, prefetch: bool) -> anyhow::Result<(Found, DeferredAddons)> {
        let id = video_id.to_owned();
        if let Some(shared) = tokio::task::spawn_blocking(move || crate::downloads::open(&id).or_else(|| crate::local::open(&id))).await? {
            return Ok((Found::Saved(shared), None));
        }
        // With "play YouTube first" on, the addons are left to the background upgrade
        // instead of the play path.
        let allow_hires = !self.disable_direct.load(Ordering::Relaxed);
        let mut deferred = None;
        let (source, s) = self.resolve(video_id, false, track, allow_hires, prefetch, &mut deferred).await?;
        Ok((Found::Url(source, s), deferred))
    }

    async fn startup_addon(&self, video_id: &str, deferred: &(Vec<Arc<hires::Client>>, Track)) -> Option<Found> {
        if self.disable_direct.load(Ordering::Relaxed) || !self.source_on(MusicSource::Addons) || self.reverted.lock().unwrap().contains(video_id) {
            return None;
        }
        let (sources, track) = deferred;
        self.from_hires(sources, track, hires::Lookup::Startup).await.map(|s| Found::Url(Source::HiRes, s))
    }

    fn start_stream(&self, video_id: &str, found: Found, prefetch: bool) -> Arc<Shared> {
        let (source, s) = match found {
            Found::Saved(shared) => return shared,
            Found::Url(source, s) => (source, s),
        };
        // Atmos arrives already decoded: a complete buffer, no download to run.
        if let Some(data) = s.data {
            return Shared::from_bytes_with_format(data, s.duration_ms, s.loudness_db, s.format, s.info);
        }
        match (&s.starts, &s.media) {
            (Some(starts), _) => log::info!("stream {video_id}: {} HLS segments from {source:?}", starts.len()),
            (None, _) => log::info!("stream {video_id}: {} bytes {} from {source:?}", s.size, s.kind()),
        }
        let shared = s.buffer(prefetch);
        if source == Source::YtDlpAfterBotCheck {
            shared.mark_after_bot_check();
        }
        let refresh = Some(self.refresher(source, video_id, &s));
        let (this, buffer) = (self.clone(), shared.clone());
        self.rt.spawn(async move {
            stream::download(this.http(), s.media, s.user_agent, buffer.clone(), refresh, s.ranging).await;
            // Fresh visionOS file URLs refused too: YouTube wants a PO token for them
            // here, past the first MiB. Its HLS segments play without one; the song
            // reopens where it stopped, and the rest of the session goes that way.
            if source == Source::VisionOs && buffer.refused() && !this.visionos_hls.swap(true, Ordering::Relaxed) {
                log::warn!("visionOS file streams are refused here past 1 MiB; playing HLS from now on");
            }
        });
        shared
    }

    /// Resolves a fresh URL from the same source and format, for a download that got a 403.
    fn refresher(&self, source: Source, video_id: &str, stream: &ResolvedStream) -> stream::Refresh {
        let (this, id) = (self.clone(), video_id.to_owned());
        let (format, size, selection) = (stream.format.clone(), stream.size, stream.selection.clone());
        let starts = stream.starts.as_deref().map(Arc::<[f64]>::from);
        let once = matches!(&selection, Some(RefreshSelection::Addon(_)));
        stream::Refresh { once, callback: Arc::new(move || {
            let (this, id, format, selection, starts) = (this.clone(), id.clone(), format.clone(), selection.clone(), starts.clone());
            Box::pin(async move {
                let (media, refreshed_format, refreshed_size) = match selection {
                    Some(RefreshSelection::Provider(provider)) => {
                        let (url, format, size) = provider.refresh(&this.http(), &this.monochrome).await?;
                        (stream::Media::File(url), format, size)
                    }
                    Some(RefreshSelection::Addon(addon)) => {
                        let s = tokio::time::timeout(HIRES_WAIT, addon.refresh(&this.http())).await
                            .map_err(|_| anyhow::anyhow!("addon playback refresh timed out"))??;
                        (stream::Media::File(s.url), s.format, s.size)
                    }
                    None => {
                        let s = this.resolve_from(source, &id, format.clone()).await?;
                        (s.media, s.format, s.size)
                    }
                };
                // The buffer is laid out for the format it started with.
                anyhow::ensure!(refreshed_format == format, "the stream changed format");
                // A podcast host may put other ads in, which makes another file.
                anyhow::ensure!(refreshed_size == size, "the stream changed size");
                anyhow::ensure!(same_segments(source, starts.as_deref(), s.starts.as_deref()), "the stream changed segment boundaries");
                Ok(media)
            })
        }) }
    }

    /// Download a song into the downloads folder, at the best quality the account gets.
    /// `behind_player`: each part waits while the playing song still needs some of its own.
    /// Returns the file, and whether this saved it (false: it was there already).
    async fn download(&self, t: &Track, for_video: Option<String>, behind_player: bool) -> anyhow::Result<(PathBuf, bool)> {
        // One save of a song at a time: one on its way already, this waits and takes its file.
        let _saving = loop {
            if self.batch.lock().unwrap().saving.insert(t.id.clone()) {
                break Saving(self.batch.clone(), t.id.clone());
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        };
        if let Some(path) = reuse_download(&t.id, for_video.clone()).await {
            return Ok((path, false));
        }
        let (source, s) = self.resolve(&t.id, true, Some(t), false, false, &mut None).await?;
        log::info!("download {}: {} bytes from {source:?}", t.id, s.size);
        // Downloads stay on the file stream: they're saved as the .m4a it is.
        anyhow::ensure!(s.starts.is_none(), "downloads need the file stream");
        // Saved as a tagged .m4a, which a feed's MP3 isn't.
        anyhow::ensure!(!matches!(s.media, stream::Media::Feed(_)), "episodes of podcasts added by RSS can't be downloaded");
        let shared = s.buffer(false);
        let ahead: Option<stream::Ahead> = behind_player.then(|| {
            let current = self.current.clone();
            Arc::new(move || current.lock().unwrap().as_ref().map(|(_, s)| s.clone())) as stream::Ahead
        });
        stream::download_behind(self.http(), s.media, s.user_agent, shared.clone(), Some(self.refresher(source, &t.id, s.format.clone(), s.size, s.selection)), s.ranging, ahead).await;
        if let Some(e) = shared.error() {
            anyhow::bail!("{e}");
        }
        let audio = shared.bytes().ok_or_else(|| anyhow::anyhow!("the download stopped early"))?;
        let cover = match model::pick(&t.thumbs, 120).map(|u| sized(u, 1200)) {
            Some(url) => async { self.http().get(url).send().await?.error_for_status()?.bytes().await }.await.ok().map(|b| b.to_vec()),
            None => None,
        };
        let (t, duration_ms, loudness) = (t.clone(), shared.duration_ms, shared.loudness_db);
        let path = tokio::task::spawn_blocking(move || crate::downloads::save(&t, &audio, duration_ms, loudness, cover, for_video)).await??;
        Ok((path, true))
    }

    /// Queue the songs of an album, playlist or selection that aren't downloaded yet (see
    /// `Req::DownloadMany`) and start downloading them. Every run as it stands goes to the
    /// UI from here, in order with the workers' updates; a large one is asked about first.
    async fn queue_downloads(&self, target: Option<Target>, title: String, tracks: Option<Vec<Track>>, confirmed: bool, me: bool) -> anyhow::Result<Option<Resp>> {
        let (title, tracks) = match (tracks, &target) {
            (Some(tracks), _) => (title, tracks),
            (None, Some(Target::Album(id))) => ytm::album(&self.api, id).await.map(|c| (c.title, c.tracks))?,
            (None, Some(Target::Playlist(id))) => ytm::playlist(&self.api, id, me, None).await.map(|c| (c.title, c.tracks))?,
            (None, _) => anyhow::bail!("nothing to download"),
        };
        // An empty playlist (from its card, which can't tell): nothing to run.
        if tracks.is_empty() {
            let _ = self.tx.send(Resp::Error(format!("\u{201c}{title}\u{201d} has no songs to download")));
            return Ok(None);
        }
        let key = match &target {
            Some(Target::Album(id) | Target::Playlist(id)) => Some(id.clone()),
            _ => None,
        };
        // Each song once. Looking for their files is disk work.
        let (listed, todo) = tokio::task::spawn_blocking(move || {
            let mut seen = std::collections::HashSet::new();
            let listed: Vec<Track> = tracks.into_iter().filter(|t| seen.insert(t.id.clone())).collect();
            let todo: Vec<Track> = listed.iter().filter(|t| !is_downloaded(t)).cloned().collect();
            (listed, todo)
        })
        .await?;
        if !confirmed {
            // Those on their way for this run already aren't asked about again.
            let new = {
                let q = self.batch.lock().unwrap();
                todo.iter().filter(|t| key.as_ref().is_none_or(|k| !q.wanted.get(&t.id).is_some_and(|ks| ks.contains(k)))).count()
            };
            if new > LARGE_DOWNLOAD {
                return Ok(Some(Resp::ConfirmDownload { target, title, count: new, tracks: listed }));
            }
        }
        let skipped = listed.len() - todo.len();
        let mut q = self.batch.lock().unwrap();
        let (key, queued) = q.add(key, title, listed.len(), todo);
        log::info!("download {key}: {queued} songs queued, {skipped} downloaded already");
        for _ in 0..q.more_workers() {
            let this = self.clone();
            tokio::spawn(async move { this.download_worker().await });
        }
        let _ = self.tx.send(q.snapshot());
        drop(q);
        self.wake();
        Ok(None)
    }

    /// Download queued songs, one after another, until none are left.
    async fn download_worker(&self) {
        loop {
            let next = self.batch.lock().unwrap().next();
            let Some(t) = next else { return };
            // On a task of its own, so a panic in it fails this song, not the whole queue.
            let (this, song) = (self.clone(), t.clone());
            let result = tokio::spawn(async move { this.download_listed(&song).await }).await.unwrap_or_else(|e| Err(anyhow::anyhow!("{e}")));
            if let Err(e) = &result {
                log::warn!("download {}: {e:#}", t.id);
            }
            {
                let mut q = self.batch.lock().unwrap();
                q.finish(&t, &result);
                let _ = self.tx.send(q.snapshot());
            }
            self.wake();
        }
    }

    /// One song of a run, behind the playing song, unless it's downloaded by now. A music
    /// video saves its song version (with none found it isn't downloaded). True once saved.
    async fn download_listed(&self, t: &Track) -> anyhow::Result<bool> {
        let (song, for_video) = match t.music_video {
            true => (self.song_version(&t.id, Some(t), PREFETCH_SONG_WAIT).await.ok_or_else(|| anyhow::anyhow!("no song version of this music video"))?, Some(t.id.clone())),
            false => (t.clone(), None),
        };
        // Its song is downloaded already: the video plays that file too.
        if reuse_download(&song.id, for_video.clone()).await.is_some() {
            return Ok(false);
        }
        if !self.download(&song, for_video, true).await?.1 {
            // Saved meanwhile by another download, lyrics and all.
            return Ok(false);
        }
        // Its lyrics too, for offline: before the next song, so a long run doesn't pile them up.
        let found = self.lyrics_for(&song).await.sources.len();
        log::info!("download {}: lyrics from {found} source(s) kept", song.id);
        Ok(true)
    }

    /// Stop downloading the run `key`: its waiting songs are dropped (unless another run
    /// wants them too), and the ones downloading now finish.
    pub fn cancel_downloads(&self, key: &str) {
        {
            let mut q = self.batch.lock().unwrap();
            if !q.cancel(key) {
                return;
            }
            let _ = self.tx.send(q.snapshot());
        }
        log::info!("download {key}: cancelled");
        self.wake();
    }

    /// Find a stream URL, trying the sources in the order Settings › Music sources
    /// lists them: the first one that has the song plays its copy, and when none does
    /// (or the song was sent back to YouTube audio), YouTube plays. `best` (downloads)
    /// takes the high quality stream whatever the setting, and waits for the solver it
    /// needs, and downloads always come from YouTube. Playback takes Opus where there
    /// is one (Premium's 774 at ~256 kbps, else 251); downloads stay AAC, which saves
    /// as a tagged .m4a.
    /// `allow_hires`: the sources before YouTube are tried (playback only).
    async fn resolve(&self, video_id: &str, best: bool, track: Option<&Track>, allow_hires: bool, prefetch: bool, deferred_addons: &mut DeferredAddons) -> anyhow::Result<(Source, ResolvedStream)> {
        let youtube = best || self.source_on(MusicSource::Youtube);
        let reverted = self.reverted.lock().unwrap().contains(video_id);
        if allow_hires
            && !reverted
            && let Some(track) = track
        {
            // Snapshotted first: the read guard must not live across the awaits below.
            let sources = self.sources.read().unwrap().clone();
            let has_fallback = youtube || sources.iter().any(|&(kind, on)| on && !matches!(kind, MusicSource::Addons | MusicSource::Youtube));
            let lookup = if prefetch { hires::Lookup::Prefetch } else if has_fallback { hires::Lookup::Playback } else { hires::Lookup::Startup };
            for (kind, on) in sources {
                if !on || kind == MusicSource::Youtube {
                    continue;
                }
                let started = std::time::Instant::now();
                let found = match kind {
                    MusicSource::Addons => {
                        let sources = self.hires.read().unwrap().clone();
                        if sources.is_empty() {
                            continue;
                        }
                        let found = self.from_hires(&sources, track, lookup).await.map(|s| (Source::HiRes, s));
                        if found.is_none() && matches!(lookup, hires::Lookup::Playback | hires::Lookup::Prefetch) {
                            *deferred_addons = Some((sources, track.clone()));
                        }
                        found
                    }
                    MusicSource::MonoServers => tokio::time::timeout(HIRES_WAIT, self.from_mono(providers::Which::Added, track)).await.ok().flatten().map(|s| (Source::Provider, s)),
                    MusicSource::Monochrome => tokio::time::timeout(HIRES_WAIT, self.from_mono(providers::Which::Builtin, track)).await.ok().flatten().map(|s| (Source::Provider, s)),
                    MusicSource::JioSaavn => tokio::time::timeout(HIRES_WAIT, self.from_jiosaavn(track)).await.ok().flatten().map(|s| (Source::Provider, s)),
                    MusicSource::Youtube => continue,
                };
                if let Some((source, s)) = found {
                    match s.info.as_ref() {
                        Some(info) => log::info!("{video_id}: {} audio from {}", info.badge(), info.source),
                        None => log::info!("{video_id}: played from {} ({:.1}s)", kind.label(), started.elapsed().as_secs_f32()),
                    }
                    return Ok((source, s));
                }
                log::info!("{video_id}: {} had nothing ({:.1}s)", kind.label(), started.elapsed().as_secs_f32());
            }
        }
        // Downloads always come from YouTube. For playback, try it if enabled
        // before going back to any addons whose manifest renditions were deferred.
        let result = if youtube {
            self.resolve_youtube(video_id, best).await
        } else {
            Err(anyhow::anyhow!("YouTube Music is switched off in Settings \u{203a} Music sources, and no source before it has the song"))
        };
        if result.is_err()
            && !prefetch
            && let Some((sources, track)) = deferred_addons.take()
            && let Some(stream) = self.from_hires(&sources, &track, hires::Lookup::Startup).await
        {
            return Ok((Source::HiRes, stream));
        }
        result
    }

    async fn resolve_youtube(&self, video_id: &str, best: bool) -> anyhow::Result<(Source, ResolvedStream)> {
        let hq = best || self.hq.load(Ordering::Relaxed);
        if best
            && !self.jsc.ready()
            && let Some(session) = self.api.account()
            && self.is_premium(&session).await
            && let Err(e) = self.jsc.warm(&self.http()).await
        {
            log::warn!("solver warm-up failed: {e:#}");
        }
        // High quality needs the signed-in player's solver. Until it's prepared (a new
        // install, a new player version: ~5 s or more), play standard quality at once.
        // Without Premium it's never prepared, and standard quality is all there is.
        let solver_ready = self.jsc.ready();
        if hq && !solver_ready && self.api.account().is_some() && !self.known_not_premium() {
            log::info!("{video_id}: solver not ready yet, standard quality for now");
            self.warm_signed_in();
        }
        if hq
            && solver_ready
            && let Some(session) = self.api.account()
            && self.is_premium(&session).await
        {
            // Premium plays Opus (774, ~256 kbps) where there is one; downloads stay AAC,
            // which saves as a tagged .m4a.
            match self.resolve_from(Source::WebMusic, video_id, if best { stream::Format::Aac } else { stream::Format::Opus }).await {
                Ok(s) => return Ok((Source::WebMusic, s)),
                // Not fatal: the usual 128 kbps stream still plays.
                Err(e) => log::warn!("high quality stream failed for {video_id} ({e:#}); using standard quality"),
            }
        }
        let opus = if best { stream::Format::Aac } else { stream::Format::Opus };
        // Downloads stay on the file stream: they're saved as the .m4a it is.
        if !best && self.visionos_hls.load(Ordering::Relaxed) {
            match self.resolve_from(Source::VisionOsHls, video_id, stream::Format::Aac).await {
                Ok(s) => return Ok((Source::VisionOsHls, s)),
                // The signed-in paths below take it.
                Err(e) if needs_account(&e) => {}
                Err(e) => log::warn!("HLS stream failed for {video_id} ({e:#}); trying the file stream"),
            }
        }
        match self.resolve_from(Source::VisionOs, video_id, opus).await {
            Ok(s) => Ok((Source::VisionOs, s)),
            // Age-restricted or Premium-only songs need the signed-in players; any other
            // failure gets the same cascade, so one player's answer failing doesn't skip
            // a song another can still serve. (The addons were asked before this: their
            // miss fell through to here.)
            Err(e) => {
                let restricted = needs_account(&e);
                if restricted {
                    log::info!("{video_id} needs a signed-in player ({e:#})");
                } else {
                    log::info!("{video_id}: the public player failed ({e:#}); trying the signed-in players");
                }
                let Some(session) = self.api.account() else {
                    // Nothing signed in to fall back on: the public player's answer stands.
                    return Err(if restricted { anyhow::anyhow!("sign in to play this song") } else { e });
                };
                if self.is_premium(&session).await {
                    match self.resolve_from(Source::WebMusic, video_id, if best { stream::Format::Aac } else { stream::Format::Opus }).await {
                        Ok(s) => return Ok((Source::WebMusic, s)),
                        Err(e) => log::warn!("web music player failed ({e:#}); using yt-dlp"),
                    }
                }
                // An episode of a podcast added by RSS: YouTube doesn't have its audio,
                // and visionOS answers as for a video that's gone. The web player has
                // its file, signed in, Premium or not.
                if let Some(s) = self.rss_episode(&session, video_id).await? {
                    return Ok((Source::Feed, s));
                }
                // The bot check the backup stream is for: Safari's HLS plays it through.
                let blocked = bot_check(&e);
                if !best && self.backup_stream.load(Ordering::Relaxed) && blocked {
                    match self.resolve_from(Source::SafariHls, video_id, stream::Format::Aac).await {
                        Ok(s) => return Ok((Source::SafariHls, s)),
                        Err(e) => log::warn!("Safari HLS failed for {video_id} ({e:#}); trying the file stream"),
                    }
                }
                let source = if blocked { Source::YtDlpAfterBotCheck } else { Source::YtDlp };
                Ok((source, self.resolve_from(source, video_id, stream::Format::Aac).await?))
            }
        }
    }

    /// The feed's file when `video_id` is an episode of a podcast added by RSS, `None`
    /// for anything else. The web player's refusal is only logged: the caller has its own
    /// error to give.
    async fn rss_episode(&self, session: &webmusic::Session, video_id: &str) -> anyhow::Result<Option<ResolvedStream>> {
        match session.feed(&self.http(), &self.jsc, video_id).await {
            Ok(Some(feed)) => self.feed_stream(feed).await.map(Some),
            Ok(None) => Ok(None),
            Err(e) => {
                log::info!("{video_id}: web player: {e:#}");
                Ok(None)
            }
        }
    }

    /// `format`: the container the caller needs (the visionOS player picks Opus or AAC
    /// from it; Premium's web player takes Opus to mean its 774, ~256 kbps, where there
    /// is one). The hi-res source re-resolves by searching for `track` again.
    async fn resolve_from(&self, source: Source, video_id: &str, format: stream::Format) -> anyhow::Result<ResolvedStream> {
        let plain = |s: innertube::Stream, ua: &str| ResolvedStream {
            media: stream::Media::File(s.url),
            starts: None,
            size: s.size,
            duration_ms: s.duration_ms,
            user_agent: ua.to_owned(),
            loudness_db: s.loudness_db,
            format: stream::Format::from_opus(s.opus),
            info: None,
            ranging: stream::Ranging::Query,
            data: None,
            selection: None,
        };
        Ok(match source {
            Source::VisionOs => plain(self.visionos_stream(video_id, format == stream::Format::Opus).await?, innertube::UA),
            Source::VisionOsHls => {
                let s = self.visionos_hls(video_id).await?;
                let (starts, urls) = s.segments.into_iter().unzip();
                ResolvedStream { media: stream::Media::Segments(urls), starts: Some(starts), size: 0, duration_ms: s.duration_ms, user_agent: innertube::UA.to_owned(), loudness_db: s.loudness_db, format: stream::Format::Aac, info: None, ranging: stream::Ranging::Query, data: None, selection: None }
            }
            Source::WebMusic => {
                let session = self.api.account().ok_or_else(|| anyhow::anyhow!("signed out"))?;
                match session.audio_stream(&self.http(), &self.jsc, video_id, format == stream::Format::Opus).await? {
                    webmusic::Audio::YouTube(s) => plain(s, webmusic::UA),
                    webmusic::Audio::Feed(feed) => self.feed_stream(feed).await?,
                }
            }
            Source::Feed => {
                let session = self.api.account().ok_or_else(|| anyhow::anyhow!("signed out"))?;
                let feed = session.feed(&self.http(), &self.jsc, video_id).await?.ok_or_else(|| anyhow::anyhow!("not an RSS podcast episode"))?;
                self.feed_stream(feed).await?
            }
            Source::YtDlp | Source::YtDlpAfterBotCheck | Source::SafariHls => {
                let session = self.api.account().ok_or_else(|| anyhow::anyhow!("sign in to play this song"))?;
                let format = if source == Source::SafariHls { ytdlp::Format::SafariHls } else { ytdlp::Format::File };
                let s = ytdlp::resolve(&self.http(), &self.data_dir, &session.cookie, video_id, format).await?;
                // Cookies Google replaced while yt-dlp used the session.
                self.api.keep(&session, s.replaced.iter().map(String::as_str));
                if format == ytdlp::Format::SafariHls {
                    let (segments, duration_ms) = crate::hls::safari_playlist(&self.http(), &s.url, &s.user_agent).await?;
                    let (starts, urls) = segments.into_iter().unzip();
                    ResolvedStream { media: stream::Media::TransportStream(urls), starts: Some(starts), size: 0, duration_ms, user_agent: s.user_agent, loudness_db: None, format: stream::Format::Aac, info: None, ranging: stream::Ranging::Query, data: None, selection: None }
                } else {
                    // yt-dlp's JSON doesn't carry YouTube's loudness figure.
                    ResolvedStream { media: stream::Media::File(s.url), starts: None, size: s.size, duration_ms: s.duration_ms, user_agent: s.user_agent, loudness_db: None, format: stream::Format::Aac, info: None, ranging: stream::Ranging::Query, data: None, selection: None }
                }
            }
            Source::HiRes | Source::Provider => anyhow::bail!("no source selection to refresh"),
        })
    }

    /// An RSS podcast episode's file, fetched from where its redirects (trackers, mostly)
    /// end, so they aren't followed again for every range.
    async fn feed_stream(&self, feed: webmusic::Feed) -> anyhow::Result<ResolvedStream> {
        let (url, size) = stream::probe_feed(&self.http(), &feed.url, webmusic::UA).await?;
        log::info!("podcast feed file on {}", reqwest::Url::parse(&url).ok().as_ref().and_then(reqwest::Url::host_str).unwrap_or("?"));
        Ok(ResolvedStream {
            media: stream::Media::Feed(url),
            starts: None,
            size,
            duration_ms: feed.duration_ms,
            user_agent: webmusic::UA.to_owned(),
            loudness_db: None,
            format: stream::Format::File { hint: "mp3", mime: None },
            info: None,
            ranging: stream::Ranging::Header,
            data: None,
            selection: None,
        })
    }

    /// The listener's addons, searched together. Atmos leads when the preference is on,
    /// then hi-res/lossless; `None` when none of them has a copy (a miss, not a fault).
    async fn from_hires(&self, sources: &[Arc<hires::Client>], track: &Track, lookup: hires::Lookup) -> Option<ResolvedStream> {
        let enabled;
        let sources = if matches!(lookup, hires::Lookup::Startup) {
            if !self.source_on(MusicSource::Addons) || self.disable_direct.load(Ordering::Relaxed) {
                return None;
            }
            // Settings recreate clients when links change. Match the full link,
            // and use only clients from the current configuration.
            enabled = self.hires.read().unwrap().iter()
                .filter(|current| sources.iter().any(|captured| current.same_link(captured)))
                .cloned().collect::<Vec<_>>();
            enabled.as_slice()
        } else {
            sources
        };
        let atmos = self.atmos.load(Ordering::Relaxed);
        let http = self.http();
        let budget = if matches!(lookup, hires::Lookup::Upgrade { .. }) { UPGRADE_ADDON_WAIT } else { HIRES_WAIT };
        let found = hires::find(&http, sources, track, atmos, tokio::time::Instant::now() + budget, lookup).await;
        let Some(found) = found else {
            log::info!("hi-res: no copy of \"{}\" on {} source(s)", track.title, sources.len());
            return None;
        };
        Some(ResolvedStream {
            media: stream::Media::File(found.url),
            starts: None,
            size: found.size,
            duration_ms: found.duration_ms,
            user_agent: hires::agent().to_owned(),
            loudness_db: None,
            format: found.format,
            info: Some(found.info),
            ranging: stream::Ranging::Header,
            data: found.data,
            selection: Some(RefreshSelection::Addon(found.selection)),
        })
    }

    /// Monochrome's copy of `track` from the servers `which` names: the ones the
    /// listener added, or its own track streamer. `None`: it has none — a miss rather
    /// than a fault.
    async fn from_mono(&self, which: providers::Which, track: &Track) -> Option<ResolvedStream> {
        let http = self.http();
        self.monochrome.find_on(&http, which, &providers::Query::of(track)).await.ok().flatten().map(provider_stream)
    }

    /// JioSaavn's copy of `track`, `None` when it has none (a miss, not a fault).
    async fn from_jiosaavn(&self, track: &Track) -> Option<ResolvedStream> {
        let http = self.http();
        self.jiosaavn.find(&http, &providers::Query::of(track)).await.ok().flatten().map(provider_stream)
    }

    /// A song's music video: YouTube's own pair (Premium), or a sure signed-out search
    /// match, which doesn't know how the two line up.
    async fn music_video(&self, song: &Track) -> anyhow::Result<Option<ytm::MusicVideo>> {
        let premium = match self.api.account() {
            Some(session) => self.is_premium(&session).await,
            None => false,
        };
        if premium {
            match ytm::paired_video(&self.api, &song.id).await {
                // YouTube pairs every song that has a music video; trust it when it says none.
                Ok(found) => return Ok(found),
                Err(e) => log::debug!("video pair of {}: {e:#}; searching instead", song.id),
            }
        }
        let found = ytm::searched_video(&self.api, song).await?;
        match &found {
            Some(v) => log::info!("music video of {} \"{}\": {} \"{}\"", song.id, song.title, v.id, v.title),
            None => log::info!("music video of {} \"{}\": none found", song.id, song.title),
        }
        Ok(found.map(|video| ytm::MusicVideo { video, offset_ms: 0 }))
    }

    /// Get the signed-in playback path ready in the background, so the first Premium-only
    /// or age-restricted song doesn't wait for the Premium check or the solver setup that
    /// each new YouTube player version needs (~5 s, every few days).
    pub fn warm_signed_in(&self) {
        let this = self.clone();
        self.rt.spawn(async move {
            let Some(session) = this.api.account() else { return };
            if this.is_premium(&session).await
                && let Err(e) = this.jsc.warm(&this.http()).await
            {
                log::warn!("solver warm-up failed: {e:#}");
            }
        });
    }

    /// The Premium check has answered, and the account doesn't have it. Doesn't ask or
    /// wait: while a check is running, it's not known yet.
    fn known_not_premium(&self) -> bool {
        self.premium.try_lock().is_ok_and(|p| matches!(*p, Ok(false)))
    }

    /// Premium accounts can stream signed-in web player URLs without a PO token.
    async fn is_premium(&self, session: &webmusic::Session) -> bool {
        const PREMIUM_RETRY: std::time::Duration = std::time::Duration::from_secs(300);
        let mut premium = self.premium.lock().await;
        match *premium {
            Ok(p) => return p,
            Err(Some(at)) if at.elapsed() < PREMIUM_RETRY => return false,
            Err(_) => {}
        }
        match session.is_premium(&self.http()).await {
            Ok(p) => {
                log::info!("account has Premium: {p}");
                *premium = Ok(p);
                p
            }
            Err(e) => {
                log::warn!("Premium check failed: {e:#}");
                *premium = Err(Some(std::time::Instant::now()));
                false
            }
        }
    }

    async fn visionos_hls(&self, video_id: &str) -> anyhow::Result<innertube::HlsStream> {
        let vd = self.api.visitor_data(false).await?;
        match innertube::hls_audio(&self.http(), video_id, &vd).await {
            Ok(s) => Ok(s),
            Err(e) if needs_account(&e) => Err(e),
            Err(e) => {
                log::info!("visionOS player (HLS): {e:#}; retrying with new visitor data");
                let vd = self.api.visitor_data(true).await?;
                innertube::hls_audio(&self.http(), video_id, &vd).await
            }
        }
    }

    async fn visionos_stream(&self, video_id: &str, opus: bool) -> anyhow::Result<innertube::Stream> {
        let vd = self.api.visitor_data(false).await?;
        match innertube::audio_stream(&self.http(), video_id, &vd, opus).await {
            Ok(s) => Ok(s),
            Err(e) if needs_account(&e) => Err(e),
            Err(e) => {
                // A stale/flagged visitor ID is the usual cause; retry once with a fresh one.
                log::info!("visionOS player: {e:#}; retrying with new visitor data");
                let vd = self.api.visitor_data(true).await?;
                innertube::audio_stream(&self.http(), video_id, &vd, opus).await
            }
        }
    }

    /// Webview profile used by the sign-in window.
    pub fn login_profile(&self) -> PathBuf {
        self.data_dir.join("webview")
    }

    /// Report an error from outside the request flow (e.g. the sign-in window).
    pub fn report_error(&self, msg: String) {
        let _ = self.tx.send(Resp::Error(msg));
        self.wake();
    }

    pub fn data_dir(&self) -> &std::path::Path {
        &self.data_dir
    }

    pub fn set_high_quality(&self, on: bool) {
        self.hq.store(on, Ordering::Relaxed);
    }

    /// The backup stream on or off. Off: a prefetch already opened with it is dropped,
    /// so it doesn't hand a song a stream the setting now refuses.
    pub fn set_backup_stream(&self, on: bool) {
        let mut prefetched = self.prefetched.lock().unwrap();
        if self.backup_stream.swap(on, Ordering::Relaxed) != on {
            self.backup_stream_revision.fetch_add(1, Ordering::Relaxed);
            if let Some((_, (stream, _))) = prefetched.take() {
                stream.cancel();
            }
        }
    }

    /// Explicit songs hidden, and whether YouTube's Restricted Mode is asked for as well.
    pub fn set_hide_explicit(&self, on: bool, restricted: bool) {
        self.clean.store(on, Ordering::Relaxed);
        self.api.set_restricted(on && restricted);
    }

    /// Prefer a Dolby Atmos rendition from the hi-res sources when they offer one.
    pub fn set_prefer_atmos(&self, on: bool) {
        self.atmos.store(on, Ordering::Relaxed);
    }

    /// Send a song back to YouTube audio: later plays of it skip the hi-res sources.
    pub fn revert_quality(&self, video_id: &str) {
        let mut reverted = self.reverted.lock().unwrap();
        // One id per user action; a session that long has forgotten songs worth
        // re-asking the sources about anyway.
        if reverted.len() >= 512 {
            reverted.clear();
        }
        reverted.insert(video_id.to_owned());
    }

    /// Allow hi-res sources again for a song that was sent back to YouTube audio.
    pub fn upgrade_quality(&self, video_id: &str) {
        self.reverted.lock().unwrap().remove(video_id);
    }

    /// Play YouTube immediately and upgrade to hi-res in the background: the play path
    /// leaves the addons alone, and a copy that answers takes over in place.
    pub fn set_disable_direct_stream(&self, on: bool) {
        self.disable_direct.store(on, Ordering::Relaxed);
    }

    /// After playback starts, opportunistically look for a finer copy from the
    /// configured music sources (addons, Monochrome servers the listener added,
    /// Monochrome's built-in streamer, or JioSaavn) and upgrade the playing stream in
    /// place — whether playback started on YouTube or on a provider's own copy. Every
    /// source is asked at once and every answer is waited for, so a copy that answers
    /// late (a cold addon resolve) still counts; the swap happens only when the best
    /// answer sits strictly above the tier already playing, so a same-rung answer never
    /// restarts the download for nothing.
    async fn try_upgrade_hires(&self, video_id: String, generation: u64, track: Track, fresh: bool) {
        if self.audio.status.wanted_gen.load(Ordering::Relaxed) != generation {
            return;
        }
        if self.reverted.lock().unwrap().contains(&video_id) {
            return;
        }
        // Snapshot the configured sources together: the upgrade can come from any of
        // them. YouTube is the fallback the user is already playing, so it's skipped.
        let sources = self.sources.read().unwrap().clone();
        let has_upgrade_path = sources.iter().any(|(kind, on)| *on && *kind != MusicSource::Youtube);
        if !has_upgrade_path {
            return;
        }

        // YouTube has no Info (rung 0); Atmos (rung 3) has no possible upgrade.
        let playing_rung = self.current.lock().unwrap().as_ref().and_then(|(_, s)| s.info.as_ref()).map(hires::Info::rung).unwrap_or(0);
        if playing_rung == 3 {
            return;
        }

        let _ = self.tx.send(Resp::UpgradingQuality { generation, active: true });
        self.wake();
        struct UpgradeGuard<'a> {
            backend: &'a Backend,
            generation: u64,
        }
        impl<'a> Drop for UpgradeGuard<'a> {
            fn drop(&mut self) {
                let _ = self.backend.tx.send(Resp::UpgradingQuality { generation: self.generation, active: false });
                self.backend.wake();
            }
        }
        let _guard = UpgradeGuard { backend: self, generation };

        let started = std::time::Instant::now();
        // Every source is asked at once, the way `resolve` races them, and each keeps
        // its own wait (the addons' `UPGRADE_ADDON_WAIT`, the providers'
        // `UPGRADE_WAIT`): a slow addon must not keep a faster provider from
        // answering, and the song keeps playing from what's already running while
        // this waits. The first answer isn't taken as final — a slower source can hold
        // a finer copy (a hi-res addon resolving cold while a provider's MP3 answered
        // in a moment), so every answer is collected and the best rung wins.
        let mut asks = tokio::task::JoinSet::new();
        for (kind, on) in sources {
            if !on || kind == MusicSource::Youtube {
                continue;
            }
            let (this, track) = (self.clone(), track.clone());
            asks.spawn(async move {
                let found = match kind {
                    MusicSource::Addons => {
                        let clients = this.hires.read().unwrap().clone();
                        if clients.is_empty() {
                            return None;
                        }
                        this.from_hires(&clients, &track, hires::Lookup::Upgrade { fresh }).await.map(|s| (Source::HiRes, s))
                    }
                    MusicSource::MonoServers => tokio::time::timeout(UPGRADE_WAIT, this.from_mono(providers::Which::Added, &track)).await.ok().flatten().map(|s| (Source::Provider, s)),
                    MusicSource::Monochrome => tokio::time::timeout(UPGRADE_WAIT, this.from_mono(providers::Which::Builtin, &track)).await.ok().flatten().map(|s| (Source::Provider, s)),
                    MusicSource::JioSaavn => tokio::time::timeout(UPGRADE_WAIT, this.from_jiosaavn(&track)).await.ok().flatten().map(|s| (Source::Provider, s)),
                    MusicSource::Youtube => return None,
                };
                found.map(|s| (kind, s))
            });
        }
        let mut best: Option<(MusicSource, Source, ResolvedStream)> = None;
        while let Some(joined) = asks.join_next().await {
            let Some((kind, (source, s))) = joined.ok().flatten() else { continue };
            let Some(info) = s.info.as_ref() else { continue };
            log::info!("{video_id}: {} offers {} ({})", kind.label(), info.badge(), info.source);
            let better = best.as_ref().and_then(|(_, _, b)| b.info.as_ref()).map(hires::Info::rung).unwrap_or(0) < info.rung();
            if better || best.is_none() {
                best = Some((kind, source, s));
            }
        }
        let (source, stream_res) = match best {
            Some((_, source, s)) => (source, s),
            None => {
                log::info!("{video_id}: no hi-res source or provider has a copy ({:.1}s)", started.elapsed().as_secs_f32());
                return;
            }
        };

        // A copy no finer than the one playing would restart the download and stutter
        // the song for nothing the ear can hear: the swap is only for a strictly higher
        // rung of the ladder.
        if stream_res.info.as_ref().map(hires::Info::rung).unwrap_or(0) <= playing_rung {
            log::info!("{video_id}: the best late copy is no finer than what's playing; keeping it ({:.1}s)", started.elapsed().as_secs_f32());
            return;
        }

        if self.audio.status.wanted_gen.load(Ordering::Relaxed) != generation {
            log::debug!("hi-res upgrade for {video_id} dropped: generation changed");
            return;
        }
        if self.reverted.lock().unwrap().contains(&video_id) {
            log::debug!("hi-res upgrade for {video_id} dropped: reverted to YouTube");
            return;
        }

        if let Some(info) = stream_res.info.as_ref() {
            log::info!("{video_id}: upgrading to {} audio from {}", info.badge(), info.source);
        } else {
            log::info!("{video_id}: upgrading from {:?}", source);
        }

        let (source, s) = (source, stream_res);
        let shared = if let Some(data) = s.data {
            Shared::from_bytes_with_format(data, s.duration_ms, s.loudness_db, s.format.clone(), s.info.clone())
        } else {
            let shared = Shared::new(s.size, s.duration_ms, s.loudness_db, s.format.clone(), s.info.clone(), false);
            self.rt.spawn(stream::download(
                self.http(),
                s.media,
                s.user_agent,
                shared.clone(),
                Some(self.refresher(source, &video_id, s.format.clone(), s.size, s.selection)),
                s.ranging,
            ));
            shared
        };

        let pos = self.audio.status.position();
        let sampled = std::time::Instant::now();
        let is_playing = self.audio.status.playing.load(Ordering::Relaxed);
        let shared_decode = shared.clone();
        let res = tokio::task::spawn_blocking(move || {
            let seekable = !pos.is_zero();
            let mut d = stream::decoder(shared_decode.clone(), seekable)?;
            let actual_pos = if is_playing { pos + sampled.elapsed() } else { pos };
            if seekable {
                rodio::Source::try_seek(&mut d, actual_pos).map_err(|e| anyhow::anyhow!("seek: {e}"))?;
            }
            Ok::<_, anyhow::Error>((d, actual_pos, seekable))
        })
        .await;

        let (decoder, actual_pos, seekable) = match res {
            Ok(Ok(tuple)) => tuple,
            Ok(Err(e)) => {
                shared.cancel();
                log::warn!("failed to decode hi-res stream for {video_id} ({e:#}); continuing with standard quality");
                return;
            }
            Err(e) => {
                shared.cancel();
                log::warn!("hi-res decoder task failed for {video_id} ({e:#}); continuing with standard quality");
                return;
            }
        };

        // Keep the rung check, state update and audio command ordered together:
        // another attempt may have installed a finer copy while this one decoded.
        let mut current = self.current.lock().unwrap();
        let reverted = self.reverted.lock().unwrap();
        let still_finer = current.as_ref().is_some_and(|(id, playing)| {
            *id == video_id && shared.info.as_ref().map_or(0, hires::Info::rung) > playing.info.as_ref().map_or(0, hires::Info::rung)
        });
        if !still_finer || self.audio.status.wanted_gen.load(Ordering::Relaxed) != generation || reverted.contains(&video_id) {
            shared.cancel();
            return;
        }

        *current = Some((video_id, shared.clone()));
        if let Some(mut info) = shared.info.clone() {
            info.sample_rate = Some(rodio::Source::sample_rate(&decoder).get());
            info.channels = Some(rodio::Source::channels(&decoder).get() as u8);
            let _ = self.tx.send(Resp::AudioInfo { generation, info });
        }
        let _ = self.tx.send(Resp::StreamStats { generation, stats: stream::Stats::of(&shared, &decoder) });
        self.audio.send(AudioCmd::Upgrade {
            generation,
            decoder,
            shared,
            at: actual_pos,
            seekable,
        });
        drop(reverted);
        drop(current);
        self.wake();
    }

    /// Upgrade the active song if it isn't reverted: the swap is worth it only when
    /// some source answers with a strictly finer tier than the one playing, which
    /// `try_upgrade_hires` decides.
    pub fn try_upgrade_active(&self, generation: u64, track: Option<Track>) {
        let video_id = {
            let current = self.current.lock().unwrap();
            let Some((id, _)) = current.as_ref() else { return };
            id.clone()
        };
        let Some(track) = track else { return };
        let this = self.clone();
        self.rt.spawn(async move {
            // The listener asked by hand, so the addons' cached stream answers are
            // skipped: a retry must be able to recover from an addon that answered
            // while one of its backends was unavailable (the Android app's
            // `clearCompletedTrackCalls` for its manual upgrade).
            this.try_upgrade_hires(video_id, generation, track, true).await;
        });
    }

    /// Turn JioSaavn's stream quality to what its Settings row picked; its switch
    /// lives in the sources list (`set_sources`).
    pub fn set_saavn_quality(&self, quality: providers::SaavnQuality) {
        self.jiosaavn.set_quality(quality);
    }

    /// Point Monochrome at the servers saved as `url|kind|name` lines (empty: just the
    /// built-in streamer); its switch lives in the sources list.
    pub fn set_monochrome_servers(&self, servers: &str) {
        self.monochrome.set_servers(servers);
    }

    /// The sources before YouTube, in the order their Settings rows list them, each with
    /// its switch. Replaces whatever was set before.
    pub fn set_sources(&self, sources: &[(MusicSource, bool)]) {
        let known = |k: &MusicSource| sources.iter().any(|(kind, _)| kind == k);
        // Every source gets a row, even if the list came in short of one.
        let mut cfg: Vec<(MusicSource, bool)> = sources.to_vec();
        for (kind, on) in DEFAULT_SOURCES {
            if !known(&kind) {
                cfg.push((kind, on));
            }
        }
        cfg.retain(|(kind, _)| MusicSource::ALL.contains(kind));
        let set: Vec<String> = cfg.iter().map(|(k, on)| format!("{}={}", k.label(), if *on { "on" } else { "off" })).collect();
        log::info!("music sources: {}", set.join(", "));
        *self.sources.write().unwrap() = cfg;
    }

    /// Whether a source's row is on, as the last `set_sources` set it. YouTube counts as
    /// on while nothing has been set (a fresh backend, before its Settings arrive).
    fn source_on(&self, kind: MusicSource) -> bool {
        let cfg = self.sources.read().unwrap();
        if cfg.is_empty() {
            return kind == MusicSource::Youtube || matches!(kind, MusicSource::Addons);
        }
        cfg.iter().find(|(k, _)| *k == kind).is_some_and(|(_, on)| *on)
    }

    /// Point the hi-res sources at the links in `links`, one per line (empty: none). The
    /// links themselves aren't logged: they can carry a token.
    pub fn set_hires_source(&self, links: &str) {
        let mut clients: Vec<Arc<hires::Client>> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut rejected = 0;
        for line in links.lines() {
            let line = line.trim();
            // One client per distinct link. An exact repeat is skipped (it would fire
            // duplicate searches and streams at one server, which is how a working
            // source starts answering 502); a different link on the same host is a
            // separate source the user asked for and is kept.
            if line.is_empty() || !seen.insert(line.to_owned()) {
                continue;
            }
            match hires::Client::new(line) {
                Some(client) => clients.push(Arc::new(client)),
                None => rejected += 1,
            }
        }
        let hosts: Vec<String> = clients.iter().map(|c| c.host()).collect();
        match clients.len() {
            0 if rejected == 0 => log::info!("hi-res sources: none"),
            0 => log::warn!("hi-res sources: none usable ({rejected} not an http(s) link)"),
            1 => log::info!("hi-res source set: {}", hosts[0]),
            n => log::info!("{n} hi-res sources set: {}", hosts.join(", ")),
        }
        if rejected > 0 && !clients.is_empty() {
            log::warn!("hi-res sources: {rejected} entry(s) aren't http(s) links and were skipped");
        }
        *self.hires.write().unwrap() = clients;
    }

    /// Like or unlike a selection, one at a time, then check Liked Music and try again
    /// what didn't take. Unlikes still listed after that count as refused; likes aren't
    /// counted so (songs that can't be played are liked but not listed).
    async fn like_many(&self, ids: Vec<String>, like: bool) -> anyhow::Result<Resp> {
        let total = ids.len();
        let (done, failed) = like_many(&self.api, ids, like, |_, n| {
            let _ = self.tx.send(Resp::LikeProgress { done: n, total, like });
            self.wake();
        })
        .await;
        // Even when all were refused: the app stops waiting on them and shows them as they are.
        Ok(Resp::LikedMany { video_ids: done, failed, like })
    }

    /// Send a playlist's first page right away, then each next page as it arrives
    /// (YouTube takes about half a second per page of 100). The last one is returned.
    async fn stream_playlist(&self, id: String, load: u64, me: bool) -> anyhow::Result<Resp> {
        let (c, mut token, limit) = ytm::playlist_start(&self.api, &id, me, None).await?;
        let mut have = c.tracks.len();
        let mut resp = Resp::Collection { load, c, more: token.is_some() && have < limit };
        while let Some(t) = token.take().filter(|_| have < limit) {
            let _ = self.tx.send(resp);
            self.wake();
            // Failing here, the page keeps what came.
            let (mut tracks, mut set_ids, next) = ytm::playlist_more(&self.api, &t, me).await?;
            tracks.truncate(limit - have);
            set_ids.truncate(limit - have);
            have += tracks.len();
            token = next.filter(|_| !tracks.is_empty());
            resp = Resp::CollectionMore { id: id.clone(), load, more: token.is_some() && have < limit, tracks, set_ids };
        }
        Ok(resp)
    }

    pub fn is_logged_in(&self) -> bool {
        self.api.account().is_some()
    }

    /// Which signed-in Google account the listening store belongs to — the one-way
    /// key of the session's SAPISID cookie, `None` signed out: what was listened to
    /// is kept per account, and signed out has its own store (see `listening.rs`).
    pub fn listening_scope(&self) -> Option<String> {
        self.api.account().and_then(|a| a.scope_key())
    }
}

/// Test instances (`YTM_BACKGROUND`) keep plays out of the account's history unless
/// `YTM_REPORT_PLAYS` is set.
fn reports_plays() -> bool {
    std::env::var_os("YTM_BACKGROUND").is_none() || std::env::var_os("YTM_REPORT_PLAYS").is_some()
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Source {
    VisionOs,
    /// The visionOS player's HLS segments: where its file URLs are cut off at 1 MiB.
    VisionOsHls,
    WebMusic,
    /// An RSS podcast episode's file, which the web player finds without Premium.
    Feed,
    YtDlp,
    /// The file stream behind a bot-check refusal: its 403s are the backup stream's cue.
    YtDlpAfterBotCheck,
    /// Safari's HLS, the backup stream: AAC in MPEG-TS video segments.
    SafariHls,
    /// The listener's own addon server (see `hires`).
    HiRes,
    /// A music provider (see `providers`): Monochrome or JioSaavn.
    Provider,
}

async fn playback_decoder(shared: Arc<Shared>, at: std::time::Duration) -> anyhow::Result<crate::audio::TrackDecoder> {
    // Reading the container header blocks on the download. Mid-song starts
    // also need the seekable decoder, which waits for every fragment header.
    tokio::task::spawn_blocking(move || {
        let seek = !at.is_zero();
        let mut decoder = stream::decoder(shared, seek)?;
        if seek {
            rodio::Source::try_seek(&mut decoder, at).map_err(|e| anyhow::anyhow!("seek: {e}"))?;
        }
        Ok(decoder)
    }).await?
}

enum Found {
    Saved(Arc<Shared>),
    Url(Source, ResolvedStream),
}

/// The music sources a song can play from before YouTube, as Settings › Music sources
/// orders them. The first one that has the song plays its own copy; when none does,
/// YouTube plays as usual.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MusicSource {
    /// The listener's Bitchord addon servers, shared as links (see `hires`).
    Addons,
    /// Monochrome-format servers the listener added (see `providers::Monochrome`).
    MonoServers,
    /// Monochrome's own track streamer, which needs no setup.
    Monochrome,
    /// JioSaavn's catalog, AAC up to 320 kbps.
    JioSaavn,
    /// YouTube Music itself: the fallback everything ends at.
    Youtube,
}

impl MusicSource {
    pub const ALL: [MusicSource; 5] = [Self::Addons, Self::MonoServers, Self::Monochrome, Self::JioSaavn, Self::Youtube];

    /// The row's saved key.
    pub fn key(self) -> &'static str {
        match self {
            Self::Addons => "addons",
            Self::MonoServers => "mono_servers",
            Self::Monochrome => "mono",
            Self::JioSaavn => "jiosaavn",
            Self::Youtube => "youtube",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.key() == key)
    }

    /// The row's name, as Settings shows it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Addons => "Bitchord addons",
            Self::MonoServers => "Monochrome servers",
            Self::Monochrome => "Monochrome",
            Self::JioSaavn => "JioSaavn",
            Self::Youtube => "YouTube Music",
        }
    }
}

/// The order and switches a fresh install starts with: the addons on, Monochrome and
/// JioSaavn off until they're wanted, YouTube under everything.
pub const DEFAULT_SOURCES: [(MusicSource, bool); 5] =
    [(MusicSource::Addons, true), (MusicSource::MonoServers, false), (MusicSource::Monochrome, false), (MusicSource::JioSaavn, false), (MusicSource::Youtube, true)];

/// A provider's answer, as the playback pipeline takes it: their CDN takes byte ranges
/// by header, and the quality the player shows comes from the `Info` they build.
fn provider_stream(found: providers::Found) -> ResolvedStream {
    ResolvedStream {
        media: stream::Media::File(found.url),
        starts: None,
        size: found.size,
        duration_ms: found.duration_ms,
        user_agent: found.user_agent.to_owned(),
        loudness_db: None,
        format: found.format,
        info: Some(found.info),
        ranging: stream::Ranging::Header,
        data: None,
        selection: Some(RefreshSelection::Provider(found.selection)),
    }
}

#[derive(Clone)]
enum RefreshSelection {
    Provider(providers::Selection),
    Addon(hires::Selection),
}

struct ResolvedStream {
    media: stream::Media,
    /// HLS: each segment's start (s).
    starts: Option<Vec<f64>>,
    /// Bytes (a file; 0 for segments).
    size: u64,
    duration_ms: u64,
    user_agent: String,
    loudness_db: Option<f32>,
    format: stream::Format,
    /// What the player can say about a hi-res stream (codec, rate, depth, size).
    info: Option<hires::Info>,
    /// How this source takes byte ranges.
    ranging: stream::Ranging,
    /// Already-decoded audio (Dolby Atmos out of DASH): no download to run.
    data: Option<Vec<u8>>,
    selection: Option<RefreshSelection>,
}

impl ResolvedStream {
    /// A buffer laid out for this stream. `head_only`: a prefetch's.
    fn buffer(&self, head_only: bool) -> Arc<Shared> {
        match (&self.starts, &self.media) {
            (Some(starts), _) => Shared::segmented(starts.clone(), self.duration_ms, self.loudness_db, head_only),
            (None, stream::Media::Feed(_)) => Shared::feed(self.size, self.duration_ms, head_only),
            (None, _) => Shared::new(self.size, self.duration_ms, self.loudness_db, self.format.clone(), self.info.clone(), head_only),
        }
    }

    /// For the log.
    fn kind(&self) -> &'static str {
        match (&self.media, &self.format) {
            (stream::Media::Feed(_), _) => "podcast feed file",
            (_, stream::Format::Opus) => "Opus",
            _ => "AAC",
        }
    }
}

/// The anonymous visionOS player refused because the song needs an account:
/// age-restricted (LOGIN_REQUIRED) or Music Premium only.
fn needs_account(e: &anyhow::Error) -> bool {
    let msg = e.to_string();
    msg.contains("LOGIN_REQUIRED") || msg.contains("Premium")
}

/// Whether YouTube's answer is the bot check the backup stream is for.
fn bot_check(e: &anyhow::Error) -> bool {
    e.downcast_ref::<innertube::Refused>().is_some_and(|r| {
        r.status == "LOGIN_REQUIRED" && r.reason.to_ascii_lowercase().contains("not a bot")
    })
}

/// Whether a refreshed stream's segments fit the buffer. The download checks their
/// count. A Safari playlist can be cut differently each time, and each block holds
/// one segment's audio, so its start times must match too.
fn same_segments(source: Source, old: Option<&[f64]>, new: Option<&[f64]>) -> bool {
    source != Source::SafariHls || old == new
}

/// The anonymous visionOS player says the video isn't there ("ERROR: This video is not
/// available"), which is also its answer for an episode of a podcast added by RSS.
/// Nothing in that answer tells the two apart.
fn not_available(e: &anyhow::Error) -> bool {
    e.downcast_ref::<innertube::Refused>().is_some_and(|r| matches!(r.status.as_str(), "ERROR" | "UNPLAYABLE"))
}

/// Keep the signed-in release feed's order, using US charts and the user's home artists.
fn popular_new_albums(albums: &[Card], ranked_artists: &[String], preferred_artists: &[String]) -> Vec<Card> {
    let chart_ids: std::collections::HashSet<&str> = ranked_artists.iter().chain(preferred_artists).map(String::as_str).collect();
    let mut seen = std::collections::HashSet::new();
    albums
        .iter()
        .filter(|album| album.links.iter().any(|a| a.id.as_deref().is_some_and(|id| chart_ids.contains(id))))
        .filter(|album| seen.insert(match &album.target {
            Target::Album(id) => id.as_str(),
            _ => album.title.as_str(),
        }))
        .take(16)
        .cloned()
        .collect()
}

/// iTunes' main genre for an artist, the way Mochy's `ListeningGenres` finds it:
/// the same name first, otherwise the top hit. YouTube never says what genre
/// anything is; Apple does.
async fn itunes_genre(http: &reqwest::Client, name: &str) -> Option<String> {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("term", name.trim())
        .append_pair("media", "music")
        .append_pair("entity", "musicArtist")
        .append_pair("limit", "5")
        .finish();
    let body: serde_json::Value = http
        .get(format!("https://itunes.apple.com/search?{query}"))
        .header("User-Agent", "Mozilla/5.0")
        .timeout(std::time::Duration::from_secs(8))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    let results = body["results"].as_array()?;
    if results.is_empty() {
        return None;
    }
    let found = results
        .iter()
        .find(|r| r["artistName"].as_str().is_some_and(|a| a.trim().eq_ignore_ascii_case(name.trim())))
        .unwrap_or(&results[0]);
    found["primaryGenreName"].as_str().map(str::trim).filter(|g| !g.is_empty()).map(str::to_owned)
}

/// A release card's year, as its subtitle spells it out ("Album • Artist • 2024").
fn card_year(c: &Card) -> Option<u64> {
    c.subtitle.split([' ', '•', ',']).find_map(|t| t.trim().parse::<u64>().ok().filter(|y| (1950..=2100).contains(y)))
}

/// Whether `author` names one of the seed artists.
fn authors_matches(artists: &[(String, String)], author: &str) -> bool {
    artists.iter().any(|(_, name)| name.eq_ignore_ascii_case(author))
}

/// Lists dealt out one item at a time — an artist at a time — up to `limit`, with
/// no doubles. Mochy's `ArtistVideos.interleave`.
fn interleave<I: Iterator<Item = Card>>(lists: Vec<I>, limit: usize) -> Vec<Card> {
    let mut rows: Vec<std::collections::VecDeque<Card>> = lists.into_iter().map(|l| l.collect()).collect();
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    while out.len() < limit && rows.iter().any(|r| !r.is_empty()) {
        for row in &mut rows {
            if out.len() >= limit {
                break;
            }
            if let Some(c) = row.pop_front() {
                let id = match &c.target {
                    Target::Album(i) | Target::Playlist(i) | Target::Artist(i) => Some(i.clone()),
                    Target::Song(t) => Some(t.id.clone()),
                };
                if id.map_or(true, |id| seen.insert(id)) {
                    out.push(c);
                }
            }
        }
    }
    out
}

/// "Episodes for later" is listed even when empty; hide it then.
fn saved_playlist_visible(card: &Card) -> bool {
    card.target != Target::Playlist("SE".into()) || card.subtitle.split_whitespace().any(|w| w.parse::<u64>().is_ok_and(|n| n > 0))
}

pub fn compact(n: u64) -> String {
    let (v, unit) = match n {
        0..1_000 => return n.to_string(),
        1_000..1_000_000 => (n as f64 / 1e3, "K"),
        1_000_000..1_000_000_000 => (n as f64 / 1e6, "M"),
        _ => (n as f64 / 1e9, "B"),
    };
    // "6M", not "6.0M"; three significant figures and up need no decimal.
    let s = if v >= 100.0 { format!("{v:.0}") } else { format!("{v:.1}") };
    format!("{}{unit}", s.strip_suffix(".0").unwrap_or(&s))
}

// Larger art URL for the background / now-playing view. Only what can be fetched:
// Discord and the OS's own fetchers take a URL, and a local song's cover isn't one.
pub fn art_url(t: &Track) -> Option<String> {
    let u = model::pick(&t.thumbs, 120)?;
    (u.starts_with("https://") || u.starts_with("http://")).then(|| sized(u, 544))
}

/// The art for the background and the accent, wherever it comes from: a fetch, or a
/// local song's cover file (see `local.rs`).
pub fn art_uri(t: &Track) -> Option<String> {
    model::pick(&t.thumbs, 120).map(|u| sized(u, 544))
}

#[cfg(test)]
mod explore_tests {
    use super::*;

    fn card(title: &str, target: Target, artist_id: Option<&str>, subtitle: &str) -> Card {
        Card {
            title: title.into(),
            subtitle: subtitle.into(),
            thumbs: vec![],
            target,
            round: false,
            links: artist_id.map(|id| model::Link { name: "Artist".into(), id: Some(id.into()) }).into_iter().collect(),
        }
    }

    #[test]
    fn new_releases_only_include_charting_artists_and_deduplicate() {
        let album = |id: &str, artist_id: &str| card(id, Target::Album(id.into()), Some(artist_id), "Album");
        let albums = vec![album("obscure", "other"), album("favorite-release", "favorite"), album("first-release", "popular"), album("second-release", "popular"), album("first-release", "popular")];
        let found = popular_new_albums(&albums, &["popular".into(), "popular".into()], &["favorite".into()]);
        assert_eq!(found.iter().map(|c| c.title.as_str()).collect::<Vec<_>>(), ["favorite-release", "first-release", "second-release"]);
    }

    #[test]
    fn empty_episodes_for_later_is_hidden() {
        let item = |id: &str, subtitle: &str| card("Episodes for later", Target::Playlist(id.into()), None, subtitle);
        assert!(!saved_playlist_visible(&item("SE", "Auto playlist")));
        assert!(!saved_playlist_visible(&item("SE", "Auto playlist • 0 episodes")));
        assert!(saved_playlist_visible(&item("SE", "Auto playlist • 2 episodes")));
        assert!(saved_playlist_visible(&item("PLcustom", "Playlist")));
    }
}

#[cfg(test)]
mod resolve_tests {
    use super::*;

    fn refused(status: &str, reason: &str) -> anyhow::Error {
        innertube::Refused { status: status.into(), reason: reason.into() }.into()
    }

    #[test]
    fn the_bot_check_and_the_backup_streams_terms() {
        assert!(!bot_check(&refused("LOGIN_REQUIRED", "Sign in to confirm your age")));
        assert!(bot_check(&refused("LOGIN_REQUIRED", "Sign in to confirm you're not a bot")));
        let (old, moved) = ([0.0, 7.0, 14.0], [0.0, 6.9, 14.0]);
        // visionOS HLS: the download only checks the count, as before.
        assert!(same_segments(Source::VisionOsHls, Some(&old), Some(&moved)));
        assert!(same_segments(Source::VisionOs, None, None));
        assert!(same_segments(Source::SafariHls, Some(&old), Some(&old)));
        assert!(!same_segments(Source::SafariHls, Some(&old), Some(&moved)));
    }

    #[test]
    fn a_video_visionos_says_isnt_there_may_be_an_rss_episode() {
        // What visionOS answers for an episode of a podcast added by RSS.
        assert!(not_available(&refused("ERROR", "This video is not available")));
        assert!(not_available(&refused("UNPLAYABLE", "This video is unavailable")));
        assert!(not_available(&refused("ERROR", "x").context("visionOS player")));
        // Those that need an account keep their own route.
        assert!(!not_available(&refused("LOGIN_REQUIRED", "Sign in to confirm your age")));
        assert!(needs_account(&refused("LOGIN_REQUIRED", "Sign in to confirm your age")));
        assert!(!needs_account(&refused("ERROR", "This video is not available")));
        // Only the player's own answer, not an error that reads like one.
        assert!(!not_available(&anyhow::anyhow!("ERROR: This video is not available")));
    }

    /// Read-only, signed in to an account with a podcast added by RSS (NPR's Planet
    /// Money): visionOS says the episode isn't available, and the web player gives its
    /// feed's file, which is reachable.
    /// `YTM_DATA_DIR=<profile> cargo test --release -- --ignored rss_episode_live --nocapture`
    #[tokio::test]
    #[ignore]
    async fn rss_episode_live() {
        let dir = PathBuf::from(std::env::var("YTM_DATA_DIR").unwrap());
        let api = ytmapi::Api::new(&dir);
        let session = api.account().expect("no session in the vault");
        let (http, solver) = (reqwest::Client::new(), jsc::Solver::new(&dir));
        let id = std::env::var("YTM_RSS_EPISODE").unwrap_or_else(|_| "M77cW_SUPm4".into());
        let vd = api.visitor_data(false).await.unwrap();
        let Err(e) = innertube::audio_stream(&http, &id, &vd, false).await else { panic!("visionOS played {id}") };
        println!("visionOS: {e:#}");
        assert!(not_available(&e), "{e:#}");
        let t = std::time::Instant::now();
        let feed = session.feed(&http, &solver, &id).await.unwrap().expect("not an RSS episode");
        println!("web player: {} ms, {} (in {:?})", feed.duration_ms, feed.url, t.elapsed());
        assert!(feed.duration_ms > 0);
        let (file, size) = stream::probe_feed(&http, &feed.url, webmusic::UA).await.unwrap();
        println!("  -> {file}: {size} bytes");
        assert!(size > 0);
    }
}

#[cfg(test)]
mod download_queue_tests {
    use super::*;

    fn song(id: &str) -> Track {
        Track { id: id.into(), title: id.to_uppercase(), ..Default::default() }
    }

    fn songs(ids: &[&str]) -> Vec<Track> {
        ids.iter().map(|id| song(id)).collect()
    }

    fn run<'a>(q: &'a DownloadQueue, key: &str) -> &'a DownloadRun {
        q.runs.iter().find(|r| r.key == key).unwrap()
    }

    fn waiting(q: &DownloadQueue) -> Vec<&str> {
        q.waiting.iter().map(|t| t.id.as_str()).collect()
    }

    #[test]
    fn a_song_two_runs_want_downloads_once_for_both() {
        let mut q = DownloadQueue::default();
        assert_eq!(q.add(Some("A".into()), "A".into(), 3, songs(&["1", "2"])), ("A".to_owned(), 2));
        assert_eq!(q.add(Some("B".into()), "B".into(), 2, songs(&["2", "3"])), ("B".to_owned(), 2));
        assert_eq!(waiting(&q), ["1", "2", "3"]);
        let (a, b) = (run(&q, "A"), run(&q, "B"));
        assert_eq!((a.total, a.done, a.skipped), (3, 1, 1));
        assert_eq!((b.total, b.done), (2, 0));

        let two = q.waiting[1].clone();
        q.finish(&two, &Ok::<_, ()>(true));
        assert_eq!((run(&q, "A").done, run(&q, "A").saved), (2, 1));
        assert_eq!((run(&q, "B").done, run(&q, "B").saved), (1, 1));
    }

    #[test]
    fn queued_again_a_run_takes_on_only_what_it_lacks() {
        let mut q = DownloadQueue::default();
        q.add(Some("A".into()), "A".into(), 2, songs(&["1", "2"]));
        assert_eq!(q.add(Some("A".into()), "A".into(), 3, songs(&["1", "2", "3"])).1, 1);
        assert_eq!(waiting(&q), ["1", "2", "3"]);
        assert_eq!(run(&q, "A").total, 3);
    }

    #[test]
    fn queued_again_a_failed_song_is_retried_not_counted_twice() {
        let mut q = DownloadQueue::default();
        q.add(Some("A".into()), "A".into(), 3, songs(&["1", "2", "3"]));
        let one = q.next().unwrap();
        q.finish(&one, &Err::<bool, _>(()));
        assert_eq!((run(&q, "A").done, run(&q, "A").failed.clone()), (1, vec!["1".to_owned()]));

        // Queued again from the card's menu while 2 and 3 are still to go: 1 failed, so
        // it isn't downloaded and comes back; 2 and 3 are on their way already.
        assert_eq!(q.add(Some("A".into()), "A".into(), 3, songs(&["1", "2", "3"])).1, 1);
        let a = run(&q, "A");
        assert_eq!((a.total, a.done), (3, 0));
        assert!(a.failed.is_empty() && a.failed_ids.is_empty());
        assert_eq!(waiting(&q), ["2", "3", "1"]);

        // It fails again: listed once.
        for _ in 0..3 {
            let t = q.next().unwrap();
            let failed = t.id == "1";
            q.finish(&t, &if failed { Err(()) } else { Ok(true) });
        }
        let a = run(&q, "A");
        assert_eq!((a.total, a.done, a.saved), (3, 3, 2));
        assert_eq!(a.failed, ["1"]);
        assert!(a.finished());
    }

    #[test]
    fn cancelling_drops_only_the_songs_no_other_run_wants() {
        let mut q = DownloadQueue::default();
        q.add(Some("A".into()), "A".into(), 3, songs(&["1", "2", "3"]));
        q.add(Some("B".into()), "B".into(), 1, songs(&["3"]));
        // 1 is on its way: it finishes.
        let one = q.next().unwrap();
        assert!(q.cancel("A"));
        assert!(!q.cancel("nothing"));
        assert_eq!(waiting(&q), ["3"]);
        assert_eq!(q.wanted["3"], ["B"]);
        let a = run(&q, "A");
        assert_eq!((a.total, a.done, a.cancelled), (1, 0, true));
        q.finish(&one, &Ok::<_, ()>(true));
        assert!(run(&q, "A").finished());

        // A finished run is listed once more, then let go.
        let Resp::Downloads(runs) = q.snapshot() else { panic!() };
        assert_eq!(runs.len(), 2);
        assert_eq!(q.runs.iter().map(|r| r.key.as_str()).collect::<Vec<_>>(), ["B"]);
    }

    #[test]
    fn selections_get_keys_of_their_own_and_workers_are_capped() {
        let mut q = DownloadQueue::default();
        assert_eq!(q.add(None, String::new(), 1, songs(&["1"])).0, "selection-1");
        assert_eq!(q.add(None, String::new(), 2, songs(&["2", "3"])).0, "selection-2");
        assert_eq!(q.more_workers(), DOWNLOADS_AT_ONCE.min(3));
        assert_eq!(q.more_workers(), 0);
        while q.next().is_some() {}
        assert_eq!(q.workers, DOWNLOADS_AT_ONCE.min(3) - 1);
    }
}
