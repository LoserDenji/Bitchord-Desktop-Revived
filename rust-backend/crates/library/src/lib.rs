//! Music library scanner and indexer
//!
//! This crate provides SQLite-based persistence for the music library,
//! including tracks, albums, artists, playlists, and their relationships.

pub mod sqlite;

use anyhow::Result;
use std::path::PathBuf;
use bitchord_core::{ScanConfig, TrackMetadata};

/// Library scanner
pub struct LibraryScanner {
    config: ScanConfig,
    results: Vec<LibraryScanResult>,
}

#[derive(Debug, Clone)]
pub struct LibraryScanResult {
    pub track_id: uuid::Uuid,
    pub metadata: TrackMetadata,
}

impl LibraryScanner {
    pub fn new(config: ScanConfig) -> Self {
        Self {
            config,
            results: Vec::new(),
        }
    }
    
    pub fn with_default_config() -> Self {
        Self::new(ScanConfig::default())
    }
    
    pub async fn scan(&mut self, _path: &PathBuf) -> Result<LibraryScanStats> {
        // Placeholder: actual implementation would walk directories and read metadata
        Ok(LibraryScanStats {
            tracks_found: 0,
            bytes_scanned: 0,
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct LibraryScanStats {
    pub tracks_found: usize,
    pub bytes_scanned: u64,
}