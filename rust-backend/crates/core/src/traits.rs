//! Core traits for repository pattern
use async_trait::async_trait;
use std::path::PathBuf;
use uuid::Uuid;

use super::track::Track;
use super::album::Album;
use super::artist::Artist;
use super::playlist::Playlist;

#[async_trait]
pub trait TrackRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Option<Track>;
    async fn get_by_ids(&self, ids: &[Uuid]) -> Vec<Track>;
    async fn find_by_path(&self, path: &PathBuf) -> Option<Track>;
    async fn save(&self, track: &mut Track) -> anyhow::Result<()>;
    async fn delete(&self, id: Uuid) -> bool;
    async fn search(&self, query: &str, limit: usize) -> Vec<Track>;
    async fn count(&self) -> usize;
    async fn all(&self, offset: usize, limit: usize) -> Vec<Track>;
}

#[async_trait]
pub trait AlbumRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Option<Album>;
    async fn save(&self, album: &mut Album) -> anyhow::Result<()>;
    async fn delete(&self, id: Uuid) -> bool;
    async fn search(&self, query: &str, limit: usize) -> Vec<Album>;
    async fn all(&self, offset: usize, limit: usize) -> Vec<Album>;
}

#[async_trait]
pub trait ArtistRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Option<Artist>;
    async fn save(&self, artist: &mut Artist) -> anyhow::Result<()>;
    async fn delete(&self, id: Uuid) -> bool;
    async fn search(&self, query: &str, limit: usize) -> Vec<Artist>;
}

#[async_trait]
pub trait PlaylistRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Option<Playlist>;
    async fn create(&self, name: &str) -> anyhow::Result<Playlist>;
    async fn save(&self, playlist: &mut Playlist) -> anyhow::Result<()>;
    async fn delete(&self, id: Uuid) -> bool;
    async fn all(&self) -> Vec<Playlist>;
    
    // Playlist-track relationship
    async fn add_track(&self, playlist_id: Uuid, track_id: Uuid, position: Option<usize>) -> anyhow::Result<()>;
    async fn remove_track(&self, playlist_id: Uuid, track_id: Uuid) -> anyhow::Result<()>;
    async fn move_track(&self, playlist_id: Uuid, track_id: Uuid, new_position: usize) -> anyhow::Result<()>;
    async fn tracks(&self, playlist_id: Uuid, offset: usize, limit: usize) -> Vec<Track>;
    async fn track_count(&self, playlist_id: Uuid) -> usize;
}