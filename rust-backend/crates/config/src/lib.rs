//! Configuration management
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use bitchord_core::UserSettings;

/// Application configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    /// User settings
    pub settings: UserSettings,
    
    /// Last database path used
    pub last_db_path: Option<PathBuf>,
    
    /// Recently played tracks
    pub recent_tracks: Vec<uuid::Uuid>,
    
    /// API keys for streaming services
    pub api_keys: ApiKeys,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ApiKeys {
    pub spotify_client_id: Option<String>,
    pub spotify_client_secret: Option<String>,
}

/// Configuration manager
pub struct ConfigManager {
    config_path: PathBuf,
    config: Config,
}

impl ConfigManager {
    pub fn new() -> anyhow::Result<Self> {
        let config_path = Self::get_config_path()?;
        let config = if config_path.exists() {
            Self::load(&config_path)?
        } else {
            Config::default()
        };
        Ok(Self { config_path, config })
    }
    
    fn get_config_path() -> anyhow::Result<PathBuf> {
        let config_dir = directories::ProjectDirs::from("com", "bitchord", "BitChord")
            .ok_or_else(|| anyhow::anyhow!("Cannot determine config directory"))?
            .config_dir()
            .to_path_buf();
        
        std::fs::create_dir_all(&config_dir)?;
        Ok(config_dir.join("config.json"))
    }
    
    fn load(path: &PathBuf) -> anyhow::Result<Config> {
        let contents = std::fs::read_to_string(path)?;
        let config: Config = serde_json::from_str(&contents)?;
        Ok(config)
    }
    
    pub fn save(&self) -> anyhow::Result<()> {
        let contents = serde_json::to_string_pretty(&self.config)?;
        std::fs::write(&self.config_path, contents)?;
        Ok(())
    }
    
    pub fn settings(&self) -> &UserSettings {
        &self.config.settings
    }
    
    pub fn settings_mut(&mut self) -> &mut UserSettings {
        &mut self.config.settings
    }
}