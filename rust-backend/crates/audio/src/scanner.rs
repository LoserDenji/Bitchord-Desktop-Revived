//! Audio directory scanner - scans local directories and yields track titles

use anyhow::Result;
use infer;
use notify::{RecommendedWatcher, Watcher};
use parking_lot::RwLock;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::Stream;
use walkdir::WalkDir;

/// Audio file extensions we recognize
const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "m4a", "aac", "flac", "wav", "ogg", "opus", "wma", "ape", "aiff", "alac",
];

/// MIME types considered audio
const AUDIO_MIME_PREFIXES: &[&str] = &["audio/", "video/"];

/// Result type for a scanned file
#[derive(Debug, Clone)]
pub struct ScannedTrack {
    /// Full path to the file
    pub path: PathBuf,
    /// Inferred title from filename
    pub title: String,
    /// MIME type if detected
    pub mime: Option<String>,
    /// File size in bytes
    pub size: u64,
}

impl ScannedTrack {
    /// Create a new scanned track with inferred title from filename
    pub fn from_path(path: PathBuf) -> Result<Self> {
        let title = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();

        let size = std::fs::metadata(&path)
            .map(|m| m.len())
            .unwrap_or(0);

        // Try to detect MIME type using first 512 bytes (file signature)
        let mime = std::fs::read(&path)
            .ok()
            .and_then(|head| infer::get(&head).map(|m| m.mime_type().to_string()));

        Ok(Self { path, title, mime, size })
    }

    /// Check if this track appears to be an audio file
    pub fn is_audio_file(&self) -> bool {
        // First check extension
        if let Some(ext) = self.path.extension().and_then(|e| e.to_str()) {
            if AUDIO_EXTENSIONS.contains(&ext) {
                return true;
            }
        }

        // Then check MIME detection
        if let Some(ref mime) = self.mime {
            return AUDIO_MIME_PREFIXES.iter().any(|p| mime.starts_with(p));
        }

        false
    }
}

/// Scan result containing collected tracks
#[derive(Debug, Clone, Default)]
pub struct ScanResult {
    pub tracks: Vec<ScannedTrack>,
    pub directories_scanned: usize,
    pub bytes_scanned: u64,
}

/// File system change events from the watcher
#[derive(Debug, Clone)]
pub enum FileEvent {
    /// A new file was created
    Created(PathBuf),
    /// A file was modified
    Modified(PathBuf),
    /// A file was deleted
    Deleted(PathBuf),
    /// A file was renamed (old path, new path)
    Renamed(PathBuf, PathBuf),
}

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
            _ => FileEvent::Created(PathBuf::new()),
        }
    }
}

/// Audio scanner that walks directories and yields track titles via futures
pub struct AudioScanner {
    watched_dirs: Arc<RwLock<HashSet<PathBuf>>>,
}

impl AudioScanner {
    /// Create a new audio scanner
    pub fn new() -> Self {
        Self {
            watched_dirs: Arc::new(RwLock::new(HashSet::new())),
        }
    }

    /// Add a directory to watch
    pub fn add_directory(&self, path: impl Into<PathBuf>) -> Result<()> {
        let path = path.into();
        let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
        self.watched_dirs.write().insert(canonical);
        Ok(())
    }

    /// Remove a directory from watch list
    pub fn remove_directory(&self, path: &Path) -> Result<()> {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        self.watched_dirs.write().remove(&canonical);
        Ok(())
    }

    /// Get the list of watched directories
    pub fn watched_directories(&self) -> Vec<PathBuf> {
        self.watched_dirs.read().iter().cloned().collect()
    }

    /// Scan all tracked directories synchronously and return results
    pub fn scan_all(&self) -> ScanResult {
        let dirs = self.watched_dirs.read().iter().cloned().collect::<Vec<_>>();
        let mut tracks = Vec::new();
        let mut directories_scanned = 0usize;
        let mut bytes_scanned = 0u64;

        for dir in dirs {
            if dir.exists() {
                directories_scanned += 1;
                let walker = WalkDir::new(&dir);

                for entry in walker.into_iter().filter_map(|e| e.ok()) {
                    if entry.file_type().is_file() {
                        let path = entry.path().to_path_buf();
                        if Self::is_audio_path(&path) {
                            if let Ok(scanned) = ScannedTrack::from_path(path.clone()) {
                                if scanned.is_audio_file() {
                                    bytes_scanned += scanned.size;
                                    tracks.push(scanned);
                                }
                            }
                        }
                    }
                }
            }
        }

        ScanResult {
            tracks,
            directories_scanned,
            bytes_scanned,
        }
    }

