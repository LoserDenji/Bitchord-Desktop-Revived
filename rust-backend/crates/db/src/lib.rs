//! Database layer
use crate::bitchord_core::{Track, Album, Artist, Playlist};
use sqlx::SqlitePool;
use anyhow::Result;

/// SQLite database repository
pub struct DbRepository {
    pool: SqlitePool,
}

impl DbRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
    
    pub async fn init(&self) -> Result<()> {
        // Create tables
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS tracks (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                duration_ms INTEGER NOT NULL,
                file_path TEXT NOT NULL,
                added_at TEXT NOT NULL,
                last_played TEXT,
                play_count INTEGER DEFAULT 0,
                skip_count INTEGER DEFAULT 0
            )
            "#
        ).execute(&self.pool).await?;
        
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS playlists (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )
            "#
        ).execute(&self.pool).await?;
        
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS playlist_tracks (
                playlist_id TEXT NOT NULL,
                track_id TEXT NOT NULL,
                position INTEGER NOT NULL,
                PRIMARY KEY (playlist_id, track_id)
            )
            "#
        ).execute(&self.pool).await?;
        
        Ok(())
    }
}

/// In-memory repository implementation (for testing)
pub struct MemoryRepository;

impl MemoryRepository {
    pub fn new() -> Self {
        Self
    }
}