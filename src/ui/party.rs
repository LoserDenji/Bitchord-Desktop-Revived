//! Listen Together: the party-server half of the "Listen together" page, and the
//! binding between the party and the player — a port of ForkChord's
//! `partySync.ts` + `ListenTogetherPage.tsx`.
//!
//! The two directions come through different doors, as they do there:
//! - outbound, `publish`, runs when something in the player changed (an intent, or
//!   the queue moving on by itself) and sends the new truth to the server;
//! - inbound, `reconcile`, runs every 700 ms and writes the party's playhead
//!   through the same player calls a listener's own presses use.
//!
//! Following in time: a resume waits for the server's scheduled start, a new
//! control is landed on exactly, and drift between controls is corrected only
//! when it's real (twice in a row past 1.2 s, then left alone for 6 s).

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{Align2, RichText, Sense, Ui, pos2, vec2};

use super::pages::{divider, section_label, setting_row, settings_card, toggle};
use super::widgets::{icon_at, pill};
use super::{Action, App, Page};
use crate::audio::AudioCmd;
use crate::model::Track;
use crate::party::{self, Party, PartyTrack};
use crate::theme::{self, icon};

/// How often the player is brought back in line with the party.
const TICK_MS: u64 = 700;
const DRIFT_LIMIT_MS: i64 = 1_200;
const DRIFT_STRIKES: u32 = 2;
const DRIFT_COOLDOWN_MS: u64 = 6_000;
const PAUSED_TOLERANCE_MS: i64 = 400;
const ALIGN_TOLERANCE_MS: i64 = 120;
/// A seek is only reported when it really moved (two devices finishing a track together).
const SEEK_REPORT_FLOOR_MS: i64 = 1_000;
/// The press must do something even if the party never answers.
const DEFERRED_PLAY_TIMEOUT_MS: i64 = 1_800;
const PUBLISH_DEBOUNCE_MS: u64 = 120;
const INTENT_QUIET_MS: u64 = 4_500;
/// How many upcoming songs the party's queue carries onto the player.
const MAX_UPCOMING: usize = 25;
/// Autoplay.MAX_QUEUED_AUTOPLAY: how far AutoPlay tops the party up.
const MAX_QUEUED_AUTOPLAY: usize = 10;
/// A batch of autoplay songs waits this long for its echo before the seed frees up.
const QUEUE_ECHO_TIMEOUT_MS: u64 = 5_000;

/// A party session: the connection, the page's own state, and the sync's working
/// set. The party's truth itself lives in the `Party` client.
#[derive(Default)]
pub struct Session {
    conn: Option<Arc<Party>>,
    /// The code typed on the page (a code, or a whole invite link).
    code_input: String,
    /// A custom party server (empty: BitChord's).
    pub server: String,
    /// The name others see (saved); empty: the account's name.
    pub nickname: String,
    /// An invite being looked at, and what it said: code, server, answer, asked at.
    preview: Option<(String, Option<String>, Preview, Instant)>,
    preview_waiting: HashSet<String>,
    error: Option<String>,

    // ---- sync state ----
    /// The player's shape when it was last published (ids, index, playing, generation).
    published: Option<(Vec<String>, usize, bool, u64)>,
    publish_at: Option<Instant>,
    quiet_until: Option<Instant>,
    /// Server sequence numbers this device's sent controls will catch up to.
    await_seq: (i64, i64),
    /// The last control landed on exactly, so an echo of it isn't fought.
    aligned_seq: i64,
    aligned_at: (i64, i64, String, bool),
    drift_strikes: u32,
    drift_cooldown: Option<Instant>,
    /// A locked listener's own pause: the party's truth waits for them.
    locally_paused: bool,
    /// The song we started for the party, and when we asked for it.
    loaded: Option<String>,
    load_started: Option<Instant>,
    /// How long songs take to start here, so they're opened that far ahead.
    load_ms: u64,
    /// The last song reported to YouTube as played, so pauses don't count it twice.
    reported: Option<String>,
    /// A resume held back for the party's scheduled instant.
    start_at: Option<Instant>,
    /// The personal queue, put back after the party (PartyPersonalQueueStash).
    stash: Option<Stash>,
    last_reconcile: Option<Instant>,
    /// AutoPlay top-up: a batch under way since (the queue seq it waits for, when).
    topping: Option<(i64, Instant)>,
    /// The playing song the current autoplay seed was taken for.
    autoplay_seed: Option<String>,
}

#[derive(Clone, Debug)]
enum Preview {
    Loading,
    Answer(Result<party::PartyPreview, String>),
}

struct Stash {
    tracks: Vec<Track>,
    index: usize,
    position: Duration,
    was_playing: bool,
}

impl Session {
    /// The party's state as the server last stated it.
    fn state(&self) -> party::State {
        self.conn.as_ref().map(|c| c.state()).unwrap_or_default()
    }

    fn offset(&self) -> i64 {
        self.conn
            .as_ref()
            .and_then(|c| c.clock())
            .map(|(o, _)| o)
            .unwrap_or(0)
    }
}

/// Who this device jams as: the YouTube Music account it's signed in with when it
/// is one; otherwise a stable per-device identity. The id is a hash of the login
/// (or the device), never the login itself.
fn identity(app: &App) -> party::JoinRequest {
    let account = app.account_name.clone().unwrap_or_default();
    let device = party::device_id(&app.backend.data_dir());
    let seed = if account.trim().is_empty() {
        device.clone()
    } else {
        format!("{account}:{device}")
    };
    use sha2::{Digest, Sha256};
    let user_id = format!("{:x}", Sha256::digest(seed.as_bytes()))[..32].to_owned();
    let name = [
        app.party.nickname.trim().to_owned(),
        account.trim().to_owned(),
    ]
    .into_iter()
    .find(|n| !n.is_empty())
    .unwrap_or_else(|| "Guest".to_owned());
    let avatar = app.account_avatar.clone().filter(|a| a.starts_with("http"));
    party::JoinRequest {
        user_id,
        device_id: device,
        display_name: name,
        avatar_url: avatar,
        max_members: None,
        autoplay_enabled: None,
    }
}

// ---- In and out of the party --------------------------------------------------------

