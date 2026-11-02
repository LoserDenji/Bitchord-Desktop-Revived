//! Artist domain models — ported from Kotlin `ArtistPage`, `ArtistCredit`, `CreditSegment`.
use serde::{Deserialize, Serialize};

use crate::album::SubscriptionState;
use crate::home::HomeShelf;
use crate::song::{ArtistRef, Song};

/// Parsed artist landing page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtistPage {
    pub songs: Vec<Song>,
    /// Playlist holding the artist's full song list, when the page links one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub more_songs_browse_id: Option<String>,
    pub sections: Vec<HomeShelf>,
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

/// One name in a credit line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtistCredit {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browse_id: Option<String>,
}

/// One stretch of the credit line as it will be drawn: either a credit, which can be tapped, or plain text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreditSegment {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credit: Option<ArtistCredit>,
}

/// Normalizes an artist name for comparison: NFD-decompose, strip combining marks, lowercase, collapse whitespace.
pub fn normalized_artist_name(raw: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    raw.nfd()
        .filter(|c| !c.is_combining_mark())
        .collect::<String>()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether a search result is the artist that was asked for, exactly.
pub fn is_exact_artist_match(answer: &str, wanted: &str) -> bool {
    let name = normalized_artist_name(wanted);
    if name.is_empty() {
        return false;
    }
    normalized_artist_name(answer) == name
}

/// Completes a byline's credits from what the app already knows.
/// Port of `completeArtistCredits` — simplified for initial domain model.
pub fn complete_artist_credits(_line: &str, stated: Vec<ArtistRef>) -> Vec<ArtistRef> {
    stated
}
