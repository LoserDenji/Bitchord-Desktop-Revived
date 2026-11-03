//! Party synchronization protocol - client for BitChord party server
//!
//! Connects to the central party server to sync playback state with other users,
//! matching the Compose app behavior.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;
use uuid::Uuid;

/// Default party server - matches Compose's DEFAULT_SERVER
pub const DEFAULT_SERVER: &str = "https://bitchord.kushagrasingh.in";

/// Code length for party codes
pub const CODE_LENGTH: usize = 6;

/// Clean and normalize a party code
pub fn clean_code(code: &str) -> String {
    code.chars()
        .filter(|c| c.is_alphanumeric())
        .take(CODE_LENGTH)
        .collect::<String>()
        .to_uppercase()
}

/// Validate a party code
pub fn validate_code(code: &str) -> bool {
    let cleaned = clean_code(code);
    cleaned.len() == CODE_LENGTH && cleaned.chars().all(|c| c.is_alphanumeric())
}

/// Connect to a party session
pub async fn connect_to_session(session_id: &str, server: &str, token: &str) -> Result<()> {
    let _url = format!("{}/ws/parties/{}", 
        server.replace("https://", "wss://").replace("http://", "ws://"), 
        session_id);
    
    // TODO: Implement WebSocket connection with Authorization header
    // Handles: welcome, pong, state, queue, members, activity, error, bye
    
    Ok(())
}

/// Broadcast a message to all session participants
pub async fn broadcast_message(session_id: &str, msg: SyncMessage) -> Result<()> {
    // TODO: Send via authenticated WebSocket
    let _ = session_id;
    let _ = msg;
    Ok(())
}

/// Handle incoming WebSocket message
pub fn handle_incoming(msg: &str) -> Option<SyncMessage> {
    serde_json::from_str::<serde_json::Value>(msg).ok().and_then(|json| {
        let msg_type = json.get("type")?.as_str()?;
        match msg_type {
            "state" => json.get("playback").cloned().map(|p| SyncMessage::State { playback: p }),
            "queue" => json.get("queue").cloned().map(|q| SyncMessage::Queue { queue: q }),
            "members" => {
                let members = json.get("members")?.clone();
                let max = json.get("maxMembers")?.as_i64()?.try_into().ok()?;
                let host_only = json.get("hostOnlyControl")?.as_bool()?.unwrap_or(false);
                Some(SyncMessage::Members { members, max_members: max, host_only_control: host_only })
            },
            _ => None,
        }
    })
}