/// Start a party: whatever this device is playing comes with it.
pub fn start(app: &mut App) {
    if app.party.conn.is_some() {
        leave(app);
    }
    jam_stands_down(app);
    let autoplay = app.autoplay;
    let base = party::idle_server(&app.party.server);
    let mut id = identity(app);
    id.max_members = Some(5);
    id.autoplay_enabled = Some(autoplay);
    app.party.error = None;
    // The server hands out the code; an empty one asks it to create.
    app.party.conn = Some(Arc::new(Party::start(&app.backend, base, id, true, String::new())));
}

/// Join `code_or_link` (a 6-letter code or a whole invite link), on the custom
/// server an invite names.
pub fn join(app: &mut App, code_or_link: &str) {
    let (code, server) = match party::parse_invite(code_or_link) {
        Some((code, server)) => (Some(code), server),
        None => (party::clean_code(code_or_link), None),
    };
    let Some(code) = code else {
        app.party.error = Some(format!(
            "A party code is {} letters or digits",
            party::CODE_LEN
        ));
        return;
    };
    if let Some(message) = party::kicked_message(&app.backend.data_dir(), &code) {
        app.party.error = Some(message);
        return;
    }
    if app.party.conn.as_ref().is_some_and(|c| c.code() == code) {
        return;
    }
    leave(app);
    jam_stands_down(app);
    let base = server
        .map(|s| party::sanitize_server_url(&s).unwrap_or(s))
        .unwrap_or_else(|| party::idle_server(&app.party.server));
    app.party.error = None;
    app.party.conn = Some(Arc::new(Party::start(&app.backend, base, identity(app), false, code)));
}

/// An invite link (or a code from another launch) asked to join a party.
pub fn open_link(app: &mut App, code: String, server: Option<String>) {
    app.party_view = true;
    app.go(Page::Jam);
    match server {
        Some(server) => join(app, &party::invite_url(&code, Some(&server))),
        None => join(app, &code),
    }
}

/// Leave the party. Playback carries on locally, from the personal queue if the
/// party replaced one.
pub fn leave(app: &mut App) {
    if let Some(conn) = app.party.conn.take() {
        conn.leave();
    }
    reset_sync(app);
}

/// Only one room kind at a time: the relay jam stands down when a party starts.
fn jam_stands_down(app: &mut App) {
    if app.jam.active() {
        super::jam::leave(app);
    }
}

/// Reset the sync's working set, and give the personal queue back (the page's own
/// state stays).
fn reset_sync(app: &mut App) {
    let s = &mut app.party;
    s.published = None;
    s.publish_at = None;
    s.quiet_until = None;
    s.await_seq = (i64::MAX, i64::MAX);
    s.aligned_seq = -1;
    s.drift_strikes = 0;
    s.drift_cooldown = None;
    s.locally_paused = false;
    s.loaded = None;
    s.load_started = None;
    s.reported = None;
    s.start_at = None;
    s.last_reconcile = None;
    s.autoplay_seed = None;
    s.topping = None;
    if let Some(stash) = s.stash.take() {
        restore_stash(app, stash);
    }
}

/// Entering a party keeps the personal queue for afterwards.
fn stash_player(app: &mut App) {
    if app.party.stash.is_some() || app.queue.tracks.is_empty() {
        return;
    }
    let index = app.queue.index.unwrap_or(0).min(app.queue.tracks.len() - 1);
    app.party.stash = Some(Stash {
        tracks: app.queue.tracks.clone(),
        index,
        position: app.audio.status.position(),
        was_playing: app.playing(),
    });
}

