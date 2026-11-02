//! Song (Track) domain model — faithful port of Kotlin `Song` / `ArtistRef` / `QueueTier`.
use serde::{Deserialize, Serialize};

use crate::playback_source::PlaybackSourceType;

/// Tier of an item in the playback queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum QueueTier {
    /// Explicitly queued by user ("Play Next", "Add to Queue", or preserved user queue).
    #[serde(rename = "USER_QUEUE")]
    UserQueue,
    /// Part of the active Album, Playlist, or Artist collection context.
    #[default]
    #[serde(rename = "CONTEXT")]
    Context,
    /// Dynamically generated radio recommendations appended when context/user queue ends.
    #[serde(rename = "AUTOPLAY")]
    Autoplay,
}

/// One credited artist, as YouTube Music itself states them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtistRef {
    pub name: String,
    /// The artist's own channel browse id. Null when the row names an artist without linking one.
    #[serde(rename = "browseId")]
    pub browse_id: Option<String>,
}

/// A playable YouTube Music track.
///
/// Field names use `camelCase` via serde to match the Kotlin wire/shared format exactly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Song {
    pub video_id: String,
    pub title: String,
    pub artist: String,
    pub thumbnail_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_text: Option<String>,
    /// Browse ids lifted from the row, used by long-press actions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artist_id: Option<String>,
    /// Every credited artist, each with their own channel.
    #[serde(default)]
    pub artists: Vec<ArtistRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album_id: Option<String>,
    /// Names the album page header, which album_id alone can't.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album_name: Option<String>,
    /// A music-video upload rather than the catalogue track.
    #[serde(default)]
    pub is_video: bool,
    /// True for a video upload and for its manually selected catalogue match.
    #[serde(default)]
    pub is_video_origin: bool,
    /// This track's identity within one playlist (not its video_id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set_video_id: Option<String>,
    /// Which queue tier this track belongs to in the playback timeline.
    #[serde(default)]
    pub queue_tier: QueueTier,
    /// Unique, strictly immutable queue-entry identity assigned when entering the player's queue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_entry_id: Option<String>,
    /// The seed title of an explicitly started radio queue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radio_name: Option<String>,
    /// Explicit content or file URI for local device tracks or downloaded audio.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_uri: Option<String>,
    /// A premium rendition marker recorded by BitChord for its Downloads list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download_format: Option<String>,
    /// Real filesystem path backing local_uri.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_path: Option<String>,
    /// MediaStore timestamps used only to sort device and downloaded libraries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_date_added_seconds: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_date_modified_seconds: Option<i64>,
    /// What a non-YouTube source says it can serve: LOSSLESS, HIGH, LOW.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_quality: Option<String>,
    /// Explicit-content state from the catalogue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_explicit: Option<bool>,
    /// The page, collection, or feed section that put this track in the queue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playback_source: Option<String>,
    /// Where tapping playback_source should return in the app.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playback_source_type: Option<PlaybackSourceType>,
    /// Browse id for an album, playlist, or other source page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playback_source_id: Option<String>,
}

impl Song {
    /// Legacy derived property: true if and only if queue_tier is Autoplay.
    pub fn from_autoplay(&self) -> bool {
        self.queue_tier == QueueTier::Autoplay
    }

    /// Whether a row is the track the player is on, for the now-playing highlight.
    /// Title and credit, matched exactly, and nothing else.
    pub fn is_same_track_as(&self, other: &Song) -> bool {
        self.title == other.title && self.artist == other.artist
    }

    /// Duration text in milliseconds, or 0 when the row didn't state one.
    pub fn duration_millis(&self) -> u64 {
        parse_duration_millis(self.duration_text.as_deref())
    }

    /// Artwork at a given pixel size.
    pub fn artwork_at(&self, px: u32) -> Option<String> {
        artwork_at(self.thumbnail_url.as_deref(), px)
    }
}

/// Parse a `M:SS` or `H:MM:SS` string into milliseconds. Zero for anything that isn't a duration.
pub fn parse_duration_millis(text: Option<&str>) -> u64 {
    let trimmed = match text.map(|s| s.trim()).filter(|s| !s.is_empty()) {
        Some(s) => s,
        None => return 0,
    };
    let parts: Vec<&str> = trimmed.split(':').collect();
    let numbers: Vec<u64> = parts.iter().filter_map(|p| p.trim().parse().ok()).collect();
    if numbers.len() != parts.len() {
        return 0; // some part wasn't a number
    }
    let seconds = match numbers.len() {
        2 => numbers[0] * 60 + numbers[1],
        3 => numbers[0] * 3600 + numbers[1] * 60 + numbers[2],
        _ => return 0,
    };
    seconds.saturating_mul(1000)
}

/// Replace YouTube's `w<N>-h<N>` size hint with the requested pixel size.
pub fn artwork_at(url: Option<&str>, px: u32) -> Option<String> {
    let u = url?;
    let re = regex_lite::Regex::new(r"w\d+-h\d+").ok()?;
    let replacement = format!("w{px}-h{px}");
    Some(re.replace(u, replacement.as_str()).into_owned())
}

/// Artwork for a list row — 52dp at most, ~140px on 3x screen.
pub const ROW_ART_PX: u32 = 160;
/// Artwork for a shelf card: 166dp wide.
pub const CARD_ART_PX: u32 = 480;
/// Artwork for a page header.
pub const HEADER_ART_PX: u32 = 720;
/// Artwork handed to the media session.
pub const NOTIFICATION_ART_PX: u32 = 544;
/// Artwork for the full player.
pub const PLAYER_ART_PX: u32 = 1200;
