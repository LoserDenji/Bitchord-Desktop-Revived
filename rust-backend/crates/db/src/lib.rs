//! Database layer — SQLite persistence with sqlx
use sqlx::SqlitePool;
use anyhow::Result;
use uuid::Uuid;

/// SQLite database repository (async, sqlx-based)
pub struct DbRepository {
    pool: SqlitePool,
}

impl DbRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn init(&self) -> Result<()> {
        // Create tracks table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS tracks (
                id TEXT PRIMARY KEY,
                video_id TEXT UNIQUE,
                title TEXT NOT NULL,
                artist TEXT NOT NULL,
                thumbnail_url TEXT,
                duration_ms INTEGER NOT NULL DEFAULT 0,
                album_id TEXT,
                album_name TEXT,
                artist_id TEXT,
                artist_ids TEXT,
                is_video INTEGER NOT NULL DEFAULT 0,
                is_explicit INTEGER DEFAULT NULL,
                source_quality TEXT,
                source TEXT,
                local_uri TEXT,
                local_path TEXT,
                file_path TEXT,
                file_size_bytes INTEGER DEFAULT NULL,
                bitrate INTEGER DEFAULT NULL,
                sample_rate INTEGER DEFAULT NULL,
                channels INTEGER DEFAULT NULL,
                genres TEXT,
                composers TEXT,
                copyright TEXT,
                track_number INTEGER DEFAULT NULL,
                disc_number INTEGER DEFAULT NULL,
                album_total_tracks INTEGER DEFAULT NULL,
                date_added_seconds INTEGER DEFAULT NULL,
                date_modified_seconds INTEGER DEFAULT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                play_count INTEGER NOT NULL DEFAULT 0,
                skip_count INTEGER NOT NULL DEFAULT 0,
                last_played TEXT DEFAULT NULL,
                rating INTEGER DEFAULT NULL,
                lyrics TEXT,
                release_date TEXT DEFAULT NULL
            )
            "#
        ).execute(&self.pool).await?;

        // Search indexes on tracks
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_tracks_artist ON tracks(artist)")
            .execute(&self.pool).await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_tracks_album_id ON tracks(album_id)")
            .execute(&self.pool).await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_tracks_title ON tracks(title)")
            .execute(&self.pool).await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_tracks_video_id ON tracks(video_id)")
            .execute(&self.pool).await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_tracks_local_path ON tracks(local_path)")
            .execute(&self.pool).await?;

        // Play history table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS play_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                track_id TEXT NOT NULL,
                played_at TEXT NOT NULL,
                position_ms INTEGER DEFAULT NULL,
                duration_ms INTEGER DEFAULT NULL
            )
            "#
        ).execute(&self.pool).await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_play_history_track ON play_history(track_id)")
            .execute(&self.pool).await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_play_history_played ON play_history(played_at)")
            .execute(&self.pool).await?;
        
        // Artists table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS artists (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                sort_name TEXT DEFAULT NULL,
                artist_type TEXT NOT NULL DEFAULT 'Artist',
                genres TEXT,
                biography TEXT,
                art_path TEXT DEFAULT NULL,
                url TEXT DEFAULT NULL,
                mbid TEXT DEFAULT NULL,
                track_count INTEGER NOT NULL DEFAULT 0,
                album_count INTEGER NOT NULL DEFAULT 0,
                subscriber_count INTEGER DEFAULT NULL,
                monthly_listener_count INTEGER DEFAULT NULL,
                is_subscribed INTEGER DEFAULT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )
            "#
        ).execute(&self.pool).await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_artists_name ON artists(name)")
            .execute(&self.pool).await?;

        // Albums table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS albums (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                artist_id TEXT,
                artist_ids TEXT,
                release_date TEXT DEFAULT NULL,
                cover_path TEXT DEFAULT NULL,
                total_tracks INTEGER NOT NULL DEFAULT 0,
                total_discs INTEGER NOT NULL DEFAULT 1,
                genres TEXT,
                compilation INTEGER NOT NULL DEFAULT 0,
                label TEXT DEFAULT NULL,
                copyright_text TEXT DEFAULT NULL,
                record_label TEXT DEFAULT NULL,
                disc_count INTEGER DEFAULT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )
            "#
        ).execute(&self.pool).await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_albums_title ON albums(title)")
            .execute(&self.pool).await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_albums_artist_id ON albums(artist_id)")
            .execute(&self.pool).await?;

        // Playlists table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS playlists (
                id TEXT PRIMARY KEY,
                browse_id TEXT UNIQUE,
                name TEXT NOT NULL,
                description TEXT DEFAULT NULL,
                cover_path TEXT DEFAULT NULL,
                source TEXT NOT NULL DEFAULT 'Local',
                collaborative INTEGER NOT NULL DEFAULT 0,
                is_explicit INTEGER DEFAULT NULL,
                track_count INTEGER NOT NULL DEFAULT 0,
                duration_ms INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )
            "#
        ).execute(&self.pool).await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_playlists_name ON playlists(name)")
            .execute(&self.pool).await?;

        // Playlist tracks junction
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS playlist_tracks (
                playlist_id TEXT NOT NULL,
                track_id TEXT NOT NULL,
                position INTEGER NOT NULL,
                added_at TEXT NOT NULL,
                PRIMARY KEY (playlist_id, track_id)
            )
            "#
        ).execute(&self.pool).await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_playlist_tracks_playlist ON playlist_tracks(playlist_id)")
            .execute(&self.pool).await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_playlist_tracks_track ON playlist_tracks(track_id)")
            .execute(&self.pool).await?;

        // Track artists junction
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS track_artists (
                track_id TEXT NOT NULL,
                artist_id TEXT NOT NULL,
                role TEXT NOT NULL DEFAULT 'primary',
                PRIMARY KEY (track_id, artist_id, role)
            )
            "#
        ).execute(&self.pool).await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_track_artists_track ON track_artists(track_id)")
            .execute(&self.pool).await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_track_artists_artist ON track_artists(artist_id)")
            .execute(&self.pool).await?;

        // Album artists junction
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS album_artists (
                album_id TEXT NOT NULL,
                artist_id TEXT NOT NULL,
                PRIMARY KEY (album_id, artist_id)
            )
            "#
        ).execute(&self.pool).await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_album_artists_album ON album_artists(album_id)")
            .execute(&self.pool).await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_album_artists_artist ON album_artists(artist_id)")
            .execute(&self.pool).await?;

        // Library folders
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS library_folders (
                path TEXT PRIMARY KEY,
                label TEXT NOT NULL,
                is_active INTEGER NOT NULL DEFAULT 1,
                last_scanned TEXT DEFAULT NULL,
                scan_error TEXT DEFAULT NULL,
                created_at TEXT NOT NULL
            )
            "#
        ).execute(&self.pool).await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_library_folders_active ON library_folders(is_active)")
            .execute(&self.pool).await?;

        // Sync state
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS sync_state (
                id TEXT PRIMARY KEY,
                entity_type TEXT NOT NULL,
                entity_id TEXT NOT NULL,
                sync_hash TEXT NOT NULL,
                last_synced TEXT NOT NULL,
                UNIQUE(entity_type, entity_id)
            )
            "#
        ).execute(&self.pool).await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_sync_state_entity ON sync_state(entity_type, entity_id)")
            .execute(&self.pool).await?;

        Ok(())
    }

    /// Get a track by ID
    pub async fn get_track(&self, id: &Uuid) -> Result<Option<TrackDbRecord>> {
        let row = sqlx::query_as::<_, TrackDbRow>(
            "SELECT id, video_id, title, artist, thumbnail_url, duration_ms,                    album_id, album_name, artist_id, artist_ids, is_video,                    is_explicit, source_quality, source, local_uri, local_path,                    file_path, file_size_bytes, bitrate, sample_rate, channels,                    genres, composers, copyright, track_number, disc_number,                    album_total_tracks, date_added_seconds, date_modified_seconds,                    play_count, skip_count, last_played, rating, lyrics,                    release_date, created_at, updated_at             FROM tracks WHERE id = ?"
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| r.into()))
    }

    /// Create a new track
    pub async fn create_track(&self, track: &TrackDbRecord) -> Result<()> {
        sqlx::query(
            "INSERT INTO tracks (id, video_id, title, artist, thumbnail_url, duration_ms,                    album_id, album_name, artist_id, artist_ids, is_video,                    is_explicit, source_quality, source, local_uri, local_path,                    file_path, file_size_bytes, bitrate, sample_rate, channels,                    genres, composers, copyright, track_number, disc_number,                    album_total_tracks, date_added_seconds, date_modified_seconds,                    play_count, skip_count, last_played, rating, lyrics,                    release_date, created_at, updated_at)             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31, ?32, ?33, ?34, ?35, ?36, ?37)"
        )
        .bind(track.id.to_string())
        .bind(track.video_id)
        .bind(&track.title)
        .bind(&track.artist)
        .bind(track.thumbnail_url)
        .bind(track.duration_ms)
        .bind(track.album_id)
        .bind(&track.album_name)
        .bind(track.artist_id)
        .bind(track.artist_ids)
        .bind(if track.is_video { 1i32 } else { 0i32 })
        .bind(track.is_explicit.map(|v| v as i32))
        .bind(track.source_quality)
        .bind(track.source)
        .bind(track.local_uri)
        .bind(track.local_path)
        .bind(track.file_path)
        .bind(track.file_size_bytes)
        .bind(track.bitrate)
        .bind(track.sample_rate)
        .bind(track.channels)
        .bind(track.genres)
        .bind(track.composers)
        .bind(track.copyright)
        .bind(track.track_number)
        .bind(track.disc_number)
        .bind(track.album_total_tracks)
        .bind(track.date_added_seconds)
        .bind(track.date_modified_seconds)
        .bind(track.play_count)
        .bind(track.skip_count)
        .bind(track.last_played)
        .bind(track.rating)
        .bind(track.lyrics)
        .bind(track.release_date)
        .bind(track.created_at)
        .bind(track.updated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Update an existing track
    pub async fn update_track(&self, track: &TrackDbRecord) -> Result<()> {
        sqlx::query(
            "UPDATE tracks SET video_id = ?2, title = ?3, artist = ?4, thumbnail_url = ?5,                    duration_ms = ?6, album_id = ?7, album_name = ?8, artist_id = ?9,                    artist_ids = ?10, is_video = ?11, is_explicit = ?12,                    source_quality = ?13, source = ?14, local_uri = ?15, local_path = ?16,                    file_path = ?17, file_size_bytes = ?18, bitrate = ?19, sample_rate = ?20,                    channels = ?21, genres = ?22, composers = ?23, copyright = ?24,                    track_number = ?25, disc_number = ?26, album_total_tracks = ?27,                    date_added_seconds = ?28, date_modified_seconds = ?29,                    play_count = ?30, skip_count = ?31, last_played = ?32, rating = ?33,                    lyrics = ?34, release_date = ?35, created_at = ?36, updated_at = ?37             WHERE id = ?1"
        )
        .bind(track.id.to_string())
        .bind(track.video_id)
        .bind(&track.title)
        .bind(&track.artist)
        .bind(track.thumbnail_url)
        .bind(track.duration_ms)
        .bind(track.album_id)
        .bind(&track.album_name)
        .bind(track.artist_id)
        .bind(track.artist_ids)
        .bind(if track.is_video { 1i32 } else { 0i32 })
        .bind(track.is_explicit.map(|v| v as i32))
        .bind(track.source_quality)
        .bind(track.source)
        .bind(track.local_uri)
        .bind(track.local_path)
        .bind(track.file_path)
        .bind(track.file_size_bytes)
        .bind(track.bitrate)
        .bind(track.sample_rate)
        .bind(track.channels)
        .bind(track.genres)
        .bind(track.composers)
        .bind(track.copyright)
        .bind(track.track_number)
        .bind(track.disc_number)
        .bind(track.album_total_tracks)
        .bind(track.date_added_seconds)
        .bind(track.date_modified_seconds)
        .bind(track.play_count)
        .bind(track.skip_count)
        .bind(track.last_played)
        .bind(track.rating)
        .bind(track.lyrics)
        .bind(track.release_date)
        .bind(track.created_at)
        .bind(track.updated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Delete a track by ID
    pub async fn delete_track(&self, id: &Uuid) -> Result<()> {
        sqlx::query("DELETE FROM tracks WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Get all tracks
    pub async fn get_all_tracks(&self) -> Result<Vec<TrackDbRecord>> {
        let rows = sqlx::query_as::<_, TrackDbRow>(
            "SELECT id, video_id, title, artist, thumbnail_url, duration_ms,                    album_id, album_name, artist_id, artist_ids, is_video,                    is_explicit, source_quality, source, local_uri, local_path,                    file_path, file_size_bytes, bitrate, sample_rate, channels,                    genres, composers, copyright, track_number, disc_number,                    album_total_tracks, date_added_seconds, date_modified_seconds,                    play_count, skip_count, last_played, rating, lyrics,                    release_date, created_at, updated_at             FROM tracks ORDER BY title ASC"
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|r| r.into()).collect())
    }

    /// Search tracks by query string (full-text search on title, artist, album)
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<TrackDbRecord>> {
        let search_term = format!("%{}%", query.to_lowercase());
        let rows = sqlx::query_as::<_, TrackDbRow>(
            "SELECT id, video_id, title, artist, thumbnail_url, duration_ms,                    album_id, album_name, artist_id, artist_ids, is_video,                    is_explicit, source_quality, source, local_uri, local_path,                    file_path, file_size_bytes, bitrate, sample_rate, channels,                    genres, composers, copyright, track_number, disc_number,                    album_total_tracks, date_added_seconds, date_modified_seconds,                    play_count, skip_count, last_played, rating, lyrics,                    release_date, created_at, updated_at             FROM tracks             WHERE LOWER(title) LIKE ?1 OR LOWER(artist) LIKE ?1 OR LOWER(album_name) LIKE ?1             ORDER BY play_count DESC, last_played DESC             LIMIT ?2"
        )
        .bind(&search_term)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|r| r.into()).collect())
    }
}

// === Records ===
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TrackDbRecord {
    pub id: Uuid,
    pub video_id: Option<String>,
    pub title: String,
    pub artist: String,
    pub thumbnail_url: Option<String>,
    pub duration_ms: i64,
    pub album_id: Option<String>,
    pub album_name: Option<String>,
    pub artist_id: Option<String>,
    pub artist_ids: Option<String>,
    pub is_video: bool,
    pub is_explicit: Option<bool>,
    pub source_quality: Option<String>,
    pub source: Option<String>,
    pub local_uri: Option<String>,
    pub local_path: Option<String>,
    pub file_path: Option<String>,
    pub file_size_bytes: Option<i64>,
    pub bitrate: Option<i32>,
    pub sample_rate: Option<i32>,
    pub channels: Option<i16>,
    pub genres: Option<String>,
    pub composers: Option<String>,
    pub copyright: Option<String>,
    pub track_number: Option<i32>,
    pub disc_number: Option<i32>,
    pub album_total_tracks: Option<i32>,
    pub date_added_seconds: Option<i64>,
    pub date_modified_seconds: Option<i64>,
    pub play_count: i32,
    pub skip_count: i32,
    pub last_played: Option<String>,
    pub rating: Option<i32>,
    pub lyrics: Option<String>,
    pub release_date: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(sqlx::FromRow)]
struct TrackDbRow {
    id: String,
    video_id: Option<String>,
    title: String,
    artist: String,
    thumbnail_url: Option<String>,
    duration_ms: i64,
    album_id: Option<String>,
    album_name: Option<String>,
    artist_id: Option<String>,
    artist_ids: Option<String>,
    is_video: i32,
    is_explicit: Option<i32>,
    source_quality: Option<String>,
    source: Option<String>,
    local_uri: Option<String>,
    local_path: Option<String>,
    file_path: Option<String>,
    file_size_bytes: Option<i64>,
    bitrate: Option<i32>,
    sample_rate: Option<i32>,
    channels: Option<i16>,
    genres: Option<String>,
    composers: Option<String>,
    copyright: Option<String>,
    track_number: Option<i32>,
    disc_number: Option<i32>,
    album_total_tracks: Option<i32>,
    date_added_seconds: Option<i64>,
    date_modified_seconds: Option<i64>,
    play_count: i32,
    skip_count: i32,
    last_played: Option<String>,
    rating: Option<i32>,
    lyrics: Option<String>,
    release_date: Option<String>,
    created_at: String,
    updated_at: String,
}

impl From<TrackDbRow> for TrackDbRecord {
    fn from(row: TrackDbRow) -> Self {
        Self {
            id: Uuid::parse_str(&row.id).unwrap_or_else(|_| uuid::Uuid::nil()),
            video_id: row.video_id,
            title: row.title,
            artist: row.artist,
            thumbnail_url: row.thumbnail_url,
            duration_ms: row.duration_ms,
            album_id: row.album_id,
            album_name: row.album_name,
            artist_id: row.artist_id,
            artist_ids: row.artist_ids,
            is_video: row.is_video != 0,
            is_explicit: row.is_explicit.map(|v| v != 0),
            source_quality: row.source_quality,
            source: row.source,
            local_uri: row.local_uri,
            local_path: row.local_path,
            file_path: row.file_path,
            file_size_bytes: row.file_size_bytes,
            bitrate: row.bitrate,
            sample_rate: row.sample_rate,
            channels: row.channels,
            genres: row.genres,
            composers: row.composers,
            copyright: row.copyright,
            track_number: row.track_number,
            disc_number: row.disc_number,
            album_total_tracks: row.album_total_tracks,
            date_added_seconds: row.date_added_seconds,
            date_modified_seconds: row.date_modified_seconds,
            play_count: row.play_count,
            skip_count: row.skip_count,
            last_played: row.last_played,
            rating: row.rating,
            lyrics: row.lyrics,
            release_date: row.release_date,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}