/// The personal queue comes back after the party, where it left off.
fn restore_stash(app: &mut App, stash: Stash) {
    if stash.index >= stash.tracks.len() {
        return;
    }
    app.queue_source = None;
    app.queue.fill = None;
    app.play_pending = None;
    app.queue.tracks = stash.tracks;
    app.queue.index = Some(stash.index);
    if stash.was_playing {
        app.play_current_at(stash.position, true);
    } else {
        app.audio.send(AudioCmd::Stop);
        app.resume_at = Some(stash.position);
        app.audio.status.position_ms.store(
            stash.position.as_millis() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
        app.audio.status.duration_ms.store(
            app.current()
                .and_then(|t| t.duration)
                .map_or(0, |d| d as u64 * 1000),
            std::sync::atomic::Ordering::Relaxed,
        );
        app.request_art();
    }
}

// ---- Tracks across the boundary -----------------------------------------------------

fn to_party_track(track: &Track, duration_ms: u64) -> PartyTrack {
    PartyTrack {
        video_id: track.id.clone(),
        title: track.title.clone(),
        artist: track.artist_line(),
        thumbnail_url: track
            .thumbs
            .iter()
            .max_by_key(|t| t.width)
            .map(|t| t.url.clone()),
        duration_ms: Some(if duration_ms > 0 {
            duration_ms as i64
        } else {
            track.duration.map(|s| s as i64 * 1000).unwrap_or(0)
        }),
        from_autoplay: false,
    }
}

fn from_party_track(track: &PartyTrack) -> Track {
    Track {
        id: track.video_id.clone(),
        title: track.title.clone(),
        artists: if track.artist.is_empty() {
            vec![]
        } else {
            vec![crate::model::Link {
                name: track.artist.clone(),
                id: None,
            }]
        },
        album: None,
        duration: track
            .duration_ms
            .filter(|ms| *ms > 0)
            .map(|ms| (ms / 1000) as u32),
        thumbs: track
            .thumbnail_url
            .as_ref()
            .map(|url| {
                vec![crate::model::Thumb {
                    url: url.clone(),
                    width: 544,
                    height: 544,
                }]
            })
            .unwrap_or_default(),
        track_nr: None,
        plays: None,
        music_video: false,
        explicit: false,
    }
}

/// Whether the player means to be playing: going, or buffering a track it will start.
fn play_when_ready(app: &App) -> bool {
    app.playing() || app.buffering
}

// ---- Every frame ---------------------------------------------------------------------

pub fn tick(app: &mut App, ctx: &egui::Context) {
    // An invite lookup that came back.
    if let Some((code, answer)) = PREVIEWS.lock().unwrap().take() {
        app.party.preview_waiting.remove(&code);
        if let Some(slot) = &mut app.party.preview
            && slot.0 == code
        {
            slot.2 = Preview::Answer(answer);
        }
    }
    // What the party said since last frame.
    let mut events = Vec::new();
    if let Some(conn) = &app.party.conn {
        while let Some(e) = conn.try_recv() {
            events.push(e);
        }
    }
    for e in events {
        on_event(app, e);
    }
    if app.party.conn.is_none() {
        return;
    }
    // A held-back resume that never came: the press still does something.
    if let Some(at) = app.party.start_at {
        if at <= Instant::now() {
            app.party.start_at = None;
            if app.party.state().awaiting_start {
                if let Some(want) = app.party.conn.as_ref().and_then(|c| c.position_now()) {
                    app.audio.send(AudioCmd::SeekLive {
                        pos: Duration::from_millis(want.max(0) as u64),
                        at: Instant::now(),
                    });
                }
                app.audio.send(AudioCmd::Resume);
                app.party.locally_paused = false;
                if let Some(conn) = &app.party.conn {
                    conn.set_awaiting(false);
                }
                app.party.autoplay_seed = None;
            }
        } else {
            ctx.request_repaint_after(Duration::from_millis(
                (at - Instant::now()).as_millis() as u64 + 50,
            ));
        }
    }
    let state = app.party.state();
    let locked = party::controls_locked(&state);
    // Outbound: what changed in the player goes to the party (debounced).
    if !locked && publish_due(app) {
        app.party.publish_at = Some(Instant::now() + Duration::from_millis(PUBLISH_DEBOUNCE_MS));
    }
    if let Some(at) = app.party.publish_at
        && at <= Instant::now()
    {
        app.party.publish_at = None;
        publish(app);
    }
    // A burst of own sends settled, or the party moved past them: listen again.
    if state.playback.seq >= app.party.await_seq.0 && state.queue.seq >= app.party.await_seq.1 {
        app.party.quiet_until = None;
    }
    // An autoplay batch that never echoed frees its seed.
    if let Some((seq, since)) = app.party.topping {
        if state.queue.seq != seq {
            app.party.topping = None;
        } else if since.elapsed() >= Duration::from_millis(QUEUE_ECHO_TIMEOUT_MS) {
            app.party.topping = None;
            app.party.autoplay_seed = None;
        }
    }
    // Inbound: catch up with the party every 700 ms.
    if app
        .party
        .last_reconcile
        .is_none_or(|t| t.elapsed() >= Duration::from_millis(TICK_MS))
    {
        app.party.last_reconcile = Some(Instant::now());
        reconcile(app, ctx);
        top_up_autoplay(app);
    }
}

fn on_event(app: &mut App, e: party::Event) {
    match e {
        party::Event::Adopted => {
            let host = app.party.state().you.as_ref().is_some_and(|y| y.is_host);
            // Entering a party keeps the personal queue for afterwards.
            stash_player(app);
            app.party.await_seq = (i64::MAX, i64::MAX);
            app.party.quiet_until = None;
            app.party.published = None;
            app.party.last_reconcile = None;
            if host {
                app.toast("Party started: share the code or link");
            } else {
                app.toast("Joined the party");
            }
        }
        party::Event::Connection(_) => {}
        party::Event::Ended(reason) => {
            if app.party.conn.take().is_some() {
                let error = app.party.error.clone();
                reset_sync(app);
                if !reason.is_empty() {
                    app.party.error = Some(reason);
                } else {
                    app.party.error = error;
                }
            }
        }
    }
}

/// Whether the player changed in a way the party should hear about (a new song, a
/// play/pause, the queue moving on by itself). Seeks go out as intents instead,
/// so ordinary playback progress doesn't publish every frame.
fn publish_due(app: &App) -> bool {
    let index = app.queue.index.unwrap_or(0);
    // The same slice publish() sends: everything from the top, clamped a little
    // past the current song.
    let take = index.saturating_add(1).saturating_add(MAX_UPCOMING);
    let ids: Vec<String> = app
        .queue
        .tracks
        .iter()
        .take(take)
        .map(|t| t.id.clone())
        .collect();
    let playing = play_when_ready(app);
    let changed = app
        .party
        .published
        .as_ref()
        .is_none_or(|(pids, pindex, pplaying, pgen)| {
            ids != *pids || index != *pindex || playing != *pplaying || app.generation != *pgen
        });
    changed && app.party.quiet_until.is_none_or(|t| t <= Instant::now())
}

/// A listener's or host's action went through: the party hears about it, and its
/// own echo of the control is waited out.
pub fn on_intent(app: &mut App) {
    if app.party.conn.is_none() {
        return;
    }
    if party::controls_locked(&app.party.state()) {
        return;
    }
    app.party.quiet_until = Some(Instant::now() + Duration::from_millis(INTENT_QUIET_MS));
    // Nothing published yet: no seq to wait for, and someone else's control mustn't
    // clear the window.
    app.party.await_seq = (i64::MAX, i64::MAX);
    app.party.publish_at = Some(Instant::now() + Duration::from_millis(PUBLISH_DEBOUNCE_MS));
}

/// What this device is playing, sent to the party (PartySync.publish): the queue
/// (past, current, upcoming) and the transport, each only when it really changed.
fn publish(app: &mut App) {
    let Some(conn) = app.party.conn.clone() else {
        return;
    };
    let state = conn.state();
    if party::controls_locked(&state) {
        return;
    }
    let index = app.queue.index;
    let Some(entry) = app.current().cloned() else {
        // Nothing playing here: nothing to publish (a new party plays nothing until
        // someone presses something).
        app.party.published.replace((
            app.queue.tracks.iter().map(|t| t.id.clone()).collect(),
            index.unwrap_or(0),
            false,
            app.generation,
        ));
        return;
    };
    let index = index.unwrap_or(0);
    let duration_ms = app
        .audio
        .status
        .duration_ms
        .load(std::sync::atomic::Ordering::Relaxed);
    let track = to_party_track(&entry, duration_ms);
    let position = app.audio.status.position().as_millis() as i64;
    let wants_playing = state.awaiting_start || play_when_ready(app);
    let base_playback = state.playback.seq;
    let base_queue = state.queue.seq;
    let mut playback_controls = 0u64;
    let mut queue_controls = 0u64;

    // The party's queue is the server's: everything from the top, clamped a little
    // past the current song.
    let take = index.saturating_add(1).saturating_add(MAX_UPCOMING);
    let local: Vec<PartyTrack> = app
        .queue
        .tracks
        .iter()
        .take(take)
        .map(|t| to_party_track(t, 0))
        .collect();
    let local_ids: Vec<String> = local.iter().map(|t| t.video_id.clone()).collect();
    let party_ids: Vec<String> = state
        .queue
        .items
        .iter()
        .map(|t| t.video_id.clone())
        .collect();
    let party_index = party::party_queue_index_of(
        &state.queue,
        &state.playback,
        state.playback.track.as_ref().map(|t| t.video_id.as_str()),
    );
    let upcoming_party: Vec<PartyTrack> = if party_index >= 0 {
        state.queue.items[(party_index as usize + 1).min(state.queue.items.len())..].to_vec()
    } else {
        vec![]
    };

    if state.playback.track.as_ref().map(|t| t.video_id.as_str()) != Some(entry.id.as_str())
        && local_ids.len() == 1
        && !upcoming_party.is_empty()
    {
        // A new song on its own: the party's hand-queued songs carry on after it.
        let mut queue = vec![track.clone()];
        queue.extend(
            upcoming_party
                .iter()
                .filter(|t| !t.from_autoplay)
                .take(MAX_UPCOMING)
                .cloned(),
        );
        conn.set_queue(&queue, 0);
        queue_controls += 1;
    } else if local_ids != party_ids {
        let move_to = if party_ids.len() == local_ids.len()
            && (index as i64) < party_ids.len() as i64
            && party_ids.get(index) == local_ids.get(index)
        {
            party::detect_single_move(&party_ids, &local_ids)
        } else {
            None
        };
        if let Some((from, to, _)) = move_to
            && from > index
            && to > index
        {
            conn.queue_move(from, to);
        } else {
            conn.set_queue(&local, index);
        }
        queue_controls += 1;
    }

    if state.playback.track.as_ref().map(|t| t.video_id.as_str()) != Some(entry.id.as_str()) {
        // After the queue, never before: a track pointing into a running order
        // nobody has yet plays the wrong song.
        conn.set_track(&track, position, wants_playing);
        playback_controls += 1;
    } else if state.playback.is_playing != wants_playing {
        if wants_playing {
            conn.play(Some(position));
        } else {
            conn.pause(Some(position));
        }
        playback_controls += 1;
    } else {
        // Same track, same state: a seek, unless it barely moved.
        let party_position = party::position_now(&state, app.party.offset()).unwrap_or(position);
        if (position - party_position).abs() > SEEK_REPORT_FLOOR_MS {
            conn.seek(position);
            playback_controls += 1;
        }
    }

    app.party.await_seq = (
        base_playback + playback_controls as i64,
        base_queue + queue_controls as i64,
    );
    if playback_controls == 0 && queue_controls == 0 {
        app.party.quiet_until = None;
    } else {
        app.party.quiet_until = Some(Instant::now() + Duration::from_millis(INTENT_QUIET_MS));
    }
    app.party.published = Some((local_ids, index, play_when_ready(app), app.generation));
}

/// A listener's action in a locked party: their own pause, and a resume that
/// rejoins where the party is. Returns the action if it should still run.
pub fn locked_action(app: &mut App, a: Action) -> Option<Action> {
    let state = app.party.state();
    if app.party.conn.is_none() || !party::controls_locked(&state) {
        return Some(a);
    }
    match a {
        Action::TogglePlay => {
            if play_when_ready(app) {
                // A listener may pause their own device.
                app.party.locally_paused = true;
                app.audio.send(AudioCmd::Pause);
            } else {
                rejoin(app, &state);
            }
            None
        }
        Action::Play(tracks, i)
            if tracks
                .get(i)
                .is_some_and(|t| app.current_id() == Some(t.id.as_str()))
                && !play_when_ready(app) =>
        {
            rejoin(app, &state);
            None
        }
        _ => Some(a),
    }
}

/// A resume in a locked party lands where the party is.
fn rejoin(app: &mut App, state: &party::State) {
    app.party.locally_paused = false;
    if !state.playback.is_playing {
        return;
    }
    if state.clock_synced
        && let Some(want) = party::position_now(state, app.party.offset())
    {
        app.audio
            .send(AudioCmd::Seek(Duration::from_millis(want.max(0) as u64)));
    }
    app.audio.send(AudioCmd::Resume);
}

// ---- Inbound -------------------------------------------------------------------------

/// Get playback to where the party is (PartySync.reconcile).
fn reconcile(app: &mut App, ctx: &egui::Context) {
    let state = app.party.state();
    if state.code.is_none() {
        app.party.locally_paused = false;
        app.party.loaded = None;
        return;
    }
    if app.party.locally_paused && !party::controls_locked(&state) {
        app.party.locally_paused = false;
    }
    if app.party.quiet_until.is_some_and(|t| t > Instant::now()) {
        return;
    }
    let target = state.playback.clone();
    let Some(track) = &target.track else {
        // A new party plays nothing until someone presses something; the host hands
        // it what's already playing.
        if state.connection == party::Connection::Live
            && state.you.as_ref().is_some_and(|y| y.is_host)
            && app.current().is_some()
        {
            publish(app);
        }
        return;
    };
    let Some(entry) = app.current() else {
        load(app, &state);
        return;
    };
    if entry.id != track.video_id {
        load(app, &state);
        return;
    }
    if app.party.loaded.as_deref() == Some(track.video_id.as_str()) && app.buffering {
        return;
    }
    keep_queue(app, &state);

    if app.party.locally_paused {
        app.party.start_at = None;
        if let Some(conn) = &app.party.conn {
            conn.set_awaiting(false);
        }
        return;
    }

    if !target.is_playing {
        if let Some(conn) = &app.party.conn {
            conn.set_awaiting(false);
        }
        app.party.start_at = None;
        if play_when_ready(app) {
            app.audio.send(AudioCmd::Pause);
        }
        let pos = app.audio.status.position().as_millis() as i64;
        if (pos - target.position_ms).abs() > PAUSED_TOLERANCE_MS {
            // Paused: show the song at the party's position without loading it.
            app.audio.send(AudioCmd::Stop);
            app.party.loaded = None;
            app.resume_at = Some(Duration::from_millis(target.position_ms.max(0) as u64));
            app.audio.status.position_ms.store(
                target.position_ms.max(0) as u64,
                std::sync::atomic::Ordering::Relaxed,
            );
            app.audio.status.duration_ms.store(
                track
                    .duration_ms
                    .filter(|d| *d > 0)
                    .map(|d| d as u64)
                    .unwrap_or(0),
                std::sync::atomic::Ordering::Relaxed,
            );
            app.request_art();
        }
        return;
    }
    // A playing party can't be joined in time without a measured clock.
    if !state.clock_synced {
        return;
    }
    let want = party::position_now(&state, app.party.offset());
    if !play_when_ready(app) {
        let wait = party::ms_until_start(&state, app.party.offset());
        if wait > 0 {
            // Woken for the scheduled instant, not the next tick: late is what this
            // avoids. The press must do something even if the party never answers,
            // so the wait is capped.
            if let Some(conn) = &app.party.conn {
                conn.set_awaiting(true);
            }
            let deadline = wait.min(DEFERRED_PLAY_TIMEOUT_MS).max(50);
            app.party.start_at = Some(Instant::now() + Duration::from_millis(deadline as u64));
            ctx.request_repaint_after(Duration::from_millis(deadline as u64 + 100));
            return;
        }
        if let Some(w) = want {
            app.audio
                .send(AudioCmd::Seek(Duration::from_millis(w.max(0) as u64)));
        }
        if let Some(conn) = &app.party.conn {
            conn.set_awaiting(false);
        }
        app.party.start_at = None;
        app.audio.send(AudioCmd::Resume);
        app.party.aligned_seq = target.seq;
        return;
    }
    if let Some(conn) = &app.party.conn {
        conn.set_awaiting(false);
    }
    app.party.start_at = None;
    let Some(want) = want else { return };
    // Only a settled player can be measured.
    if app.buffering {
        app.party.drift_strikes = 0;
        return;
    }

    let drift = app.audio.status.position().as_millis() as i64 - want;
    if target.seq != app.party.aligned_seq {
        // A control is landed on exactly, even by the device that sent it; a
        // queue-only change isn't one.
        let anchor_changed = app.party.aligned_at.0 != target.anchor_ms
            || app.party.aligned_at.1 != target.position_ms
            || app.party.aligned_at.2 != track.video_id
            || app.party.aligned_at.3 != target.is_playing;
        app.party.aligned_seq = target.seq;
        app.party.aligned_at = (
            target.anchor_ms,
            target.position_ms,
            track.video_id.clone(),
            target.is_playing,
        );
        if anchor_changed && drift.abs() > ALIGN_TOLERANCE_MS {
            app.audio.send(AudioCmd::SeekLive {
                pos: Duration::from_millis(want.max(0) as u64),
                at: Instant::now(),
            });
            return;
        }
    }
    if drift.abs() <= DRIFT_LIMIT_MS {
        app.party.drift_strikes = 0;
        return;
    }
    // A seek costs a re-buffer; asked for twice in a row, then left alone to settle.
    let now = Instant::now();
    if app.party.drift_cooldown.is_some_and(|t| t > now) {
        return;
    }
    app.party.drift_strikes += 1;
    if app.party.drift_strikes < DRIFT_STRIKES {
        return;
    }
    app.party.drift_strikes = 0;
    app.party.drift_cooldown = Some(now + Duration::from_millis(DRIFT_COOLDOWN_MS));
    app.audio.send(AudioCmd::SeekLive {
        pos: Duration::from_millis(want.max(0) as u64),
        at: Instant::now(),
    });
}

/// The party's running order onto the player, at its position; started only if the
/// party is going now (PartySync.load).
fn load(app: &mut App, state: &party::State) {
    let Some(track) = &state.playback.track else {
        return;
    };
    if app.party.loaded.as_deref() == Some(track.video_id.as_str()) {
        return;
    }
    // A not-found index plays the track alone, never queue[0].
    let at =
        party::party_queue_index_of(&state.queue, &state.playback, Some(track.video_id.as_str()));
    let items: Vec<PartyTrack> = if at >= 0 {
        state.queue.items.clone()
    } else {
        vec![track.clone()]
    };
    let start_ms =
        party::position_now(state, app.party.offset()).unwrap_or(state.playback.position_ms);
    let play = state.playback.is_playing
        && state.clock_synced
        && party::ms_until_start(state, app.party.offset()) <= 0;
    app.queue_source = None;
    app.queue.fill = None;
    app.play_pending = None;
    app.queue.tracks = items.iter().map(from_party_track).collect();
    app.queue.index = Some(if at >= 0 {
        (at as usize).min(app.queue.tracks.len() - 1)
    } else {
        0
    });
    if play {
        // Opened a little ahead: by the time it starts, the party is there.
        let ahead = Duration::from_millis(app.party.load_ms);
        // Joining a song counts as a play; catching up on the same one doesn't.
        let report = app.party.reported.as_deref() != Some(track.video_id.as_str());
        app.party.reported = Some(track.video_id.clone());
        app.party.loaded = Some(track.video_id.clone());
        app.party.load_started = Some(Instant::now());
        app.play_current_at(
            Duration::from_millis(start_ms.max(0) as u64) + ahead,
            report,
        );
    } else {
        // Paused: show the song at the party's position without loading it.
        app.audio.send(AudioCmd::Stop);
        app.party.loaded = None;
        app.resume_at = Some(Duration::from_millis(start_ms.max(0) as u64));
        app.audio
            .status
            .position_ms
            .store(start_ms.max(0) as u64, std::sync::atomic::Ordering::Relaxed);
        app.audio.status.duration_ms.store(
            track
                .duration_ms
                .filter(|d| *d > 0)
                .map(|d| d as u64)
                .unwrap_or(0),
            std::sync::atomic::Ordering::Relaxed,
        );
        app.request_art();
    }
}

/// Keeps the player's upcoming tracks equal to the party's, without touching the
/// playing one (PartySync.reconcileQueue).
fn keep_queue(app: &mut App, state: &party::State) {
    if state.queue.items.is_empty() {
        return;
    }
    let Some(entry) = app.current() else { return };
    let at = party::party_queue_index_of(&state.queue, &state.playback, Some(entry.id.as_str()));
    if at < 0 {
        return;
    }
    let desired: Vec<PartyTrack> = state
        .queue
        .items
        .iter()
        .skip((at as usize + 1).min(state.queue.items.len()))
        .take(MAX_UPCOMING)
        .cloned()
        .collect();
    let index = app.queue.index.unwrap_or(0);
    let local: Vec<String> = app
        .queue
        .tracks
        .get(index + 1..)
        .map(|rest| rest.iter().map(|t| t.id.clone()).collect())
        .unwrap_or_default();
    let wanted: Vec<String> = desired.iter().map(|t| t.video_id.clone()).collect();
    if local == wanted {
        return;
    }
    let mut tracks = app.queue.tracks.clone();
    tracks.truncate(index + 1);
    tracks.extend(desired.iter().map(from_party_track));
    app.queue.tracks = tracks;
}

// ---- AutoPlay: one supplier for the whole party ---------------------------------------

/// Tops the party up to 10 AutoPlay tracks after the playing one, from the elected
/// supplier only, counted against the server's queue and one batch at a time
/// (PartySync.maybeTopUpAutoplay): otherwise the same station went out twice.
fn top_up_autoplay(app: &mut App) {
    let state = app.party.state();
    let Some(playing) = &state.playback.track else {
        return;
    };
    if state.code.is_none() || !state.playback.autoplay_enabled {
        return;
    }
    if party::autoplay_supplier_id(&state.members, state.host_only_control).as_deref()
        != state.you.as_ref().map(|y| y.member_id.as_str())
    {
        return;
    }
    if party::party_queue_index_of(
        &state.queue,
        &state.playback,
        Some(playing.video_id.as_str()),
    ) < 0
    {
        return;
    }
    let waiting = party::party_upcoming_after(
        &state.queue,
        &state.playback,
        Some(playing.video_id.as_str()),
    );
    let needed =
        MAX_QUEUED_AUTOPLAY.saturating_sub(waiting.iter().filter(|t| t.from_autoplay).count());
    if needed == 0
        || app.party.autoplay_seed.as_deref() == Some(playing.video_id.as_str())
        || app.party.topping.is_some()
    {
        return;
    }
    app.party.autoplay_seed = Some(playing.video_id.clone());
    app.party.topping = Some((state.queue.seq, Instant::now()));
    let seed = playing.video_id.clone();
    let Some(conn) = app.party.conn.clone() else {
        return;
    };
    let api = app.backend.api();
    let existing: HashSet<String> = app
        .queue
        .tracks
        .iter()
        .map(|t| t.id.clone())
        .chain(state.queue.items.iter().map(|t| t.video_id.clone()))
        .collect();
    let waiting_ids: HashSet<String> = waiting.iter().map(|t| t.video_id.clone()).collect();
    app.backend.spawn(async move {
        let mut fresh: Vec<PartyTrack> = Vec::new();
        match crate::ytm::radio(&api, &seed, false, 50).await {
            Ok(candidates) => {
                for t in candidates {
                    if fresh.len() >= needed {
                        break;
                    }
                    if !existing.contains(&t.id) && !waiting_ids.contains(&t.id) {
                        let mut p = to_party_track(&t, 0);
                        p.from_autoplay = true;
                        fresh.push(p);
                    }
                }
            }
            Err(e) => log::info!("party: autoplay radio: {e:#}"),
        }
        if !fresh.is_empty() {
            log::info!("party: topping up {} autoplay songs", fresh.len());
            conn.queue_add(&fresh, false);
        } else {
            // Nothing to add: the seed is freed when the echo wait runs out, and the
            // next tick tries again with the station it then has.
        }
        conn.wake();
    });
}

// -------------------------------------------------------------------------------- page

pub fn page(app: &mut App, ui: &mut Ui) {
    let avail = ui.available_width();
    let w = avail.min(720.0);
    ui.horizontal(|ui| {
        ui.add_space(((avail - w) / 2.0).max(0.0));
        ui.vertical(|ui| {
            ui.set_width(w);
            // The invite card stands alone when an invite is being looked at.
            if let Some((code, server, _, _)) = app.party.preview.clone()
                && app.party.conn.is_none()
            {
                invite_card(app, ui, &code, server.as_deref());
                return;
            }
            if app.party.conn.is_some() {
                room(app, ui);
            } else {
                lobby(app, ui);
            }
        });
    });
}

fn dim(ui: &mut Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .font(theme::regular(13.5))
            .color(theme::text_dim()),
    );
}

