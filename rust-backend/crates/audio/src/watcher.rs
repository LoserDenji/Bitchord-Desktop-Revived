//! File system watcher for audio directories using notify crate
//!
//! This module provides a lightweight wrapper around the notify crate that watches
//! directories for changes and yields events asynchronously.

use anyhow::{Context, Result};
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use parking_lot::RwLock;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::Stream;

/// Audio file extensions we recognize
const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "m4a", "aac", "flac", "wav", "ogg", "opus", "wma", "ape", "aiff", "alac",
];

/// File system change events from the watcher
#[derive(Debug, Clone)]
pub enum FileEvent {
    /// A new audio file was created
    Created(PathBuf),
    /// An audio file was modified
    Modified(PathBuf),
    /// An audio file was deleted
    Deleted(PathBuf),
    /// A file was renamed (old path, new path)
    Renamed(PathBuf, PathBuf),
    /// A new directory was created
    DirectoryCreated(PathBuf),
    /// A directory was removed
    DirectoryRemoved(PathBuf),
}

impl FileEvent {
    /// Check if this event is relevant to audio files
    pub fn is_audio_event(&self) -> bool {
        match self {
            FileEvent::Created(path) | FileEvent::Modified(path) | FileEvent::Deleted(path) => {
                is_audio_path(path) || path.is_dir()
            }
            FileEvent::Renamed(old, new) => {
                is_audio_path(old) || is_audio_path(new) ||
                old.is_dir() || new.is_dir()
            }
            FileEvent::DirectoryCreated(_path) | FileEvent::DirectoryRemoved(_path) => {
                true // All directory events are relevant
            }
        }
    }

    /// Check if the path is an audio file
    fn _is_audio_path(path: &PathBuf) -> bool {
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| AUDIO_EXTENSIONS.contains(&e))
            .unwrap_or(false)
    }
}

/// Convert notify::Event to FileEvent
impl From<notify::Event> for FileEvent {
    fn from(event: notify::Event) -> Self {
        match event.kind {
            notify::EventKind::Create(_) => {
                let path = event.paths.first().cloned().unwrap_or_default();
                FileEvent::Created(path)
            }
            notify::EventKind::Modify(_) => {
                let path = event.paths.first().cloned().unwrap_or_default();
                FileEvent::Modified(path)
            }
            notify::EventKind::Remove(_) => {
                let path = event.paths.first().cloned().unwrap_or_default();
                FileEvent::Deleted(path)
            }
            _ => {
                match event.paths.as_slice() {
                    [old, new] => FileEvent::Renamed(old.clone(), new.clone()),
                    _ => FileEvent::Created(PathBuf::new()),
                }
            }
        }
    }
}

/// FS Watcher that monitors directories for file system changes
pub struct AudioWatcher {
    watched_dirs: Arc<RwLock<HashSet<PathBuf>>>,
    watcher: Option<RecommendedWatcher>,
    rx: Option<mpsc::Receiver<FileEvent>>,
}

impl AudioWatcher {
    /// Create a new audio watcher
    pub fn new() -> Result<Self> {
        Ok(Self {
            watched_dirs: Arc::new(RwLock::new(HashSet::new())),
            watcher: None,
            rx: None,
        })
    }

    /// Add a directory to watch
    pub fn add_directory(&mut self, path: PathBuf) -> Result<()> {
        let canonical = path.canonicalize()
            .unwrap_or_else(|_| path.clone());
        self.watched_dirs.write().insert(canonical);
        Ok(())
    }

    /// Remove a directory from watch list
    pub fn remove_directory(&mut self, path: &std::path::Path) -> Result<()> {
        let canonical = path.canonicalize()
            .unwrap_or_else(|_| path.to_path_buf());
        self.watched_dirs.write().remove(&canonical);
        Ok(())
    }

    /// Get the list of watched directories
    pub fn watched_directories(&self) -> Vec<PathBuf> {
        self.watched_dirs.read().iter().cloned().collect()
    }

    /// Initialize and start the file system watcher
    /// Returns a stream of file events
    pub fn start(&mut self) -> Result<impl Stream<Item = FileEvent>> {
        let (tx, rx) = mpsc::channel(64);
        let watched_dirs = self.watched_dirs.clone();

        // Create the watcher with a callback
        let mut watcher: RecommendedWatcher = Watcher::new(
            move |res| {
                if let Ok(event) = res {
                    let _ = tx.try_send(FileEvent::from(event));
                }
            },
            Config::default()
                .with_compare_contents(true),
        ).context("Failed to create file system watcher")?;

        // Watch all directories
        for dir in watched_dirs.read().iter() {
            if dir.exists() {
                watcher.watch(dir, RecursiveMode::Recursive)
                    .with_context(|| format!("Failed to watch directory: {:?}", dir))?;
            }
        }

        self.watcher = Some(watcher);
        self.rx = Some(rx);

        Ok(ReceiverStream::new(self.rx.take().unwrap()))
    }

    /// Stop watching and clean up
    pub fn stop(&mut self) -> Result<()> {
        self.watcher = None;
        self.rx = None;
        Ok(())
    }
}

impl Default for AudioWatcher {
    fn default() -> Self {
        Self::new().expect("Failed to create default AudioWatcher")
    }
}

/// Helper to check if a path has an audio extension
pub fn is_audio_path(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| AUDIO_EXTENSIONS.contains(&e))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_audio_path() {
        assert!(is_audio_path(Path::new("song.mp3")));
        assert!(is_audio_path(Path::new("track.flac")));
        assert!(is_audio_path(Path::new("music.ogg")));
        assert!(!is_audio_path(Path::new("image.png")));
        assert!(!is_audio_path(Path::new("document.pdf")));
    }
}