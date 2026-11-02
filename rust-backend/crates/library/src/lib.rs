//! Music library scanner and indexer
//!
//! This crate provides SQLite-based persistence for the music library,
//! including tracks, albums, artists, playlists, and their relationships.

pub mod sqlite;

use anyhow::Result;
use std::path::PathBuf;
use walkdir::WalkDir;
use lofty::read_from_path;
use chrono::{DateTime, Utc};
use uuid::Uuid;
use bitchord_core::{ScanConfig, TrackMetadata};

use self::sqlite::{LibraryRepository, TrackRecord};

/// Library scanner
pub struct LibraryScanner {
    config: ScanConfig,
    results: Vec<LibraryScanResult>,
    repo: Option<LibraryRepository>,
}

#[derive(Debug, Clone)]
pub struct LibraryScanResult {
    pub track_id: Uuid,
    pub metadata: TrackMetadata,
}

impl LibraryScanner {
    pub fn new(config: ScanConfig) -> Self {
        Self {
            config: config.clone(),
            results: Vec::new(),
            repo: None,
        }
    }

    pub fn with_default_config() -> Self {
        Self::new(ScanConfig::default())
    }

    /// Scan a directory for audio files and index them
    pub async fn scan(&mut self, path: &PathBuf) -> Result<LibraryScanStats> {
        let start_time = std::time::Instant::now();
        let mut tracks_found = 0;
        let mut bytes_scanned = 0u64;

        let extensions: Vec<String> = self.config.extensions.iter().map(|s| s.to_lowercase()).collect();

        for entry in WalkDir::new(path)
            .follow_links(true)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let file_path = entry.path();

            if file_path.is_dir() {
                continue;
            }

            if let Some(ext) = file_path.extension() {
                let ext_str = ext.to_string_lossy().to_lowercase();
                if !extensions.iter().any(|e| e.replace('.', "") == ext_str) {
                    continue;
                }
            } else {
                continue;
            }

            if let Ok(metadata) = std::fs::metadata(file_path) {
                bytes_scanned += metadata.len();
            }

            if let Ok(audio_file) = read_from_path(file_path) {
                let title = audio_file.songtitle()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| {
                        file_path.file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("Unknown")
                            .to_string()
                    });

                let artists: Vec<String> = audio_file
                    .iter_tag_info()
                    .filter_map(|tag| tag.artist().map(|s| s.to_string()))
                    .collect();

                let album = audio_file.album().map(|s| s.to_string());

                let duration_ms = audio_file.duration()
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);

                let genres: Vec<String> = audio_file.genre().into_iter().map(|s| s.to_string()).collect();

                let track_number: Option<u32> = audio_file.track_number()
                    .and_then(|(num, _)| num)
                    .map(|n| n as u32);

                let disc_number = audio_file.disc_number()
                    .and_then(|(num, _)| num)
                    .map(|n| n as u32);

                let metadata = TrackMetadata {
                    title,
                    artists,
                    album,
                    genres,
                    track_number,
                    disc_number,
                    duration_ms,
                    bitrate: None,
                    sample_rate: None,
                    channels: None,
                    file_path: file_path.to_path_buf(),
                    file_size: Some(metadata.len() as u64),
                    date_added: Utc::now(),
                    date_modified: Some(Utc::now()),
                    art_path: None,
                };

                let track_id = Uuid::new_v4();
                let track_record = TrackRecord {
                    id: track_id,
                    video_id: None,
                    title: metadata.title.clone(),
                    artist: metadata.artists.join(", "),
                    thumbnail_url: None,
                    duration_ms: metadata.duration_ms as i64,
                    album_id: None,
                    album_name: metadata.album.clone(),
                    artist_id: None,
                    artist_ids: None,
                    is_video: false,
                    is_explicit: None,
                    source_quality: None,
                    source: Some("Local".to_string()),
                    local_uri: None,
                    local_path: Some(file_path.to_string_lossy().to_string()),
                    file_path: Some(file_path.to_string_lossy().to_string()),
                    file_size_bytes: Some(metadata.file_size.unwrap_or(0) as i64),
                    bitrate: metadata.bitrate.map(|b| b as i32),
                    sample_rate: metadata.sample_rate.map(|s| s as i32),
                    channels: metadata.channels.map(|c| c as i16),
                    genres: Some(metadata.genres.join("; ")),
                    composers: None,
                    copyright: None,
                    track_number: metadata.track_number.map(|n| n as i32),
                    disc_number: metadata.disc_number.map(|n| n as i32),
                    album_total_tracks: None,
                    date_added_seconds: Some(metadata.date_added.timestamp()),
                    date_modified_seconds: metadata.date_modified.map(|d| d.timestamp()),
                    play_count: 0,
                    skip_count: 0,
                    last_played: None,
                    rating: None,
                    lyrics: None,
                    release_date: None,
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                };

                self.results.push(LibraryScanResult {
                    track_id,
                    metadata,
                });

                tracks_found += 1;
            }
        }

        let duration_ms = start_time.elapsed().as_millis() as u64;

        Ok(LibraryScanStats {
            tracks_found,
            bytes_scanned,
            duration_ms,
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct LibraryScanStats {
    pub tracks_found: usize,
    pub bytes_scanned: u64,
    pub duration_ms: u64,
}

/// Scan a directory for audio files and return track metadata
pub fn scan_library(path: &std::path::Path, extensions: Vec<String>) -> Result<Vec<TrackMetadata>> {
    let mut scanner = LibraryScanner {
        config: ScanConfig {
            root_path: path.to_path_buf(),
            extensions,
            recursive: true,
            max_concurrent: 4,
        },
        results: Vec::new(),
        repo: None,
    };

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async { scanner.scan(&path.to_path_buf()).await })?;

    Ok(scanner.results.into_iter().map(|r| r.metadata).collect())
}

/// Create a library repository at the given path
pub fn open_library(path: &std::path::Path) -> Result<LibraryRepository> {
    Ok(LibraryRepository::open(path)?)
}

/// Search tracks by query string
pub fn search_tracks(repo: &LibraryRepository, query: &str, limit: usize) -> Result<Vec<TrackRecord>> {
    Ok(repo.search(query, limit)?)
}

/// Get a track by ID
pub fn get_track(repo: &LibraryRepository, id: &Uuid) -> Result<Option<TrackRecord>> {
    Ok(repo.get_track(id)?)
}

/// Create a new track
pub fn create_track(repo: &LibraryRepository, track: &TrackRecord) -> Result<()> {
    Ok(repo.insert_track(track)?)
}

/// Update an existing track
pub fn update_track(repo: &LibraryRepository, track: &TrackRecord) -> Result<()> {
    Ok(repo.update_track(track)?)
}

/// Delete a track by ID
pub fn delete_track(repo: &LibraryRepository, id: &Uuid) -> Result<bool> {
    Ok(repo.delete_track(id)?)
}