fn lobby(app: &mut App, ui: &mut Ui) {
    dim(
        ui,
        "Listen to the same music at the same moment with up to five devices, on the party server the web app and the phone use. Anyone in the party can play, pause and skip, unless the host takes over.",
    );
    section_label(ui, "Start a party", None);
    settings_card(ui, |ui| {
        let hint = app
            .account_name
            .clone()
            .unwrap_or_else(|| "Guest".to_owned());
        setting_row(
            ui,
            "Your name",
            Some("What others in the party see."),
            |ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(200.0, 36.0), Sense::hover());
                ui.painter().rect_filled(rect, 10.0, theme::glass(0.08));
                let inner = rect.shrink2(vec2(12.0, 4.0));
                ui.put(
                    inner,
                    egui::TextEdit::singleline(&mut app.party.nickname)
                        .hint_text(RichText::new(hint).color(theme::text_faint()))
                        .font(theme::regular(14.0))
                        .char_limit(80)
                        .frame(egui::Frame::NONE)
                        .align(Align2::LEFT_CENTER)
                        .desired_width(inner.width()),
                );
            },
        );
        divider(ui);
        setting_row(
            ui,
            "Start",
            Some("Whatever you're playing comes with you."),
            |ui| {
                if pill(ui, "Start", Some(icon::USERS), true).clicked() {
                    start(app);
                }
            },
        );
    });
    section_label(ui, "Join a party", None);
    settings_card(ui, |ui| {
        ui.add_space(12.0);
        dim(
            ui,
            "Type the code from the host, or paste the link they shared.",
        );
        ui.add_space(10.0);
        let mut join_now = false;
        let mut preview_now = false;
        ui.allocate_ui_with_layout(
            vec2(ui.available_width(), 42.0),
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                if pill(ui, "Join", None, true).clicked() {
                    join_now = true;
                }
                ui.add_space(8.0);
                if pill(ui, "Look up", None, false).clicked() {
                    preview_now = true;
                }
                ui.add_space(10.0);
                let (rect, _) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 42.0), Sense::hover());
                ui.painter().rect_filled(rect, 10.0, theme::glass(0.08));
                let inner = rect.shrink2(vec2(14.0, 4.0));
                let edit = ui.put(
                    inner,
                    egui::TextEdit::singleline(&mut app.party.code_input)
                        .hint_text(RichText::new("ABC123").color(theme::text_faint()))
                        .font(theme::semibold(16.0))
                        .char_limit(120)
                        .frame(egui::Frame::NONE)
                        .align(Align2::LEFT_CENTER)
                        .desired_width(inner.width()),
                );
                if edit.changed() {
                    app.party.error = None;
                }
                join_now |= edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            },
        );
        if join_now {
            let code = app.party.code_input.clone();
            join(app, &code);
        }
        if preview_now {
            look_up(app, &app.party.code_input.clone());
        }
        if let Some(e) = &app.party.error {
            ui.add_space(8.0);
            ui.label(
                RichText::new(e)
                    .font(theme::regular(13.0))
                    .color(theme::RED),
            );
        }
        ui.add_space(14.0);
    });
    section_label(ui, "You", None);
    settings_card(ui, |ui| {
        setting_row(
            ui,
            "Name in parties",
            Some("Empty: your YouTube Music account's name."),
            |ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(220.0, 36.0), Sense::hover());
                ui.painter().rect_filled(rect, 10.0, theme::glass(0.08));
                let inner = rect.shrink2(vec2(12.0, 4.0));
                ui.put(
                    inner,
                    egui::TextEdit::singleline(&mut app.party.nickname)
                        .hint_text(RichText::new("Your YouTube name").color(theme::text_faint()))
                        .font(theme::regular(14.0))
                        .char_limit(80)
                        .frame(egui::Frame::NONE)
                        .align(Align2::LEFT_CENTER)
                        .desired_width(inner.width()),
                );
            },
        );
        divider(ui);
        setting_row(
            ui,
            "Party server",
            Some(
                "Leave empty to use BitChord's. Anyone joining your party uses the server it's on.",
            ),
            |ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(220.0, 36.0), Sense::hover());
                ui.painter().rect_filled(rect, 10.0, theme::glass(0.08));
                let inner = rect.shrink2(vec2(12.0, 4.0));
                ui.put(
                    inner,
                    egui::TextEdit::singleline(&mut app.party.server)
                        .hint_text(RichText::new("party.example.com").color(theme::text_faint()))
                        .font(theme::regular(14.0))
                        .char_limit(120)
                        .frame(egui::Frame::NONE)
                        .align(Align2::LEFT_CENTER)
                        .desired_width(inner.width()),
                );
            },
        );
    });
}

