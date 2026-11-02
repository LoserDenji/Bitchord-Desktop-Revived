//! BitChord Core - Domain Models and Traits
//!
//! This crate contains the pure domain layer with no external dependencies
//! except async runtime primitives. It follows the Hexagonal Architecture pattern.
//!
//! Ported from Kotlin shared module DTOs (Models.kt, PartyModels.kt, etc.)

pub mod album;
pub mod artist;
pub mod error;
pub mod formats;
pub mod home;
pub mod library;
pub mod library_ext;
pub mod music_library;
pub mod party;
pub mod player;
pub mod playback_source;
pub mod playlist;
pub mod queue;
pub mod search;
pub mod settings;
pub mod song;
pub mod track;
pub mod traits;
pub mod ui_state;

// Re-exports for convenience
pub use album::{Album, AlbumPage, BrowseItem, BrowseType, DetailPage, LibraryState, SubscriptionState};
pub use artist::{Artist, ArtistCredit, ArtistPage, CreditSegment};
pub use error::{Error, Result};
pub use formats::{ContainerFormat, AudioFormat};
pub use home::{HomeFeed, HomeShelf, MoodGenre, MoodGenreSection, ShelfItem, Account, AccountChannel, LibraryPage};
pub use library::{Library, LibraryStats};
pub use library_ext::{ScanConfig, TrackMetadata};
pub use playlist::Playlist;
pub use track::{LyricLine, Track};
pub use party::{ApiError, JamInvite, PartyActivity, PartyMember, PartyMembership, PartyPlayback, PartyPreview, PartyPreviewMember, PartyQueue, PartySnapshot, PartyTrack};
pub use player::{PlaybackSpeed, RepeatMode, ShuffleMode};
pub use playback_source::{PlaybackSource, SeekableRange};
pub use queue::PlaybackQueue;
pub use search::{EntityType, LikeStatus, SearchFilter, SearchHistoryEntity};
pub use settings::UserSettings;
pub use song::{ArtistRef, QueueTier, Song};
pub use ui_state::UiState;

// Re-export jam_invite as a module
pub use party::jam_invite;

/// Re-export common types for ergonomics
pub type VideoId = String;
pub type QueuePosition = usize;
pub type Seconds = u64;