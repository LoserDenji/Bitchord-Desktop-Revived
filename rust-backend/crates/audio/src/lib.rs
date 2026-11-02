//! Audio playback engine and library scanner/watcher tools

use anyhow::Result;
use parking_lot::RwLock;
use std::sync::Arc;
use std::path::PathBuf;

use rodio::{Decoder, Source, OutputStream, Sink};
use uuid::Uuid;

pub mod scanner;
pub mod watcher;

pub use scanner::AudioScanner;
pub use watcher::AudioWatcher;

/// Basic decoder type alias for simplicity
pub type AudioSource = Box<dyn rodio::AudioSource<Items = Box<dyn Source<Error = (), Item = f32>> + Send + Sync>>;

/// Audio decoder trait for format abstraction
#[async_trait::async_trait]
pub trait AudioDecoder: Send + Sync {
    async fn decode(&self, path: &PathBuf) -> Result<AudioSource>;
}

/// Basic decoder implementation using rodio's built-in support
pub struct FFmpegDecoder;

impl FFmpegDecoder {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl AudioDecoder for FFmpegDecoder {
    async fn decode(&self, path: &PathBuf) -> Result<AudioSource> {
        let file = std::fs::File::open(path)?;
        let source = std::io::BufReader::new(file);
        let decoder = Decoder::new(source).map_err(|e| anyhow::anyhow!("Decode error: {}", e))?;
        Ok(Box::new(decoder))
    }
}

impl Default for FFmpegDecoder {
    fn default() -> Self {
        Self::new()
    }
}

/// Audio state shared between components
#[derive(Debug, Clone, Default)]
pub struct AudioState {
    pub is_playing: bool,
    pub position_ms: u64,
    pub volume: f32,
    pub speed: f32,
    pub current_track: Option<Uuid>,
}

/// Rodio-based audio engine
pub struct AudioEngine {
    state: Arc<RwLock<AudioState>>,
    decoder: FFmpegDecoder,
    #[allow(dead_code)]
    output: Option<OutputStream>,
    sink: Option<Sink>,
    current_file: Option<PathBuf>,
    position_ms: u64,
}

impl AudioEngine {
    pub fn new() -> Result<Self> {
        let (output, device) = OutputStream::try_default()
            .map_err(|e| anyhow::anyhow!("Failed to open audio device: {}", e))?;
        
        let sink = Sink::try_new(&device)
            .map_err(|e| anyhow::anyhow!("Failed to create audio sink: {}", e))?;
        
        Ok(Self {
            state: Arc::new(RwLock::new(AudioState::default())),
            decoder: FFmpegDecoder::new(),
            output: Some(output),
            sink: Some(sink),
            current_file: None,
            position_ms: 0,
        })
    }

    pub fn with_state(state: Arc<RwLock<AudioState>>) -> Result<Self> {
        let (output, device) = OutputStream::try_default()
            .map_err(|e| anyhow::anyhow!("Failed to open audio device: {}", e))?;
        
        let sink = Sink::try_new(&device)
            .map_err(|e| anyhow::anyhow!("Failed to create audio sink: {}", e))?;
        
        Ok(Self {
            state,
            decoder: FFmpegDecoder::new(),
            output: Some(output),
            sink: Some(sink),
            current_file: None,
            position_ms: 0,
        })
    }

    pub fn get_state(&self) -> Arc<RwLock<AudioState>> {
        Arc::clone(&self.state)
    }

    pub fn play_file(&mut self, path: PathBuf) -> Result<()> {
        if let Some(sink) = &self.sink {
            sink.append(self.decoder.decode_blocking(&path)?);
            sink.play();
            
            let mut state = self.state.write();
            state.is_playing = true;
            self.current_file = Some(path);
        }
        Ok(())
    }

    pub fn pause(&self) -> Result<()> {
        if let Some(sink) = &self.sink {
            sink.pause();
        }
        let mut state = self.state.write();
        state.is_playing = false;
        Ok(())
    }