/// Look a party up before joining it: who's in it, and whether it's full.
fn look_up(app: &mut App, code_or_link: &str) {
    let (code, server) = match party::parse_invite(code_or_link) {
        Some((code, server)) => (Some(code), server),
        None => (party::clean_code(code_or_link), None),
    };
    let Some(code) = code else {
        app.party.error = Some(format!(
            "A party code is {} letters or digits",
            party::CODE_LEN
        ));
        return;
    };
    if let Some(message) = party::kicked_message(&app.backend.data_dir(), &code) {
        app.party.error = Some(message);
        return;
    }
    app.party.preview = Some((
        code.clone(),
        server.clone(),
        Preview::Loading,
        Instant::now(),
    ));
    if !app.party.preview_waiting.insert(code.clone()) {
        return;
    }
    let base = server
        .clone()
        .map(|s| party::sanitize_server_url(&s).unwrap_or(s))
        .unwrap_or_else(|| party::idle_server(&app.party.server));
    let http = app.backend.http();
    let b = app.backend.clone();
    app.backend.spawn(async move {
        let answer = party::preview(&http, &base, &code).await;
        *PREVIEWS.lock().unwrap() = Some((code, answer));
        b.wake();
    });
}

/// The preview answers, from their task.
static PREVIEWS: std::sync::Mutex<Option<(String, Result<party::PartyPreview, String>)>> =
    std::sync::Mutex::new(None);

