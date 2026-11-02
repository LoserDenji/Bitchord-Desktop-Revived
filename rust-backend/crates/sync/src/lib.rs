//! Party synchronization protocol
use bitchord_core::TrackId;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// WebSocket-based party sync protocol
pub struct PartySync {
    sessions: HashMap<String, PartySession>,
}

#[derive(Debug, Clone)]
pub struct PartySession {
    session_id: String,
    participants: Vec<Participant>,
    current_track: Option<TrackId>,
    playback_state: PlaybackState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Participant {
    pub client_id: String,
    pub display_name: String,
    pub ready: bool,
    pub last_seen: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaybackState {
    pub position_ms: u64,
    pub is_playing: bool,
    pub speed: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncMessage {
    pub event: SyncEvent,
    pub payload: serde_json::Value,
    pub sender: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncEvent {
    TrackChanged { track_id: TrackId },
    PositionUpdate { position_ms: u64 },
    PlaybackStateChanged { is_playing: bool },
    ParticipantJoined,
    ParticipantLeft,
}

impl PartySync {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
        }
    }
    
    pub async fn join_session(&mut self, session_id: &str, participant: Participant) -> Result<()> {
        self.sessions.entry(session_id.to_string()).or_insert_with(|| {
            PartySession {
                session_id: session_id.to_string(),
                participants: Vec::new(),
                current_track: None,
                playback_state: PlaybackState {
                    position_ms: 0,
                    is_playing: false,
                    speed: 1.0,
                },
            }
        }).participants.push(participant);
        Ok(())
    }
}

impl Default for PartySync {
    fn default() -> Self {
        Self::new()
    }
}