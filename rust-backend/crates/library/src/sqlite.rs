//! SQLite schema and migrations for music library
//!
//! Tables are based on the shared/sync model from Kotlin BitChord.
//! Each entity (Track, Album, Artist, Playlist) has a corresponding table.

use rusqlite::{params, Connection, Result as SqlResult};
use chrono::{DateTime, Utc};
use uuid::Uuid;

/// Initialize all library tables in the database
pub fn init_schema(conn: &Connection) -> SqlResult<()> {
    conn.execute_batch(
        r#"
        -- Tracks table - core music files
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
        );

        -- Create index for video_id lookups
        CREATE INDEX IF NOT EXISTS idx_tracks_video_id ON tracks(video_id);
        
        -- Create index for artist lookups
        CREATE INDEX IF NOT EXISTS idx_tracks_artist ON tracks(artist);
        
        -- Create index for album lookups
        CREATE INDEX IF NOT EXISTS idx_tracks_album_id ON tracks(album_id);
        
        -- Create index for local path lookups
        CREATE INDEX IF NOT EXISTS idx_tracks_local_path ON tracks(local_path);

        -- Artists table
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
        );

        -- Create index for artist name lookups
        CREATE INDEX IF NOT EXISTS idx_artists_name ON artists(name);

        -- Albums table
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
        );

        -- Create index for album title lookups
        CREATE INDEX IF NOT EXISTS idx_albums_title ON albums(title);
        
        -- Create index for album artist lookups
        CREATE INDEX IF NOT EXISTS idx_albums_artist_id ON albums(artist_id);

        -- Playlists table
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
        );

        -- Create index for playlist name lookups
        CREATE INDEX IF NOT EXISTS idx_playlists_name ON playlists(name);

        -- Playlist tracks junction table (many-to-many)
        CREATE TABLE IF NOT EXISTS playlist_tracks (
            playlist_id TEXT NOT NULL,
            track_id TEXT NOT NULL,
            position INTEGER NOT NULL,
            added_at TEXT NOT NULL,
            PRIMARY KEY (playlist_id, track_id)
        );

        -- Create index for playlist track lookups
        CREATE INDEX IF NOT EXISTS idx_playlist_tracks_playlist ON playlist_tracks(playlist_id);
        CREATE INDEX IF NOT EXISTS idx_playlist_tracks_track ON playlist_tracks(track_id);

        -- Track artists junction table (many-to-many)
        CREATE TABLE IF NOT EXISTS track_artists (
            track_id TEXT NOT NULL,
            artist_id TEXT NOT NULL,
            role TEXT NOT NULL DEFAULT 'primary',
            PRIMARY KEY (track_id, artist_id, role)
        );

        -- Create index for track artist lookups
        CREATE INDEX IF NOT EXISTS idx_track_artists_track ON track_artists(track_id);
        CREATE INDEX IF NOT EXISTS idx_track_artists_artist ON track_artists(artist_id);

        -- Album artists junction table (many-to-many)
        CREATE TABLE IF NOT EXISTS album_artists (
            album_id TEXT NOT NULL,
            artist_id TEXT NOT NULL,
            PRIMARY KEY (album_id, artist_id)
        );

        -- Create index for album artist lookups
        CREATE INDEX IF NOT EXISTS idx_album_artists_album ON album_artists(album_id);
        CREATE INDEX IF NOT EXISTS idx_album_artists_artist ON album_artists(artist_id);

        -- Library folder watch configuration
        CREATE TABLE IF NOT EXISTS library_folders (
            path TEXT PRIMARY KEY,
            label TEXT NOT NULL,
            is_active INTEGER NOT NULL DEFAULT 1,
            last_scanned TEXT DEFAULT NULL,
            scan_error TEXT DEFAULT NULL,
            created_at TEXT NOT NULL
        );

        -- Create index for folder active status
        CREATE INDEX IF NOT EXISTS idx_library_folders_active ON library_folders(is_active);

        -- Sync metadata for tracking changes
        CREATE TABLE IF NOT EXISTS sync_state (
            id TEXT PRIMARY KEY,
            entity_type TEXT NOT NULL,
            entity_id TEXT NOT NULL,
            sync_hash TEXT NOT NULL,
            last_synced TEXT NOT NULL,
            UNIQUE(entity_type, entity_id)
        );

        -- Create index for sync lookups
        CREATE INDEX IF NOT EXISTS idx_sync_state_entity ON sync_state(entity_type, entity_id);
        "#
    )
}

