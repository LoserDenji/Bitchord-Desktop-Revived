//! Audio format enumeration
use serde::{Deserialize, Serialize};

/// Audio codec/format specification
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AudioFormat {
    /// MP3 audio
    Mp3,
    /// FLAC lossless
    Flac,
    /// OGG Vorbis
    Ogg,
    /// WAV PCM
    Wav,
    /// AAC (M4A)
    Aac,
    /// Opus
    Opus,
    /// WMA
    Wma,
    /// ALAC (Apple Lossless, M4A)
    Alac,
    /// Unknown/unsupported
    Unknown,
}

impl std::fmt::Display for AudioFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AudioFormat::Mp3 => write!(f, "MP3"),
            AudioFormat::Flac => write!(f, "FLAC"),
            AudioFormat::Ogg => write!(f, "OGG"),
            AudioFormat::Wav => write!(f, "WAV"),
            AudioFormat::Aac => write!(f, "AAC"),
            AudioFormat::Opus => write!(f, "Opus"),
            AudioFormat::Wma => write!(f, "WMA"),
            AudioFormat::Alac => write!(f, "ALAC"),
            AudioFormat::Unknown => write!(f, "Unknown"),
        }
    }
}

/// Container format specification
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContainerFormat {
    /// MP4 container (M4A, MP4, MOV)
    Mp4,
    /// Matroska (MKV, MKA)
    Matroska,
    /// OGG container
    Ogg,
    /// WAV container
    Wav,
    /// WebM
    WebM,
    /// AVI
    Avi,
    ///Unknown
    Unknown,
}