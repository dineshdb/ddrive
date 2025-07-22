pub mod checksum;
pub mod cli;
pub mod config;
pub mod database;
pub mod error;
pub mod repository;
pub mod scanner;
pub mod utils;

pub use error::{DdriveError, Result};

/// Application context that holds shared state
#[derive(Clone)]
pub struct AppContext {
    pub database: database::Database,
    pub repo_root: std::path::PathBuf,
    pub config: config::Config,
}

impl AppContext {
    pub async fn new(repo_root: std::path::PathBuf) -> Result<Self> {
        let db_path = repo_root.join(".ddrive").join("metadata.sqlite3");
        let database_url = format!("sqlite://{}", db_path.display());
        let database = database::Database::new(&database_url, repo_root.clone()).await?;

        let config = config::Config::load(&repo_root)?;

        Ok(Self {
            database,
            repo_root,
            config,
        })
    }

    /// Get a reference to the database
    pub fn database(&self) -> &database::Database {
        &self.database
    }

    /// Get the repository root path
    pub fn repo_root(&self) -> &std::path::Path {
        &self.repo_root
    }

    /// Get configuration
    pub fn config(&self) -> &config::Config {
        &self.config
    }

    /// Reload configuration from disk
    pub fn reload_config(&mut self) -> Result<()> {
        self.config = config::Config::load(&self.repo_root)?;
        Ok(())
    }
}
