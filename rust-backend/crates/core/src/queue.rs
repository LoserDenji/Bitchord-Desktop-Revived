//! Playback queue management — ported from Kotlin `QueueBuilder`, `QueueSections`, `QueueTimeline`.
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// A playback queue that manages the order of tracks to play.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackQueue {
    tracks: VecDeque<QueuedItem>,
    current_position: Option<usize>,
    prev_history: VecDeque<String>,
    max_history: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueuedItem {
    /// Video ID of the track in YouTube Music format.
    pub video_id: String,
    pub title: String,
    pub artist: String,
    pub thumbnail_url: Option<String>,
    /// Duration in milliseconds.
    pub duration_ms: Option<u64>,
    /// Whether this track is from autoplay (vs user/context queue).
    #[serde(default)]
    pub from_autoplay: bool,
    pub position: usize,
    pub added_at: chrono::DateTime<chrono::Utc>,
}

impl Default for PlaybackQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl PlaybackQueue {
    pub fn new() -> Self {
        Self {
            tracks: VecDeque::new(),
            current_position: None,
            prev_history: VecDeque::new(),
            max_history: 50,
        }
    }

    pub fn len(&self) -> usize {
        self.tracks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
    }

    pub fn current(&self) -> Option<&QueuedItem> {
        self.current_position.and_then(|pos| self.tracks.get(pos))
    }

    pub fn push_back(&mut self, video_id: String) {
        if !self.tracks.iter().any(|t| t.video_id == video_id) {
            let position = self.tracks.len();
            self.tracks.push_back(QueuedItem {
                video_id,
                title: String::new(),
                artist: String::new(),
                thumbnail_url: None,
                duration_ms: None,
                from_autoplay: false,
                position,
                added_at: chrono::Utc::now(),
            });
        }
    }

    pub fn pop_front(&mut self) -> Option<QueuedItem> {
        self.tracks.pop_front()
    }

    pub fn clear(&mut self) {
        self.tracks.clear();
        self.current_position = None;
        self.prev_history.clear();
    }

    pub fn move_to_front(&mut self, video_id: &str) {
        if let Some(pos) = self.tracks.iter().position(|t| t.video_id == video_id) {
            let item = self.tracks.remove(pos).unwrap();
            self.tracks.push_front(item);
        }
    }

    pub fn remove(&mut self, video_id: &str) -> bool {
        if let Some(pos) = self.tracks.iter().position(|t| t.video_id == video_id) {
            self.tracks.remove(pos);
            true
        } else {
            false
        }
    }

    fn advance_position(&mut self) {
        self.current_position = match self.current_position {
            Some(pos) if pos + 1 < self.tracks.len() => Some(pos + 1),
            _ => None,
        };
    }

    fn record_history(&mut self, video_id: String) {
        self.prev_history.push_back(video_id);
        if self.prev_history.len() > self.max_history {
            self.prev_history.pop_front();
        }
    }
}
