use crate::{DdriveError, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use tracing::debug;

/// Configuration for ddrive
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct Config {
    /// General configuration settings
    #[serde(default)]
    pub general: GeneralConfig,

    /// Verification settings
    #[serde(default)]
    pub verify: VerifyConfig,

    /// Prune settings
    #[serde(default)]
    pub prune: PruneConfig,

    /// Object store settings
    #[serde(default)]
    pub object_store: ObjectStoreConfig,
}

/// General configuration settings
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GeneralConfig {
    /// Enable verbose logging
    #[serde(default = "default_verbose")]
    pub verbose: bool,
}

/// Verification settings
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct VerifyConfig {
    /// Days between automatic checksum verification
    #[serde(default = "default_verify_interval")]
    pub interval_days: u32,
}

impl VerifyConfig {
    pub fn cutoff_date(&self) -> DateTime<Utc> {
        Utc::now() - Duration::days(self.interval_days as i64)
    }
}

/// Prune settings
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PruneConfig {
    /// Days to keep deleted files before pruning
    #[serde(default = "default_retention_days")]
    pub retention_days: u32,
}

impl PruneConfig {
    pub fn cutoff_date(&self) -> DateTime<Utc> {
        Utc::now() - Duration::days(self.retention_days as i64)
    }
}

/// Object store settings
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ObjectStoreConfig {
    /// Path to object store directory (relative to repository root)
    #[serde(default = "default_object_store_path")]
    pub path: String,
}

// Default values
fn default_verbose() -> bool {
    false
}

fn default_verify_interval() -> u32 {
    30 // 30 days between automatic checksum verification
}

fn default_retention_days() -> u32 {
    90 // 90 days retention for deleted files
}

fn default_object_store_path() -> String {
    ".ddrive/objects".to_string()
}

// Default implementations
impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            verbose: default_verbose(),
        }
    }
}

impl Default for VerifyConfig {
    fn default() -> Self {
        Self {
            interval_days: default_verify_interval(),
        }
    }
}

impl Default for PruneConfig {
    fn default() -> Self {
        Self {
            retention_days: default_retention_days(),
        }
    }
}

impl Default for ObjectStoreConfig {
    fn default() -> Self {
        Self {
            path: default_object_store_path(),
        }
    }
}

impl Config {
    /// Load configuration from file, or create default if it doesn't exist
    pub fn load(repo_root: &Path) -> Result<Self> {
        let config_path = repo_root.join(".ddrive").join("config.toml");

        if !config_path.exists() {
            debug!(
                "Config file not found, creating default at {}",
                config_path.display()
            );
            let default_config = Config::default();
            default_config.save(repo_root)?;
            return Ok(default_config);
        }

        let config_str = fs::read_to_string(&config_path).map_err(|e| DdriveError::FileSystem {
            message: format!("Failed to read config file: {e}"),
        })?;

        let config: Config =
            toml::from_str(&config_str).map_err(|e| DdriveError::Configuration {
                message: format!("Failed to parse config file: {e}"),
            })?;

        debug!("Loaded configuration from {}", config_path.display());
        Ok(config)
    }

    /// Save configuration to file
    pub fn save(&self, repo_root: &Path) -> Result<()> {
        let config_dir = repo_root.join(".ddrive");
        if !config_dir.exists() {
            fs::create_dir_all(&config_dir).map_err(|e| DdriveError::FileSystem {
                message: format!("Failed to create config directory: {e}"),
            })?;
        }

        let config_path = config_dir.join("config.toml");
        let config_str = toml::to_string_pretty(self).map_err(|e| DdriveError::Configuration {
            message: format!("Failed to serialize config: {e}"),
        })?;

        fs::write(&config_path, config_str).map_err(|e| DdriveError::FileSystem {
            message: format!("Failed to write config file: {e}"),
        })?;

        debug!("Configuration saved to {}", config_path.display());
        Ok(())
    }

    /// Get the absolute path to the object store
    pub fn object_store_path(&self, repo_root: &Path) -> PathBuf {
        repo_root.join(".ddrive").join("objects")
    }
}
