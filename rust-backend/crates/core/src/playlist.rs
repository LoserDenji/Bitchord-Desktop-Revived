//! Playlist representation
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Playlist {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub track_ids: Vec<Uuid>,
    pub cover_path: Option<std::path::PathBuf>,
    pub collaborative: bool,
    pub source: PlaylistSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlaylistSource {
    Local,
    Spotify,
    AppleMusic,
    Youtube,
    Other(String),
}

impl Default for PlaylistSource {
    fn default() -> Self {
        PlaylistSource::Local
    }
}

impl Playlist {
    pub fn new(name: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            description: None,
            created_at: now,
            updated_at: now,
            track_ids: Vec::new(),
            cover_path: None,
            collaborative: false,
            source: PlaylistSource::Local,
        }
    }

    pub fn set_name(&mut self, name: String) {
        self.name = name;
        self.updated_at = Utc::now();
    }

    pub fn rename(&mut self, name: &str) {
        self.name = name.to_string();
        self.updated_at = Utc::now();
    }

    pub fn move_track(&mut self, track_id: Uuid, new_position: usize) {
        if let Some(pos) = self.track_ids.iter().position(|id| *id == track_id) {
            let item = self.track_ids.remove(pos);
            self.track_ids.insert(new_position.min(self.track_ids.len()), item);
            self.updated_at = Utc::now();
        }
    }

    pub fn remove_track(&mut self, track_id: Uuid) -> bool {
        if let Some(pos) = self.track_ids.iter().position(|id| *id == track_id) {
            self.track_ids.remove(pos);
            self.updated_at = Utc::now();
            true
        } else {
            false
        }
    }
}