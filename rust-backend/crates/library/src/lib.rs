//! Music library scanner and indexer
//!
//! This crate provides SQLite-based persistence for the music library,
//! including tracks, albums, artists, playlists, and their relationships.

pub mod sqlite;

use anyhow::Result;
use std::path::PathBuf;
use walkdir::WalkDir;
use lofty::prelude::read_from_path;
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::tag::Accessor;
use chrono::{DateTime, Utc};
use uuid::Uuid;
use bitchord_core::{ScanConfig, TrackMetadata};

use self::sqlite::{LibraryRepository, TrackRecord};

/// Library scanner
pub struct LibraryScanner {
    config: ScanConfig,
}

impl LibraryScanner {
    pub fn new(config: ScanConfig) -> Self {
        Self {
            config: config.clone(),
        }
    }

    pub fn with_default_config() -> Self {
        Self::new(ScanConfig::default())
    }

    /// Scan a directory for audio files and return metadata
    pub fn scan(&self, path: &PathBuf) -> Result<(Vec<LibraryScanResult>, LibraryScanStats)> {
        let start_time = std::time::Instant::now();
        let mut results = Vec::new();
        let mut tracks_found = 0u64;
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
                let title = audio_file
                    .primary_tag()
                    .title()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| {
                        file_path
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("Unknown")
                            .to_string()
                    });

                let artists: Vec<String> = audio_file
                    .primary_tag()
                    .artist()
                    .map(|s| vec![s.to_string()])
                    .unwrap_or_default();

                let album = audio_file
                    .primary_tag()
                    .album()
                    .map(|s| s.to_string());

                let duration_ms = audio_file
                    .properties()
                    .duration()
                    .as_millis() as u64;

                let genres: Vec<String> = audio_file
                    .primary_tag()
                    .genre()
                    .map(|g| g.to_string())
                    .into_iter()
                    .filter(|g| !g.is_empty())
                    .collect();

                let track_number: Option<u32> = audio_file
                    .primary_tag()
                    .track_number()
                    .map(|(num, _)| num);

                let disc_number: Option<u32> = audio_file
                    .primary_tag()
                    .disc_number()
                    .map(|(num, _)| num);

                let file_size = std::fs::metadata(file_path).map(|m| m.len());

                let metadata = TrackMetadata {
                    title,
                    artists,
                    album,
                    genres,
                    track_number,
                    disc_number,
                    duration_ms,
                    bitrate: audio_file.properties().overall_bitrate(),
                    sample_rate: audio_file.properties().sample_rate(),
                    channels: audio_file.properties().channels().map(|c| c as u16),
                    file_path: file_path.to_path_buf(),
                    file_size,
                    date_added: Utc::now(),
                    date_modified: Some(Utc::now()),
                    art_path: None,
                };

                let track_id = Uuid::new_v4();

                results.push(LibraryScanResult {
                    track_id,
                    metadata,
                });

                tracks_found += 1;
            }
        }

        let duration_ms = start_time.elapsed().as_millis() as u64;

        if let Some(progress) = &self.config.progress_callback {
            progress(tracks_found as usize, bytes_scanned, duration_ms);
        }

        let stats = LibraryScanStats {
            tracks_found: tracks_found as usize,
            bytes_scanned,
            duration_ms,
        };

        Ok((results, stats))
    }
}

#[derive(Debug, Clone)]
pub struct LibraryScanResult {
    pub track_id: Uuid,
    pub metadata: TrackMetadata,
}

#[derive(Debug, Clone, Default)]
pub struct LibraryScanStats {
    pub tracks_found: usize,
    pub bytes_scanned: u64,
    pub duration_ms: u64,
}

/// Convert TrackMetadata to TrackRecord for database insertion
impl From<TrackMetadata> for TrackRecord {
    fn from(meta: TrackMetadata) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            video_id: None,
            title: meta.title,
            artist: meta.artists.join(", "),
            thumbnail_url: None,
            duration_ms: meta.duration_ms as i64,
            album_id: None,
            album_name: meta.album,
            artist_id: None,
            artist_ids: Some(meta.artists.join(", ")),
            is_video: false,
            is_explicit: None,
            source_quality: None,
            source: None,
            local_uri: None,
            local_path: Some(meta.file_path.clone()),
            file_path: Some(meta.file_path.to_string_lossy().to_string()),
            file_size_bytes: meta.file_size.map(|s| s as i64),
            bitrate: meta.bitrate.map(|b| b as i32),
            sample_rate: meta.sample_rate.map(|s| s as i32),
            channels: meta.channels.map(|c| c as i16),
            genres: Some(meta.genres.join(", ")),
            composers: None,
            copyright: None,
            track_number: meta.track_number.map(|n| n as i32),
            disc_number: meta.disc_number.map(|n| n as i32),
            album_total_tracks: None,
            date_added_seconds: Some((now.timestamp() as i64) * 1000),
            date_modified_seconds: meta.date_modified.map(|d| d.timestamp() as i64),
            play_count: 0,
            skip_count: 0,
            last_played: None,
            rating: None,
            lyrics: None,
            release_date: None,
            created_at: now,
            updated_at: now,
        }
    }
}

/// Scan a directory for audio files and return track metadata
pub fn scan_library(path: &std::path::Path, extensions: Vec<String>) -> Result<Vec<TrackMetadata>> {
    let scanner = LibraryScanner {
        config: ScanConfig {
            root_path: path.to_path_buf(),
            extensions,
            recursive: true,
            max_concurrent: 4,
            batch_size: 100,
            progress_callback: None,
        },
    };

    let (results, _) = scanner.scan(&path.to_path_buf())?;
    Ok(results.into_iter().map(|r| r.metadata).collect())
}

/// Convert LibraryScanResult to TrackRecord
pub fn scan_result_to_track_record(result: LibraryScanResult) -> TrackRecord {
    result.metadata.into()
}

/// Create a library repository at the given path
pub fn open_library(path: &std::path::Path) -> Result<LibraryRepository> {
    LibraryRepository::open(path).map_err(|e| anyhow::anyhow!("{}", e))
}

/// Search tracks by query string
pub fn search_tracks(repo: &LibraryRepository, query: &str, limit: usize) -> Result<Vec<TrackRecord>> {
    repo.search(query, limit).map_err(|e| anyhow::anyhow!("{}", e))
}

/// Get a track by ID
pub fn get_track(repo: &LibraryRepository, id: &Uuid) -> Result<Option<TrackRecord>> {
    repo.get_track(id).map_err(|e| anyhow::anyhow!("{}", e))
}

/// Create a new track
pub fn create_track(repo: &LibraryRepository, track: &TrackRecord) -> Result<()> {
    repo.insert_track(track).map_err(|e| anyhow::anyhow!("{}", e))?;
    Ok(())
}

/// Update an existing track
pub fn update_track(repo: &LibraryRepository, track: &TrackRecord) -> Result<()> {
    repo.update_track(track).map_err(|e| anyhow::anyhow!("{}", e))?;
    Ok(())
}

/// Delete a track by ID
pub fn delete_track(repo: &LibraryRepository, id: &Uuid) -> Result<bool> {
    repo.delete_track(id).map_err(|e| anyhow::anyhow!("{}", e))
}

/// Get all tracks
pub fn get_all_tracks(repo: &LibraryRepository) -> Result<Vec<TrackRecord>> {
    repo.get_all_tracks().map_err(|e| anyhow::anyhow!("{}", e))
}