    /// Scan a single directory synchronously and return results
    pub fn scan_directory(&self, dir: &Path) -> ScanResult {
        let mut tracks = Vec::new();
        let mut directories_scanned = 0usize;
        let mut bytes_scanned = 0u64;

        if dir.exists() {
            directories_scanned += 1;
            let walker = WalkDir::new(dir);

            for entry in walker.into_iter().filter_map(|e| e.ok()) {
                if entry.file_type().is_file() {
                    let path = entry.path().to_path_buf();
                    if Self::is_audio_path(&path) {
                        if let Ok(scanned) = ScannedTrack::from_path(path.clone()) {
                            if scanned.is_audio_file() {
                                bytes_scanned += scanned.size;
                                tracks.push(scanned);
                            }
                        }
                    }
                }
            }
        }

        ScanResult {
            tracks,
            directories_scanned,
            bytes_scanned,
        }
    }

    /// Check if a path is an audio file by extension
    fn is_audio_path(path: &Path) -> bool {
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            return AUDIO_EXTENSIONS.contains(&ext);
        }
        false
    }

    /// Create a future that yields track titles from a directory
    pub fn scan_directory_future(
        &self,
        dir: PathBuf,
    ) -> impl std::future::Future<Output = Result<ScanResult>> + '_ {
        async move {
            let result = self.scan_directory(&dir);
            Ok(result)
        }
    }

    /// Watch for file system changes and yield events as they occur
    pub fn watch_changes(
        &self,
    ) -> Result<impl Stream<Item = FileEvent>> {
        let (tx, rx) = mpsc::channel(64);
        let dirs = self.watched_dirs.clone();

        // Create the watcher
        let mut watcher: RecommendedWatcher = notify::recommended_watcher(move |res| {
            if let Ok(event) = res {
                let _ = tx.try_send(FileEvent::from(event));
            }
        }).map_err(|e| anyhow::anyhow!("Failed to create watcher: {}", e))?;

        // Watch all directories
        for dir in dirs.read().iter() {
            if dir.exists() {
                watcher.watch(dir, notify::RecursiveMode::Recursive)
                    .map_err(|e| anyhow::anyhow!("Failed to watch directory {:?}: {}", dir, e))?;
            }
        }

        Ok(ReceiverStream::new(rx))
    }
}

impl Default for AudioScanner {
    fn default() -> Self {
        Self::new()
    }
}

/// Stream of audio tracks from directory scanning
pub struct TrackStream {
    receiver: mpsc::Receiver<ScannedTrack>,
}

impl TrackStream {
    /// Create a new track stream from a single directory
    pub fn from_directory(dir: PathBuf) -> Self {
        let (tx, rx) = mpsc::channel(64);

        tokio::spawn(async move {
            if dir.exists() {
                let walker = WalkDir::new(&dir);
                for entry in walker.into_iter().filter_map(|e| e.ok()) {
                    if entry.file_type().is_file() {
                        let path = entry.path().to_path_buf();
                        if Self::is_audio_path(&path) {
                            if let Ok(track) = ScannedTrack::from_path(path.clone()) {
                                if track.is_audio_file() {
                                    let _ = tx.send(track).await;
                                }
                            }
                        }
                    }
                }
            }
        });

        Self { receiver: rx }
    }

    fn is_audio_path(path: &Path) -> bool {
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            return AUDIO_EXTENSIONS.contains(&ext);
        }
        false
    }
}

impl Stream for TrackStream {
    type Item = ScannedTrack;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        std::pin::Pin::new(&mut self.receiver).poll_recv(cx)
    }
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

    fn is_audio_path(path: &Path) -> bool {
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            return AUDIO_EXTENSIONS.contains(&ext);
        }
        false
    }
}