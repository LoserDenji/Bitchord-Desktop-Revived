//! BitChord Core - Domain Models and Traits
//!
//! This crate contains the pure domain layer with no external dependencies
//! except async runtime primitives. It follows the Hexagonal Architecture pattern.

pub mod album;
pub mod artist;
pub mod error;
pub mod formats;
pub mod library;
pub mod music_library;
pub mod player;
pub mod playback_source;
pub mod playlist;
pub mod queue;
pub mod settings;
pub mod track;
pub mod traits;

pub use error::{Error, Result};
pub use formats::{ContainerFormat, AudioFormat};
pub use library::{Library, LibraryStats};
pub use player::{PlaybackSpeed, RepeatMode, ShuffleMode};
pub use playback_source::{PlaybackSource, SeekableRange};
pub use queue::PlaybackQueue;
pub use settings::UserSettings;
pub use track::Track;
pub use album::Album;
pub use artist::Artist;

/// Re-export common types for ergonomics
pub type TrackId = uuid::Uuid;
pub type AlbumId = uuid::Uuid;
pub type ArtistId = uuid::Uuid;
pub type PlaylistId = uuid::Uuid;
pub type QueuePosition = usize;