/// An invite: who's in it, then a press to join.
fn invite_card(app: &mut App, ui: &mut Ui, code: &str, server: Option<&str>) {
    settings_card(ui, |ui| {
        ui.add_space(16.0);
        dim(ui, "A Listen Together party");
        ui.add_space(6.0);
        let spaced: String = code
            .chars()
            .map(|c| c.to_string())
            .collect::<Vec<_>>()
            .join(" ");
        ui.label(
            RichText::new(spaced)
                .font(theme::bold(40.0))
                .color(theme::text()),
        );
        match &app.party.preview {
            Some((c, _, Preview::Loading, _)) if c == code => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    dim(ui, "Looking up the party…");
                });
            }
            Some((c, _, Preview::Answer(Err(e)), _)) if c == code => {
                ui.label(
                    RichText::new(e)
                        .font(theme::regular(13.0))
                        .color(theme::RED),
                );
            }
            Some((c, _, Preview::Answer(Ok(p)), _)) if c == code => {
                let who = if p.host_name.is_empty() {
                    "A Listen Together party".to_owned()
                } else {
                    format!("{}'s party", p.host_name)
                };
                let count = if p.member_count > 0 {
                    format!(" · {} of {} listening", p.member_count, p.max_members)
                } else {
                    String::new()
                };
                dim(ui, &format!("{who}{count}"));
            }
            _ => {}
        }
        ui.add_space(10.0);
        let full = matches!(&app.party.preview, Some((c, _, Preview::Answer(Ok(p)), _)) if c == code && p.is_full);
        ui.horizontal(|ui| {
            if full {
                pill(ui, "This party is full", None, false);
            } else if pill(ui, "Join", Some(icon::USERS), true).clicked() {
                let link = party::invite_url(code, server);
                app.party.preview = None;
                join(app, &link);
            }
            ui.add_space(8.0);
            if pill(ui, "Back", None, false).clicked() {
                app.party.preview = None;
            }
        });
        ui.add_space(16.0);
    });
}