    pub fn resume(&self) -> Result<()> {
        if let Some(sink) = &self.sink {
            sink.play();
        }
        let mut state = self.state.write();
        state.is_playing = true;
        Ok(())
    }

    pub fn stop(&self) -> Result<()> {
        if let Some(sink) = &self.sink {
            sink.stop();
        }
        let mut state = self.state.write();
        state.is_playing = false;
        state.position_ms = 0;
        self.current_file = None;
        Ok(())
    }

    pub fn set_volume(&self, volume: f32) -> Result<()> {
        if volume < 0.0 || volume > 1.0 {
            return Err(anyhow::anyhow!("Volume must be between 0.0 and 1.0"));
        }
        if let Some(sink) = &self.sink {
            sink.set_volume(volume);
        }
        let mut state = self.state.write();
        state.volume = volume;
        Ok(())
    }

    pub fn set_speed(&self, speed: f32) -> Result<()> {
        if let Some(sink) = &self.sink {
            sink.set_speed(speed);
        }
        let mut state = self.state.write();
        state.speed = speed;
        Ok(())
    }

    pub fn get_position(&self) -> u64 {
        self.position_ms
    }

    pub fn seek(&mut self, position_ms: u64) -> Result<()> {
        if let Some(sink) = &self.sink {
            sink.stop();
        }
        self.position_ms = position_ms;
        let mut state = self.state.write();
        state.position_ms = position_ms;
        Ok(())
    }

    pub fn is_playing(&self) -> bool {
        self.state.read().is_playing
    }

    pub fn get_volume(&self) -> f32 {
        self.state.read().volume
    }
}

impl Default for AudioEngine {
    fn default() -> Self {
        Self::new().expect("Failed to create AudioEngine")
    }
}

/// Trait for async decoding
trait AsyncDecode {
    fn decode_blocking(&self, path: &PathBuf) -> Result<AudioSource>;
}

impl AsyncDecode for FFmpegDecoder {
    fn decode_blocking(&self, path: &PathBuf) -> Result<AudioSource> {
        let file = std::fs::File::open(path)?;
        let source = std::io::BufReader::new(file);
        let decoder = Decoder::new(source).map_err(|e| anyhow::anyhow!("Decode error: {}", e))?;
        Ok(Box::new(decoder))
    }
}

/// Audio player trait for async playback control
#[async_trait::async_trait]
pub trait AudioPlayer: Send + Sync {
    async fn play(&self, track_id: Uuid) -> Result<()>;
    async fn pause(&self) -> Result<()>;
    async fn stop(&self) -> Result<()>;
    async fn next(&self) -> Result<()>;
    async fn previous(&self) -> Result<()>;
    async fn seek(&self, position_ms: u64) -> Result<()>;
    async fn set_volume(&self, volume: f32) -> Result<()>;
    async fn set_speed(&self, speed: f32) -> Result<()>;
    async fn get_position(&self) -> Result<u64>;
    async fn is_playing(&self) -> Result<bool>;
}

/// Mock audio player for testing
pub struct MockAudioPlayer;

#[async_trait::async_trait]
impl AudioPlayer for MockAudioPlayer {
    async fn play(&self, _track_id: Uuid) -> Result<()> {
        Ok(())
    }
    async fn pause(&self) -> Result<()> {
        Ok(())
    }
    async fn stop(&self) -> Result<()> {
        Ok(())
    }
    async fn next(&self) -> Result<()> {
        Ok(())
    }
    async fn previous(&self) -> Result<()> {
        Ok(())
    }
    async fn seek(&self, _position_ms: u64) -> Result<()> {
        Ok(())
    }
    async fn set_volume(&self, _volume: f32) -> Result<()> {
        Ok(())
    }
    async fn set_speed(&self, _speed: f32) -> Result<()> {
        Ok(())
    }
    async fn get_position(&self) -> Result<u64> {
        Ok(0)
    }
    async fn is_playing(&self) -> Result<bool> {
        Ok(false)
    }
}
