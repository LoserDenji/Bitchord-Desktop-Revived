//! Home feed and shelf models — ported from Kotlin `HomeShelf`, `MoodGenre`, `HomePage`, `LibraryPage`.
use serde::{Deserialize, Serialize};

use crate::album::BrowseItem;
use crate::song::Song;

/// A card in a home-feed carousel: either a track (videoId) or an album/playlist (browseId).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShelfItem {
    pub title: String,
    pub subtitle: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browse_id: Option<String>,
}

/// One server-defined group of the buttons shown on Explore.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MoodGenreSection {
    pub title: String,
    pub items: Vec<MoodGenre>,
}

/// A mood or genre button and the exact browse request that it represents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MoodGenre {
    pub title: String,
    pub browse_id: String,
    pub params: Option<String>,
    /// First real cover from the category's playlist shelves, loaded in the background.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail_url: Option<String>,
    /// YouTube Music's own stripe colour for the button, as ARGB; null when the button carries none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stripe_color: Option<u32>,
}

/// A home feed page, plus the token for the next one — null once exhausted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HomeFeed {
    pub shelves: Vec<HomeShelf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation: Option<String>,
}

/// A card in a home-feed carousel: either a track (videoId) or an album/playlist (browseId).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HomeShelf {
    pub title: String,
    pub items: Vec<ShelfItem>,
    /// YouTube's "strapline" — the grey line Apple Music runs under a heading.
    #[serde(default)]
    pub subtitle: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub more_browse_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub more_params: Option<String>,
    /// The source's own cell kind when it names one — Apple's `itemKind`.
    #[serde(default)]
    pub layout: String,
    /// How many rows the source itself lays this shelf out in.
    #[serde(default)]
    pub rows: u32,
}

impl HomeShelf {
    /// Filter out shelves that mostly repeat another.
    pub fn without_repeats_of(existing: &[HomeShelf], shelves: Vec<HomeShelf>) -> Vec<HomeShelf> {
        const REPEAT_SHELF_SHARE: f64 = 0.6;
        let mut shown = existing.to_vec();
        shelves
            .into_iter()
            .filter(|shelf| {
                let keys: std::collections::HashSet<_> = shelf.items.iter().filter_map(|i| i.browse_id.as_ref()).collect();
                let repeat = !keys.is_empty() && shown.iter().any(|other| {
                    let other_keys: std::collections::HashSet<_> = other.items.iter().filter_map(|i| i.browse_id.as_ref()).collect();
                    let shared = keys.intersection(&other_keys).count();
                    shared > 0 && shared >= (REPEAT_SHELF_SHARE * (shelf.items.len().min(other.items.len())) as f64) as usize
                });
                if !repeat {
                    shown.push(shelf);
                }
                !repeat
            })
            .collect()
    }
}

/// The signed-in library, as YouTube Music splits it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryPage {
    pub liked_songs: Vec<Song>,
    pub library_songs: Vec<Song>,
    pub shelves: Vec<HomeShelf>,
    /// Continuation token for liked_songs' next page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub liked_continuation: Option<String>,
}

impl LibraryPage {
    pub fn is_empty(&self) -> bool {
        self.liked_songs.is_empty() && self.library_songs.is_empty() && self.shelves.is_empty()
    }
}

/// The signed-in Google account, as YouTube Music reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub name: String,
    pub email: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail_url: Option<String>,
}

/// One identity the signed-in session can act as.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountChannel {
    pub name: String,
    pub subtitle: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_sync_id: Option<String>,
    /// Whether YouTube's own switcher marks this as the session's active one.
    pub active_on_web: bool,
}

impl AccountChannel {
    pub fn key(&self) -> String {
        self.page_id.as_ref().or(self.data_sync_id.as_ref()).cloned().unwrap_or_else(|| self.name.clone())
    }
}
