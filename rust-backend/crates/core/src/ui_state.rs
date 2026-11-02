//! UI State sealed interface — ported from Kotlin `UiState<out T>`.
use serde::{Deserialize, Serialize};

/// Sealed interface for representing loading, success, or error states.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "data", untyped)]
pub enum UiState<T> {
    #[serde(rename = "LOADING")]
    Loading,
    #[serde(rename = "SUCCESS")]
    Success(T),
    #[serde(rename = "ERROR")]
    Error(String),
}

impl<T> UiState<T> {
    pub fn loading() -> Self {
        UiState::Loading
    }

    pub fn success(data: T) -> Self {
        UiState::Success(data)
    }

    pub fn error(message: impl Into<String>) -> Self {
        UiState::Error(message.into())
    }
}

impl<T: Clone> UiState<T> {
    pub fn map<U, F: FnOnce(T) -> U>(self, f: F) -> UiState<U> {
        match self {
            UiState::Loading => UiState::Loading,
            UiState::Success(data) => UiState::Success(f(data)),
            UiState::Error(e) => UiState::Error(e),
        }
    }

    pub fn as_ref(&self) -> UiState<&T> {
        match self {
            UiState::Loading => UiState::Loading,
            UiState::Success(data) => UiState::Success(data),
            UiState::Error(e) => UiState::Error(e.clone()),
        }
    }
}