/// Sync message types (matching server protocol)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "content")]
pub enum SyncMessage {
    Welcome { snapshot: serde_json::Value, server_ms: i64 },
    Pong { client_ms: i64, server_ms: i64 },
    State { #[serde(flatten)] playback: serde_json::Value },
    Queue { #[serde(flatten)] queue: serde_json::Value },
    Members { members: serde_json::Value, max_members: i32, host_only_control: bool },
    Activity { action: String, by: String, at_ms: i64, detail: String },
    Error { error: String, message: Option<String> },
    Bye { reason: Option<String> },
}

/// Control action types (matching server protocol)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action")]
pub enum ControlAction {
    Play { position_ms: Option<i64> },
    Pause { position_ms: Option<i64> },
    Seek { position_ms: i64 },
    Next,
    Previous,
    SetTrack { track: serde_json::Value, position_ms: i64, is_playing: bool },
    SetQueue { queue: serde_json::Value, queue_index: i32 },
    QueueAdd { tracks: serde_json::Value, play_next: bool },
    QueueRemove { video_id: String },
    QueueClear,
    QueueMove { from_index: i32, to_index: i32, video_id: Option<String> },
    SetMaxMembers { max_members: i32 },
    Kick { member_id: String },
    SetAutoplay { enabled: bool },
    SetHostOnlyControl { enabled: bool },
    SyncQueue,
}

impl ControlAction {
    pub fn play() -> Self { Self::Play { position_ms: None } }
    pub fn pause() -> Self { Self::Pause { position_ms: None } }
    pub fn next() -> Self { Self::Next }
    pub fn previous() -> Self { Self::Previous }
    pub fn seek(position_ms: i64) -> Self { Self::Seek { position_ms } }
    pub fn vote_skip() -> Self { Self::SyncQueue }
}

/// Create an invite URL from a party code
pub fn create_invite_url(code: &str) -> String {
    format!("{}/invite/{}", DEFAULT_SERVER, clean_code(code))
}

/// Parse party code from an invite URL
pub fn parse_invite_url(url: &str) -> Option<String> {
    url.strip_suffix('/')?
        .split('/invite/')?.last()?
        .to_uppercase();
    if validate_code(&url.split('/invite/')?.last()?) {
        Some(url.split('/invite/')?.last()?.to_uppercase())
    } else {
        None
    }
}

/// Party session state
#[derive(Debug, Clone, Default)]
pub struct SyncSession {
    pub session_id: String,
    pub host_client_id: String,
    pub participants: HashMap<String, Participant>,
    pub current_track: Option<String>,
    pub playback_state: PlaybackState,
}

/// Party participant
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Participant {
    pub client_id: String,
    pub display_name: String,
    pub ready: bool,
}

impl Participant {
    pub fn new(client_id: String, display_name: String) -> Self {
        Self { client_id, display_name, ready: false }
    }
}

/// Playback state for sync
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PlaybackState {
    pub position_ms: u64,
    pub is_playing: bool,
    pub speed: f32,
}

/// Sync manager for handling multiple sessions
pub struct SyncManager {
    pub sessions: Arc<Mutex<HashMap<String, SyncSession>>>,
}

impl SyncManager {
    pub fn new() -> Self {
        Self { sessions: Arc::new(Mutex::new(HashMap::new())) }
    }

    pub async fn create_session(&self, host_client_id: String) -> String {
        let session_id = Uuid::new_v4().to_string();
        let mut sessions = self.sessions.lock().await;
        sessions.insert(session_id.clone(), SyncSession::new(session_id, host_client_id));
        session_id
    }

    pub async fn join_session(&self, session_id: &str) -> Result<()> {
        let sessions = self.sessions.lock().await;
        if sessions.contains_key(session_id) {
            Ok(())
        } else {
            Err(anyhow::anyhow!("Session not found"))
        }
    }

    pub async fn leave_session(&self, session_id: &str) -> Result<()> {
        let mut sessions = self.sessions.lock().await;
        sessions.remove(session_id);
        Ok(())
    }

    pub async fn broadcast_message(&self, session_id: &str, msg: SyncMessage) -> Result<()> {
        let sessions = self.sessions.lock().await;
        if sessions.contains_key(session_id) {
            broadcast_message(session_id, msg).await
        } else {
            Err(anyhow::anyhow!("Session not found"))
        }
    }
}

impl SyncSession {
    pub fn new(session_id: String, host_client_id: String) -> Self {
        Self {
            session_id,
            host_client_id,
            participants: HashMap::new(),
            current_track: None,
            playback_state: PlaybackState::default(),
        }
    }

    pub fn add_participant(&mut self, participant: Participant) {
        self.participants.insert(participant.client_id.clone(), participant);
    }

    pub fn remove_participant(&mut self, client_id: &str) {
        self.participants.remove(client_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clean_code() {
        assert_eq!(clean_code("abc123"), "ABC123");
        assert_eq!(clean_code("  abc def  "), "ABCDEF");
        assert_eq!(clean_code("ab!cd"), "ABCD");
    }

    #[test]
    fn test_validate_code() {
        assert!(validate_code("ABC123"));
        assert!(validate_code("abc123"));
        assert!(!validate_code("ab"));
        assert!(!validate_code("ab!cde"));
    }

    #[test]
    fn test_create_invite_url() {
        assert_eq!(create_invite_url("abc123"), "https://bitchord.kushagrasingh.in/invite/ABC123");
    }

    #[test]
    fn test_parse_invite_url() {
        assert_eq!(parse_invite_url("https://bitchord.kushagrasingh.in/invite/ABC123"), Some("ABC123".to_string()));
        assert_eq!(parse_invite_url("invalid"), None);
    }

    #[test]
    fn test_control_action_serialization() {
        let play = ControlAction::play();
        let json = serde_json::to_string(&play).unwrap();
        assert!(json.contains("\"action\":\"play\""));

        let seek = ControlAction::seek(12345);
        let json = serde_json::to_string(&seek).unwrap();
        assert!(json.contains("\"positionMs\":12345"));
    }
}
