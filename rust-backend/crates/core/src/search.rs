//! Search-related domain models — ported from Kotlin search models.
use serde::{Deserialize, Serialize};

/// Entity type for search history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EntityType {
    Track,
    Album,
    Artist,
    Playlist,
}

/// An entity the user tapped in search results — track, album, artist, or playlist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHistoryEntity {
    /// Unique id: videoId for tracks, browseId for albums/artists/playlists.
    pub id: String,
    /// Display title — song name, album name, artist name, etc.
    pub title: String,
    /// Subtitle — e.g. "Song • Drake", "Album • Artist", or just the type label.
    pub subtitle: String,
    /// Artwork URL for the cover/thumbnail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artwork_url: Option<String>,
    /// The kind of entity this represents.
    pub entity_type: EntityType,
    /// Timestamp (epoch millis) — updated on each re-tap to bubble it to the top.
    #[serde(default = "current_time_millis")]
    pub timestamp: i64,
}

fn current_time_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Search filter options.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SearchFilter {
    /// YouTube Music's mixed search page: songs, artists, albums and playlists.
    All,
    Songs,
    Videos,
    Albums,
    Artists,
    Playlists,
}

impl SearchFilter {
    pub fn label(&self) -> &'static str {
        match self {
            SearchFilter::All => "All",
            SearchFilter::Songs => "Songs",
            SearchFilter::Videos => "Videos",
            SearchFilter::Albums => "Albums",
            SearchFilter::Artists => "Artists",
            SearchFilter::Playlists => "Playlists",
        }
    }
}

/// Like status for tracks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LikeStatus {
    Like,
    Dislike,
    Indifferent,
}

/// Playlist privacy levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PlaylistPrivacy {
    Private,
    Unlisted,
    Public,
}
