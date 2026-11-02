//! Album-related domain models — ported from Kotlin `BrowseItem`, `DetailPage`, `LibraryState`, `Album`, `ArtistPage`.
use serde::{Deserialize, Serialize};

use crate::song::Song;
use crate::ui_state::UiState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BrowseType {
    #[serde(rename = "ALBUM")]
    Album,
    #[serde(rename = "ARTIST")]
    Artist,
    #[serde(rename = "PLAYLIST")]
    Playlist,
    #[serde(rename = "OTHER")]
    Other,
}

/// A non-track search result: album, artist or playlist.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowseItem {
    pub browse_id: String,
    pub title: String,
    pub subtitle: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail_url: Option<String>,
    #[serde(rename = "type")]
    pub browse_type: BrowseType,
}

/// Whether an album or playlist is in the library, and the id that changes that.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryState {
    pub playlist_id: String,
    pub saved: bool,
}

/// Whether an artist's channel is subscribed to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionState {
    pub channel_id: String,
    pub subscribed: bool,
}

/// The type of album release
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AlbumReleaseType {
    Album,
    Single,
    EP,
    Compilation,
    Mixtape,
}

/// An album page with all its tracks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Album {
    pub browse_id: String,
    pub title: String,
    pub subtitle: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail_url: Option<String>,
    #[serde(default)]
    pub release_type: AlbumReleaseType,
    #[serde(default)]
    pub sections: Vec<crate::home::HomeShelf>,
    #[serde(default)]
    pub suggested_songs: Vec<Song>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub library: Option<LibraryState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscriber_count_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monthly_listener_count: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription: Option<SubscriptionState>,
}

impl Default for Album {
    fn default() -> Self {
        Album {
            browse_id: String::new(),
            title: String::new(),
            subtitle: String::new(),
            thumbnail_url: None,
            release_type: AlbumReleaseType::Album,
            sections: Vec::new(),
            suggested_songs: Vec::new(),
            library: None,
            description: None,
            subscriber_count_text: None,
            monthly_listener_count: None,
            subscription: None,
        }
    }
}

/// A browsed album page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumPage {
    pub browse_id: String,
    pub title: String,
    pub subtitle: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail_url: Option<String>,
    #[serde(default)]
    pub sections: Vec<crate::home::HomeShelf>,
    #[serde(default)]
    pub suggested_songs: Vec<Song>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub library: Option<LibraryState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscriber_count_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monthly_listener_count: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription: Option<SubscriptionState>,
}

/// An artist page with their profile and songs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtistPage {
    pub songs: Vec<Song>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub more_songs_browse_id: Option<String>,
    pub sections: Vec<crate::home::HomeShelf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscriber_count_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monthly_listener_count: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription: Option<SubscriptionState>,
}

/// A browsed album / artist / playlist page (generic).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetailPage {
    pub browse_id: String,
    pub title: String,
    pub subtitle: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail_url: Option<String>,
    pub songs: UiState<Vec<Song>>,
    #[serde(default)]
    pub browse_type: BrowseType,
    #[serde(default)]
    pub sections: Vec<crate::home::HomeShelf>,
    #[serde(default)]
    pub suggested_songs: Vec<Song>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub library: Option<LibraryState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscriber_count_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monthly_listener_count: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription: Option<SubscriptionState>,
}

impl Default for BrowseType {
    fn default() -> Self {
        BrowseType::Other
    }
}