fn room(app: &mut App, ui: &mut Ui) {
    let Some(conn) = app.party.conn.clone() else {
        return;
    };
    let state = conn.state();
    let code = conn.code();
    let locked = party::controls_locked(&state);
    let host = state.you.as_ref().is_some_and(|y| y.is_host);
    let status = match state.connection {
        party::Connection::Live if state.clock_synced => {
            format!("In sync · {} ms round trip", state.round_trip_ms)
        }
        party::Connection::Live => "Measuring the clock…".to_owned(),
        party::Connection::Connecting => "Reconnecting…".to_owned(),
        party::Connection::Offline => "Offline".to_owned(),
    };
    settings_card(ui, |ui| {
        ui.add_space(16.0);
        dim(
            ui,
            if host {
                "Share this code or link with friends"
            } else {
                "You're in the party"
            },
        );
        ui.add_space(6.0);
        let spaced: String = code
            .chars()
            .map(|c| c.to_string())
            .collect::<Vec<_>>()
            .join(" ");
        ui.label(
            RichText::new(spaced)
                .font(theme::bold(40.0))
                .color(theme::text()),
        );
        if state.connection != party::Connection::Live || !state.clock_synced {
            ui.horizontal(|ui| {
                ui.spinner();
                dim(ui, &status);
            });
        } else {
            dim(ui, &status);
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if pill(ui, "Copy invite link", Some(icon::LINK), true).clicked() {
                let custom =
                    (conn.server() != party::DEFAULT_SERVER).then(|| conn.server().clone());
                app.share_pending = Some(party::invite_url(&code, custom.as_deref()));
                app.toast("Link copied");
            }
            ui.add_space(8.0);
            if pill(ui, "Copy code", Some(icon::COPY), false).clicked() {
                app.share_pending = Some(code.clone());
                app.toast("Code copied");
            }
        });
        ui.add_space(16.0);
    });

    section_label(ui, "Party", None);
    draw_party_controls(app, ui, &state, host, locked);

    let n = state.members.len();
    section_label(
        ui,
        "Listening",
        Some(&format!("{n} of {}", state.max_members)),
    );
    settings_card(ui, |ui| {
        ui.add_space(6.0);
        let you = state.you.as_ref().map(|y| y.member_id.clone());
        for m in &state.members {
            let (rect, _) =
                ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
            let is_host = m.is_host;
            icon_at(
                ui,
                pos2(rect.left() + 12.0, rect.center().y),
                if is_host {
                    icon::CROWN
                } else {
                    icon::HEADPHONES
                },
                17.0,
                if is_host {
                    app.accent
                } else {
                    theme::text_dim()
                },
            );
            let mine = you.as_ref().is_some_and(|y| *y == m.member_id);
            let suffix = if mine { " (you)" } else { "" };
            ui.painter().text(
                pos2(rect.left() + 34.0, rect.center().y),
                Align2::LEFT_CENTER,
                format!("{}{suffix}", m.display_name),
                theme::semibold(14.5),
                theme::text(),
            );
            // Connected now (holding a socket), or off trying to come back.
            let (dot, dot_color) = if m.connected {
                (rect.right() - 14.0, theme::GREEN)
            } else {
                (rect.right() - 14.0, theme::text_faint())
            };
            ui.painter()
                .circle_filled(pos2(dot, rect.center().y), 3.5, dot_color);
            if host && !mine && pill_at(ui, rect, "Remove") {
                conn.kick(&m.member_id);
            }
        }
        ui.add_space(6.0);
    });

    if !state.activity.is_empty() {
        section_label(ui, "Activity", None);
        settings_card(ui, |ui| {
            ui.add_space(6.0);
            for a in state.activity.iter().take(8) {
                let (rect, _) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::hover());
                ui.painter().text(
                    pos2(rect.left() + 4.0, rect.center().y),
                    Align2::LEFT_CENTER,
                    format!("{} · {}", a.by, a.action),
                    theme::regular(13.5),
                    theme::text(),
                );
                if !a.detail.is_empty() {
                    ui.painter().text(
                        pos2(rect.right() - 4.0, rect.center().y),
                        Align2::RIGHT_CENTER,
                        &a.detail,
                        theme::regular(12.5),
                        theme::text_dim(),
                    );
                }
            }
            ui.add_space(6.0);
        });
    }

    if locked {
        dim(
            ui,
            "The host controls the music in this party. You can still pause your own device.",
        );
        ui.add_space(6.0);
    }
    settings_card(ui, |ui| {
        setting_row(
            ui,
            "Leave party",
            Some("Your music keeps playing here, on your own."),
            |ui| {
                if pill(ui, "Leave", Some(icon::LOG_OUT), false).clicked() {
                    leave(app);
                }
            },
        );
    });
}

