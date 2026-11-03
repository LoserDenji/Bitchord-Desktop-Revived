//! Party synchronization protocol
//!
//! WebSocket-based sync system for multi-user party mode.
//! Host creates sessions, participants join and broadcast playback state.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, broadcast};
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;
use chrono::{DateTime, Utc};
use bitchord_core::TrackId;

/// WebSocket-based party sync protocol
pub struct WebSocketServer {
    sessions: HashMap<String, SyncSession>,
    port: u16,
}

impl WebSocketServer {
    pub fn new(port: u16) -> Self {
        Self {
            sessions: HashMap::new(),
            port,
        }
    }

    pub async fn start(&mut self) -> Result<()> {
        Ok(())
    }
}

impl Default for WebSocketServer {
    fn default() -> Self {
        Self::new(8443)
    }
}

#[derive(Debug, Clone)]
pub struct SyncSession {
    pub session_id: String,
    pub host_client_id: String,
    pub participants: HashMap<String, Participant>,
    pub current_track: Option<TrackId>,
    pub playback_state: PlaybackState,
    pub tx: broadcast::Sender<SyncMessage>,
}

impl SyncSession {
    pub fn new(session_id: String, host_client_id: String) -> Self {
        let (tx, _rx) = broadcast::channel(64);
        Self {
            session_id,
            host_client_id,
            participants: HashMap::new(),
            current_track: None,
            playback_state: PlaybackState::default(),
            tx,
        }
    }

    pub fn add_participant(&mut self, participant: Participant) {
        self.participants.insert(participant.client_id.clone(), participant);
    }

    pub fn remove_participant(&mut self, client_id: &str) {
        self.participants.remove(client_id);
    }

    pub async fn broadcast(&self, msg: SyncMessage) -> Result<()> {
        self.tx.send(msg).map_err(|e| anyhow::anyhow!("{}", e))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Participant {
    pub client_id: String,
    pub display_name: String,
    pub ready: bool,
    pub last_seen: DateTime<Utc>,
}

impl Participant {
    pub fn new(client_id: String, display_name: String) -> Self {
        Self {
            client_id,
            display_name,
            ready: false,
            last_seen: Utc::now(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PlaybackState {
    pub position_ms: u64,
    pub is_playing: bool,
    pub speed: f32,
    pub current_track: Option<TrackId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncMessage {
    pub event: SyncEvent,
    pub payload: serde_json::Value,
    pub sender: String,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncEvent {
    Play,
    Pause,
    Next,
    Prev,
    SetPosition { position_ms: u64 },
    VoteSkip,
    TrackChanged { track_id: TrackId },
    ParticipantJoined { participant: Participant },
    ParticipantLeft { client_id: String },
    SyncState { session_id: String },
}

/// WebSocket client for joining sync sessions
pub struct WebSocketClient {
    pub client_id: String,
    pub session_id: Option<String>,
    reconnect_attempts: u32,
}

impl WebSocketClient {
    pub fn new() -> Self {
        Self {
            client_id: Uuid::new_v4().to_string(),
            session_id: None,
            reconnect_attempts: 0,
        }
    }

    pub fn with_client_id(client_id: String) -> Self {
        Self {
            client_id,
            session_id: None,
            reconnect_attempts: 0,
        }
    }

    pub async fn connect(&mut self, session_id: &str, _host: &str) -> Result<()> {
        self.session_id = Some(session_id.to_string());
        Ok(())
    }

    pub async fn heartbeat(&self) -> Result<()> {
        Ok(())
    }

    pub async fn broadcast(&self, _event: SyncEvent, _payload: serde_json::Value) -> Result<()> {
        Ok(())
    }
}

impl Default for WebSocketClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Sync manager that handles sessions and participants
pub struct SyncManager {
    pub sessions: Arc<Mutex<HashMap<String, SyncSession>>>,
    pub clients: Arc<Mutex<HashMap<String, WebSocketClient>>>,
}

impl SyncManager {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            clients: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn create_session(&self, host_client_id: String) -> String {
        let session_id = Uuid::new_v4().to_string();
        let mut sessions = self.sessions.lock().await;
        sessions.insert(session_id.clone(), SyncSession::new(session_id.clone(), host_client_id));
        session_id
    }

    pub async fn join_session(&self, session_id: &str, participant: Participant) -> Result<()> {
        let mut sessions = self.sessions.lock().await;
        if let Some(session) = sessions.get_mut(session_id) {
            session.add_participant(participant);
            Ok(())
        } else {
            Err(anyhow::anyhow!("Session not found"))
        }
    }

    pub async fn leave_session(&self, session_id: &str, client_id: &str) -> Result<()> {
        let mut sessions = self.sessions.lock().await;
        if let Some(session) = sessions.get_mut(session_id) {
            session.remove_participant(client_id);
        }
        Ok(())
    }

    pub async fn broadcast_message(&self, session_id: &str, msg: SyncMessage) -> Result<()> {
        let sessions = self.sessions.lock().await;
        if let Some(session) = sessions.get(session_id) {
            session.broadcast(msg).await
        } else {
            Err(anyhow::anyhow!("Session not found"))
        }
    }
}

impl Default for SyncManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_session() {
        let manager = SyncManager::new();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let session_id = rt.block_on(manager.create_session("host1".to_string()));
        assert!(!session_id.is_empty());
    }

    #[test]
    fn test_add_participant() {
        let manager = SyncManager::new();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let session_id = rt.block_on(manager.create_session("host1".to_string()));
        let participant = Participant::new("client2".to_string(), "User2".to_string());
        let result = rt.block_on(manager.join_session(&session_id, participant));
        assert!(result.is_ok());
    }
}
