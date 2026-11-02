//! Listen Together (Party Mode) models — ported from Kotlin PartyModels.kt, JamInvite.kt.
use serde::{Deserialize, Serialize};

/// The public invite URL for a party, and the only shape that is accepted back.
pub mod jam_invite {
    pub const ORIGIN: &str = "https://bitchord.kushagrasingh.in";
    pub const CODE_LENGTH: usize = 6;

    /// Parse and normalize a party code from an invite URL.
    pub fn parse(value: Option<&str>) -> Option<String> {
        let text = value?.trim();
        let prefix = format!("{}/invite/", ORIGIN);
        if !text.to_lowercase().starts_with(&prefix.to_lowercase()) {
            return None;
        }
        let after_prefix = &text[prefix.len()..];
        let code = after_prefix
            .split('?')
            .next()
            .unwrap_or("")
            .split('#')
            .next()
            .unwrap_or("");
        if code.len() != CODE_LENGTH || !code.chars().all(|c| c.is_alphanumeric()) {
            return None;
        }
        Some(code.to_uppercase())
    }

    pub fn url(code: &str) -> String {
        format!("{}/invite/{}", ORIGIN, code.to_uppercase())
    }
}

/// A song as the party knows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartyTrack {
    pub video_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
    #[serde(default)]
    pub from_autoplay: bool,
}

/// One signed-in device in the party.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartyMember {
    pub member_id: String,
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
    #[serde(default)]
    pub is_host: bool,
    #[serde(default)]
    pub connected: bool,
    #[serde(default)]
    pub joined_at_ms: i64,
    #[serde(default)]
    pub last_seen_ms: i64,
}

/// Where the party is, as of a server timestamp.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PartyPlayback {
    #[serde(default)]
    pub seq: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<PartyTrack>,
    #[serde(default)]
    pub queue_seq: i64,
    #[serde(default)]
    pub queue_length: i32,
    #[serde(default)]
    pub queue_index: i32,
    #[serde(default)]
    pub is_playing: bool,
    #[serde(default)]
    pub position_ms: i64,
    #[serde(default)]
    pub anchor_ms: i64,
    #[serde(default)]
    pub effective_position_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_by_name: Option<String>,
    #[serde(default)]
    pub autoplay_enabled: bool,
    #[serde(default)]
    pub updated_at_ms: i64,
}

/// The party's running order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PartyQueue {
    #[serde(default)]
    pub seq: i64,
    #[serde(default)]
    pub index: i32,
    #[serde(default)]
    pub items: Vec<PartyTrack>,
}

/// Full party snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartySnapshot {
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub created_at_ms: i64,
    #[serde(default)]
    pub max_members: i32,
    #[serde(default)]
    pub host_only_control: bool,
    #[serde(default)]
    pub members: Vec<PartyMember>,
    #[serde(default)]
    pub playback: PartyPlayback,
    #[serde(default)]
    pub queue: PartyQueue,
    #[serde(default)]
    pub server_ms: i64,
}

/// Who is in a party, for somebody who has not joined it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartyPreview {
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub host_name: String,
    #[serde(default)]
    pub member_count: i32,
    #[serde(default)]
    pub max_members: i32,
    #[serde(default)]
    pub is_full: bool,
    #[serde(default)]
    pub members: Vec<PartyPreviewMember>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartyPreviewMember {
    #[serde(default)]
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
    #[serde(default)]
    pub is_host: bool,
}

/// The answer to a create or a join.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartyMembership {
    pub code: String,
    pub token: String,
    pub you: PartyMember,
    pub party: PartySnapshot,
    #[serde(default)]
    pub server_ms: i64,
}

/// A compact local-only record of a live party action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartyActivity {
    pub action: String,
    pub by: String,
    pub at_ms: i64,
    #[serde(default)]
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinRequest {
    pub user_id: String,
    pub device_id: String,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_members: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autoplay_enabled: Option<bool>,
}

/// API error response from party server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiError {
    #[serde(rename = "error")]
    pub code: String,
    pub message: String,
}

/// An invite to join a private party.
/// From Kotlin JamInvite.kt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JamInvite {
    /// The 6-character alphanumeric invite code.
    pub code: String,
    /// The full URL for joining.
    #[serde(skip_serializing)]
    pub url: Option<String>,
}

impl JamInvite {
    /// Create a new JamInvite from a code.
    pub fn new(code: impl Into<String>) -> Self {
        let code = code.into().to_uppercase();
        Self {
            code: code.clone(),
            url: Some(jam_invite::url(&code)),
        }
    }
    
    /// Create a JamInvite from a URL string.
    pub fn from_url(url: &str) -> Option<Self> {
        let code = jam_invite::parse(Some(url))?;
        Some(Self::new(code))
    }
    
    /// Validate that the code is properly formatted.
    pub fn is_valid(&self) -> bool {
        self.code.len() == jam_invite::CODE_LENGTH 
            && self.code.chars().all(|c| c.is_alphanumeric())
    }
}