/// Track record for database operations
#[derive(Debug, Clone)]
pub struct TrackRecord {
    pub id: Uuid,
    pub video_id: Option<String>,
    pub title: String,
    pub artist: String,
    pub thumbnail_url: Option<String>,
    pub duration_ms: i64,
    pub album_id: Option<Uuid>,
    pub album_name: Option<String>,
    pub artist_id: Option<Uuid>,
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
    pub last_played: Option<DateTime<Utc>>,
    pub rating: Option<i32>,
    pub lyrics: Option<String>,
    pub release_date: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Artist record for database operations
#[derive(Debug, Clone)]
pub struct ArtistRecord {
    pub id: Uuid,
    pub name: String,
    pub sort_name: Option<String>,
    pub artist_type: ArtistType,
    pub genres: Option<String>,
    pub biography: Option<String>,
    pub art_path: Option<String>,
    pub url: Option<String>,
    pub mbid: Option<String>,
    pub track_count: i32,
    pub album_count: i32,
    pub subscriber_count: Option<i64>,
    pub monthly_listener_count: Option<i64>,
    pub is_subscribed: Option<bool>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Artist type enum matching Kotlin ArtistType
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub enum ArtistType {
    #[default]
    Artist,
    Group,
    Orchestra,
    Choir,
    Other,
}

/// Album record for database operations
#[derive(Debug, Clone)]
pub struct AlbumRecord {
    pub id: Uuid,
    pub title: String,
    pub artist_id: Option<Uuid>,
    pub artist_ids: Option<String>,
    pub release_date: Option<DateTime<Utc>>,
    pub cover_path: Option<String>,
    pub total_tracks: i32,
    pub total_discs: i32,
    pub genres: Option<String>,
    pub compilation: bool,
    pub label: Option<String>,
    pub copyright_text: Option<String>,
    pub record_label: Option<String>,
    pub disc_count: Option<i32>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Playlist source enum matching Kotlin PlaylistSource
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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

/// Playlist record for database operations
#[derive(Debug, Clone)]
pub struct PlaylistRecord {
    pub id: Uuid,
    pub browse_id: Option<String>,
    pub name: String,
    pub description: Option<String>,
    pub cover_path: Option<String>,
    pub source: PlaylistSource,
    pub collaborative: bool,
    pub is_explicit: Option<bool>,
    pub track_count: i32,
    pub duration_ms: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Library folder watch configuration
#[derive(Debug, Clone)]
pub struct LibraryFolderRecord {
    pub path: String,
    pub label: String,
    pub is_active: bool,
    pub last_scanned: Option<DateTime<Utc>>,
    pub scan_error: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Track artist junction record
#[derive(Debug, Clone)]
pub struct TrackArtistRecord {
    pub track_id: Uuid,
    pub artist_id: Uuid,
    pub role: ArtistRole,
}

/// Artist role in a track
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ArtistRole {
    Primary,
    Featured,
    Composer,
}

impl Default for ArtistRole {
    fn default() -> Self {
        ArtistRole::Primary
    }
}

/// Album artist junction record
#[derive(Debug, Clone)]
pub struct AlbumArtistRecord {
    pub album_id: Uuid,
    pub artist_id: Uuid,
}

/// Sync state record for tracking changes
#[derive(Debug, Clone)]
pub struct SyncStateRecord {
    pub id: Uuid,
    pub entity_type: String,
    pub entity_id: Uuid,
    pub sync_hash: String,
    pub last_synced: DateTime<Utc>,
}

/// Repository for library operations
pub struct LibraryRepository {
    conn: rusqlite::Connection,
}

impl LibraryRepository {
    /// Open a database at the given path and initialize schema
    pub fn open(path: &std::path::Path) -> SqlResult<Self> {
        let conn = Connection::open(path)?;
        init_schema(&conn)?;
        Ok(Self { conn })
    }