/// The party's toggles: AutoPlay for anyone unlocked, the host's control lock.
fn draw_party_controls(app: &mut App, ui: &mut Ui, state: &party::State, host: bool, locked: bool) {
    let mut autoplay = state.playback.autoplay_enabled;
    let mut host_only = state.host_only_control;
    settings_card(ui, |ui| {
        setting_row(
            ui,
            "AutoPlay",
            Some("Keep the party going with similar music when the queue runs out."),
            |ui| {
                if locked {
                    dim(ui, if autoplay { "On" } else { "Off" });
                } else if toggle(ui, &mut autoplay, "AutoPlay").changed()
                    && let Some(conn) = app.party.conn.as_ref()
                {
                    conn.set_autoplay(autoplay);
                }
            },
        );
        if host {
            divider(ui);
            setting_row(
                ui,
                "Only I control the music",
                Some("Others can listen but not play, pause, skip or change the queue."),
                |ui| {
                    if toggle(ui, &mut host_only, "host-only").changed()
                        && let Some(conn) = app.party.conn.as_ref()
                    {
                        conn.set_host_only_control(host_only);
                    }
                },
            );
        }
    });
}

/// A small remove button drawn at a row's right end.
fn pill_at(ui: &mut Ui, row: egui::Rect, label: &str) -> bool {
    let size = vec2(84.0, 26.0);
    let rect = egui::Rect::from_min_size(
        pos2(row.right() - size.x - 24.0, row.center().y - size.y / 2.0),
        size,
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    pill(&mut child, label, None, false).clicked()
}