    /// Create a new track record
    pub fn insert_track(&self, track: &TrackRecord) -> SqlResult<()> {
        self.conn.execute(
            r#"
            INSERT INTO tracks (
                id, video_id, title, artist, thumbnail_url, duration_ms,
                album_id, album_name, artist_id, artist_ids, is_video,
                is_explicit, source_quality, source, local_uri, local_path,
                file_path, file_size_bytes, bitrate, sample_rate, channels,
                genres, composers, copyright, track_number, disc_number,
                album_total_tracks, date_added_seconds, date_modified_seconds,
                play_count, skip_count, last_played, rating, lyrics,
                release_date, created_at, updated_at
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6,
                ?7, ?8, ?9, ?10, ?11,
                ?12, ?13, ?14, ?15, ?16,
                ?17, ?18, ?19, ?20, ?21,
                ?22, ?23, ?24, ?25, ?26,
                ?27, ?28, ?29, ?30, ?31,
                ?32, ?33, ?34, ?35, ?36,
                ?37, ?38, ?39
            )
            "#,
            params![
                track.id.to_string(),
                track.video_id.as_deref(),
                &track.title,
                &track.artist,
                track.thumbnail_url.as_deref(),
                track.duration_ms,
                track.album_id.map(|v| v.to_string()),
                track.album_name.as_deref(),
                track.artist_id.map(|v| v.to_string()),
                track.artist_ids.as_deref(),
                if track.is_video { 1 } else { 0 },
                track.is_explicit.map(|v| v as i32),
                track.source_quality.as_deref(),
                track.source.as_deref(),
                track.local_uri.as_deref(),
                track.local_path.as_deref(),
                track.file_path.as_deref(),
                track.file_size_bytes,
                track.bitrate,
                track.sample_rate,
                track.channels,
                track.genres.as_deref(),
                track.composers.as_deref(),
                track.copyright.as_deref(),
                track.track_number,
                track.disc_number,
                track.album_total_tracks,
                track.date_added_seconds,
                track.date_modified_seconds,
                track.play_count as i32,
                track.skip_count as i32,
                track.last_played.map(|v| v.to_rfc3339()),
                track.rating,
                track.lyrics.as_deref(),
                track.release_date.map(|v| v.to_rfc3339()),
                track.created_at.to_rfc3339(),
                track.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    /// Get a track by ID
    pub fn get_track(&self, id: &Uuid) -> SqlResult<Option<TrackRecord>> {
        self.conn
            .query_row(
                r#"
                SELECT id, video_id, title, artist, thumbnail_url, duration_ms,
                       album_id, album_name, artist_id, artist_ids, is_video,
                       is_explicit, source_quality, source, local_uri, local_path,
                       file_path, file_size_bytes, bitrate, sample_rate, channels,
                       genres, composers, copyright, track_number, disc_number,
                       album_total_tracks, date_added_seconds, date_modified_seconds,
                       play_count, skip_count, last_played, rating, lyrics,
                       release_date, created_at, updated_at
                FROM tracks
                WHERE id = ?
                "#,
                params![id.to_string()],
                |row| {
                    let parse_uuid_opt =
                        |s: Option<String>| s.and_then(|s| Uuid::parse_str(&s).ok());

                    Ok(TrackRecord {
                        id: Uuid::parse_str(&row.get::<_, String>("id")?)
                                            .map_err(|e| rusqlite::Error::FromSqlConversionFailure(
                                                0,
                                                rusqlite::types::Type::Text,
                                                Box::new(e),
                                            ))?,
                        video_id: row.get("video_id")?,
                        title: row.get("title")?,
                        artist: row.get("artist")?,
                        thumbnail_url: row.get("thumbnail_url")?,
                        duration_ms: row.get("duration_ms")?,
                        album_id: parse_uuid_opt(row.get("album_id")?),
                        album_name: row.get("album_name")?,
                        artist_id: parse_uuid_opt(row.get("artist_id")?),
                        artist_ids: row.get("artist_ids")?,
                        is_video: row.get::<_, i32>("is_video")? != 0,
                        is_explicit: row.get("is_explicit")?,
                        source_quality: row.get("source_quality")?,
                        source: row.get("source")?,
                        local_uri: row.get("local_uri")?,
                        local_path: row.get("local_path")?,
                        file_path: row.get("file_path")?,
                        file_size_bytes: row.get("file_size_bytes")?,
                        bitrate: row.get("bitrate")?,
                        sample_rate: row.get("sample_rate")?,
                        channels: row.get::<_, Option<i16>>("channels")?,
                        genres: row.get("genres")?,
                        composers: row.get("composers")?,
                        copyright: row.get("copyright")?,
                        track_number: row.get("track_number")?,
                        disc_number: row.get("disc_number")?,
                        album_total_tracks: row.get("album_total_tracks")?,
                        date_added_seconds: row.get("date_added_seconds")?,
                        date_modified_seconds: row.get("date_modified_seconds")?,
                        play_count: row.get::<_, i32>("play_count")?,
                        skip_count: row.get::<_, i32>("skip_count")?,
                        last_played: row
                            .get::<_, Option<String>>("last_played")?
                            .and_then(|s| {
                                DateTime::parse_from_rfc3339(&s)
                                    .ok()?
                                    .with_timezone(&Utc)
                                    .into()
                            }),
                        rating: row.get("rating")?,
                        lyrics: row.get("lyrics")?,
                        release_date: row
                            .get::<_, Option<String>>("release_date")?
                            .and_then(|s| {
                                DateTime::parse_from_rfc3339(&s)
                                    .ok()?
                                    .with_timezone(&Utc)
                                    .into()
                            }),
                        created_at: {
                            let s = row.get::<_, String>("created_at")?;
                            DateTime::parse_from_rfc3339(&s)
                                .map_err(|e| rusqlite::Error::FromSqlConversionFailure(
                                    0,
                                    rusqlite::types::Type::Text,
                                    Box::new(e),
                                ))?
                                .with_timezone(&Utc)
                        },
                        updated_at: {
                            let s = row.get::<_, String>("updated_at")?;
                            DateTime::parse_from_rfc3339(&s)
                                .map_err(|e| rusqlite::Error::FromSqlConversionFailure(
                                    0,
                                    rusqlite::types::Type::Text,
                                    Box::new(e),
                                ))?
                                .with_timezone(&Utc)
                        },
                    })
                },
            )
            .map(Some)
    }

    /// Create an artist record
    pub fn insert_artist(&self, artist: &ArtistRecord) -> SqlResult<()> {
        self.conn.execute(
            r#"
            INSERT INTO artists (
                id, name, sort_name, artist_type, genres, biography,
                art_path, url, mbid, track_count, album_count,
                subscriber_count, monthly_listener_count, is_subscribed,
                created_at, updated_at
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6,
                ?7, ?8, ?9, ?10, ?11,
                ?12, ?13, ?14,
                ?15, ?16
            )
            "#,
            params![
                artist.id.to_string(),
                &artist.name,
                artist.sort_name.as_deref(),
                format!("{:?}", artist.artist_type),
                artist.genres.as_deref(),
                artist.biography.as_deref(),
                artist.art_path.as_deref(),
                artist.url.as_deref(),
                artist.mbid.as_deref(),
                artist.track_count as i32,
                artist.album_count as i32,
                artist.subscriber_count,
                artist.monthly_listener_count,
                artist.is_subscribed.map(|v| if v { 1 } else { 0 }),
                artist.created_at.to_rfc3339(),
                artist.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    /// Create an album record
    pub fn insert_album(&self, album: &AlbumRecord) -> SqlResult<()> {
        self.conn.execute(
            r#"
            INSERT INTO albums (
                id, title, artist_id, artist_ids, release_date, cover_path,
                total_tracks, total_discs, genres, compilation, label,
                copyright_text, record_label, disc_count,
                created_at, updated_at
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6,
                ?7, ?8, ?9, ?10, ?11,
                ?12, ?13, ?14,
                ?15, ?16
            )
            "#,
            params![
                album.id.to_string(),
                &album.title,
                album.artist_id.map(|v| v.to_string()),
                album.artist_ids.as_deref(),
                album.release_date.map(|v| v.to_rfc3339()),
                album.cover_path.as_deref(),
                album.total_tracks as i32,
                album.total_discs as i32,
                album.genres.as_deref(),
                if album.compilation { 1 } else { 0 },
                album.label.as_deref(),
                album.copyright_text.as_deref(),
                album.record_label.as_deref(),
                album.disc_count,
                album.created_at.to_rfc3339(),
                album.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    /// Create a playlist record
    pub fn insert_playlist(&self, playlist: &PlaylistRecord) -> SqlResult<()> {
        self.conn.execute(
            r#"
            INSERT INTO playlists (
                id, browse_id, name, description, cover_path, source,
                collaborative, is_explicit, track_count, duration_ms,
                created_at, updated_at
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6,
                ?7, ?8, ?9, ?10,
                ?11, ?12
            )
            "#,
            params![
                playlist.id.to_string(),
                playlist.browse_id.as_deref(),
                &playlist.name,
                playlist.description.as_deref(),
                playlist.cover_path.as_deref(),
                format!("{:?}", playlist.source),
                if playlist.collaborative { 1 } else { 0 },
                playlist.is_explicit.map(|v| v as i32),
                playlist.track_count as i32,
                playlist.duration_ms,
                playlist.created_at.to_rfc3339(),
                playlist.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    /// Add a track to a playlist at a specific position
    pub fn add_playlist_track(&self, playlist_id: &Uuid, track_id: &Uuid, position: i32) -> SqlResult<()> {
        self.conn.execute(
            r#"
            INSERT INTO playlist_tracks (playlist_id, track_id, position, added_at)
            VALUES (?1, ?2, ?3, ?4)
            "#,
            params![playlist_id.to_string(), track_id.to_string(), position, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    /// Remove a track from a playlist
    pub fn remove_playlist_track(&self, playlist_id: &Uuid, track_id: &Uuid) -> SqlResult<bool> {
        let rows = self.conn.execute(
            r#"DELETE FROM playlist_tracks WHERE playlist_id = ?1 AND track_id = ?2"#,
            params![playlist_id.to_string(), track_id.to_string()],
        )?;
        Ok(rows > 0)
    }

    /// Get tracks in a playlist ordered by position
    pub fn get_playlist_tracks(&self, playlist_id: &Uuid) -> SqlResult<Vec<Uuid>> {
        self.conn
            .prepare("SELECT track_id FROM playlist_tracks WHERE playlist_id = ? ORDER BY position")?
            .query_map(params![playlist_id.to_string()], |row| {
                let s: String = row.get(0)?;
                Uuid::parse_str(&s).map_err(|e| rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                ))
            })?
            .collect::<SqlResult<Vec<_>>>()
    }

    /// Add track-artist association
    pub fn add_track_artist(&self, track_id: &Uuid, artist_id: &Uuid, role: ArtistRole) -> SqlResult<()> {
        self.conn.execute(
            r#"
            INSERT INTO track_artists (track_id, artist_id, role)
            VALUES (?1, ?2, ?3)
            "#,
            params![track_id.to_string(), artist_id.to_string(), format!("{:?}", role)],
        )?;
        Ok(())
    }

    /// Get artists for a track
    pub fn get_track_artists(&self, _track_id: &Uuid) -> SqlResult<Vec<ArtistRecord>> {
        // This would need to join with artists table
        // Simplified implementation
        Ok(Vec::new())
    }

    /// Add album-artist association
    pub fn add_album_artist(&self, album_id: &Uuid, artist_id: &Uuid) -> SqlResult<()> {
        self.conn.execute(
            r#"
            INSERT INTO album_artists (album_id, artist_id)
            VALUES (?1, ?2)
            "#,
            params![album_id.to_string(), artist_id.to_string()],
        )?;
        Ok(())
    }
    /// Search tracks by query string (full-text search on title, artist, album)
    pub fn search(&self, query: &str, limit: usize) -> SqlResult<Vec<TrackRecord>> {
        let pattern = format!("%{}%", query.to_lowercase());
        let rows = self.conn
            .prepare(
                r#"
                SELECT id, video_id, title, artist, thumbnail_url, duration_ms,
                       album_id, album_name, artist_id, artist_ids, is_video,
                       is_explicit, source_quality, source, local_uri, local_path,
                       file_path, file_size_bytes, bitrate, sample_rate, channels,
                       genres, composers, copyright, track_number, disc_number,
                       album_total_tracks, date_added_seconds, date_modified_seconds,
                       play_count, skip_count, last_played, rating, lyrics,
                       release_date, created_at, updated_at
                FROM tracks
                WHERE LOWER(title) LIKE ?1
                   OR LOWER(artist) LIKE ?1
                   OR LOWER(album_name) LIKE ?1
                ORDER BY play_count DESC, last_played DESC
                LIMIT ?2
                "#
            )?
            .query_map(params![pattern, limit as i32], |row| {
                let parse_uuid_opt = |s: Option<String>| s.and_then(|s| Uuid::parse_str(&s).ok());

                Ok(TrackRecord {
                    id: Uuid::parse_str(&row.get::<_, String>("id")?)
                        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(
                            0,
                            rusqlite::types::Type::Text,
                            Box::new(e),
                        ))?,
                    video_id: row.get("video_id")?,
                    title: row.get("title")?,
                    artist: row.get("artist")?,
                    thumbnail_url: row.get("thumbnail_url")?,
                    duration_ms: row.get("duration_ms")?,
                    album_id: parse_uuid_opt(row.get("album_id")?)?,
                    album_name: row.get("album_name")?,
                    artist_id: parse_uuid_opt(row.get("artist_id")?)?,
                    artist_ids: row.get("artist_ids")?,
                    is_video: row.get::<_, i32>("is_video")? != 0,
                    is_explicit: row.get("is_explicit")?,
                    source_quality: row.get("source_quality")?,
                    source: row.get("source")?,
                    local_uri: row.get("local_uri")?,
                    local_path: row.get("local_path")?,
                    file_path: row.get("file_path")?,
                    file_size_bytes: row.get("file_size_bytes")?,
                    bitrate: row.get("bitrate")?,
                    sample_rate: row.get("sample_rate")?,
                    channels: row.get::<_, Option<i16>>("channels")?,
                    genres: row.get("genres")?,
                    composers: row.get("composers")?,
                    copyright: row.get("copyright")?,
                    track_number: row.get("track_number")?,
                    disc_number: row.get("disc_number")?,
                    album_total_tracks: row.get("album_total_tracks")?,
                    date_added_seconds: row.get("date_added_seconds")?,
                    date_modified_seconds: row.get("date_modified_seconds")?,
                    play_count: row.get::<_, i32>("play_count")?,
                    skip_count: row.get::<_, i32>("skip_count")?,
                    last_played: row
                        .get::<_, Option<String>>("last_played")?
                        .and_then(|s| {
                            DateTime::parse_from_rfc3339(&s)
                                .ok()?
                                .with_timezone(&Utc)
                                .into()
                        }),
                    rating: row.get("rating")?,
                    lyrics: row.get("lyrics")?,
                    release_date: row
                        .get::<_, Option<String>>("release_date")?
                        .and_then(|s| {
                            DateTime::parse_from_rfc3339(&s)
                                .ok()?
                                .with_timezone(&Utc)
                                .into()
                        }),
                    created_at: {
                        let s = row.get::<_, String>("created_at")?;
                        DateTime::parse_from_rfc3339(&s)
                            .map_err(|e| rusqlite::Error::FromSqlConversionFailure(
                                0,
                                rusqlite::types::Type::Text,
                                Box::new(e),
                            ))?
                            .with_timezone(&Utc)
                    },
                    updated_at: {
                        let s = row.get::<_, String>("updated_at")?;
                        DateTime::parse_from_rfc3339(&s)
                            .map_err(|e| rusqlite::Error::FromSqlConversionFailure(
                                0,
                                rusqlite::types::Type::Text,
                                Box::new(e),
                            ))?
                            .with_timezone(&Utc)
                    },
                })
            })?
            .collect()
    }

    /// Update an existing track record
    pub fn update_track(&self, track: &TrackRecord) -> SqlResult<()> {
        self.conn.execute(
            r#"
            UPDATE tracks SET
                video_id = ?2, title = ?3, artist = ?4, thumbnail_url = ?5,
                duration_ms = ?6, album_id = ?7, album_name = ?8, artist_id = ?9,
                artist_ids = ?10, is_video = ?11, is_explicit = ?12,
                source_quality = ?13, source = ?14, local_uri = ?15, local_path = ?16,
                file_path = ?17, file_size_bytes = ?18, bitrate = ?19, sample_rate = ?20,
                channels = ?21, genres = ?22, composers = ?23, copyright = ?24,
                track_number = ?25, disc_number = ?26, album_total_tracks = ?27,
                date_added_seconds = ?28, date_modified_seconds = ?29,
                play_count = ?30, skip_count = ?31, last_played = ?32, rating = ?33,
                lyrics = ?34, release_date = ?35, created_at = ?36, updated_at = ?37
            WHERE id = ?1
            "#,
            params![
                track.id.to_string(),
                track.video_id.as_deref(),
                &track.title,
                &track.artist,
                track.thumbnail_url.as_deref(),
                track.duration_ms,
                track.album_id.map(|v| v.to_string()),
                track.album_name.as_deref(),
                track.artist_id.map(|v| v.to_string()),
                track.artist_ids.as_deref(),
                if track.is_video { 1 } else { 0 },
                track.is_explicit.map(|v| v as i32),
                track.source_quality.as_deref(),
                track.source.as_deref(),
                track.local_uri.as_deref(),
                track.local_path.as_deref(),
                track.file_path.as_deref(),
                track.file_size_bytes,
                track.bitrate,
                track.sample_rate,
                track.channels,
                track.genres.as_deref(),
                track.composers.as_deref(),
                track.copyright.as_deref(),
                track.track_number,
                track.disc_number,
                track.album_total_tracks,
                track.date_added_seconds,
                track.date_modified_seconds,
                track.play_count,
                track.skip_count,
                track.last_played.map(|v| v.to_rfc3339()),
                track.rating,
                track.lyrics.as_deref(),
                track.release_date.map(|v| v.to_rfc3339()),
                track.created_at.to_rfc3339(),
                track.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    /// Delete a track by ID
    pub fn delete_track(&self, id: &Uuid) -> SqlResult<bool> {
        let rows = self.conn.execute(
            "DELETE FROM tracks WHERE id = ?",
            params![id.to_string()],
        )?;
        Ok(rows > 0)
    }

    /// Get all tracks
    pub fn get_all_tracks(&self) -> SqlResult<Vec<TrackRecord>> {
        self.conn
            .prepare(
                r#"
                SELECT id, video_id, title, artist, thumbnail_url, duration_ms,
                       album_id, album_name, artist_id, artist_ids, is_video,
                       is_explicit, source_quality, source, local_uri, local_path,
                       file_path, file_size_bytes, bitrate, sample_rate, channels,
                       genres, composers, copyright, track_number, disc_number,
                       album_total_tracks, date_added_seconds, date_modified_seconds,
                       play_count, skip_count, last_played, rating, lyrics,
                       release_date, created_at, updated_at
                FROM tracks
                ORDER BY title ASC
                "#
            )?
            .query_map([], |row| {
                let parse_uuid_opt = |s: Option<String>| s.and_then(|s| Uuid::parse_str(&s).ok());

                Ok(TrackRecord {
                    id: Uuid::parse_str(&row.get::<_, String>("id")?)
                        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(
                            0,
                            rusqlite::types::Type::Text,
                            Box::new(e),
                        ))?,
                    video_id: row.get("video_id")?,
                    title: row.get("title")?,
                    artist: row.get("artist")?,
                    thumbnail_url: row.get("thumbnail_url")?,
                    duration_ms: row.get("duration_ms")?,
                    album_id: parse_uuid_opt(row.get("album_id")?)?,
                    album_name: row.get("album_name")?,
                    artist_id: parse_uuid_opt(row.get("artist_id")?)?,
                    artist_ids: row.get("artist_ids")?,
                    is_video: row.get::<_, i32>("is_video")? != 0,
                    is_explicit: row.get("is_explicit")?,
                    source_quality: row.get("source_quality")?,
                    source: row.get("source")?,
                    local_uri: row.get("local_uri")?,
                    local_path: row.get("local_path")?,
                    file_path: row.get("file_path")?,
                    file_size_bytes: row.get("file_size_bytes")?,
                    bitrate: row.get("bitrate")?,
                    sample_rate: row.get("sample_rate")?,
                    channels: row.get::<_, Option<i16>>("channels")?,
                    genres: row.get("genres")?,
                    composers: row.get("composers")?,
                    copyright: row.get("copyright")?,
                    track_number: row.get("track_number")?,
                    disc_number: row.get("disc_number")?,
                    album_total_tracks: row.get("album_total_tracks")?,
                    date_added_seconds: row.get("date_added_seconds")?,
                    date_modified_seconds: row.get("date_modified_seconds")?,
                    play_count: row.get::<_, i32>("play_count")?,
                    skip_count: row.get::<_, i32>("skip_count")?,
                    last_played: row
                        .get::<_, Option<String>>("last_played")?
                        .and_then(|s| {
                            DateTime::parse_from_rfc3339(&s)
                                .ok()?
                                .with_timezone(&Utc)
                                .into()
                        }),
                    rating: row.get("rating")?,
                    lyrics: row.get("lyrics")?,
                    release_date: row
                        .get::<_, Option<String>>("release_date")?
                        .and_then(|s| {
                            DateTime::parse_from_rfc3339(&s)
                                .ok()?
                                .with_timezone(&Utc)
                                .into()
                        }),
                    created_at: {
                        let s = row.get::<_, String>("created_at")?;
                        DateTime::parse_from_rfc3339(&s)
                            .map_err(|e| rusqlite::Error::FromSqlConversionFailure(
                                0,
                                rusqlite::types::Type::Text,
                                Box::new(e),
                            ))?
                            .with_timezone(&Utc)
                    },
                    updated_at: {
                        let s = row.get::<_, String>("updated_at")?;
                        DateTime::parse_from_rfc3339(&s)
                            .map_err(|e| rusqlite::Error::FromSqlConversionFailure(
                                0,
                                rusqlite::types::Type::Text,
                                Box::new(e),
                            ))?
                            .with_timezone(&Utc)
                    },
                })
            })?
            .collect()
    }
}