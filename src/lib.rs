pub mod checksum;
pub mod cli;
pub mod config;
pub mod database;
pub mod error;
pub mod repository;
pub mod scanner;
pub mod utils;

use crate::repository::Repository;
pub use error::{DdriveError, Result};

/// Application context that holds shared state
#[derive(Clone)]
pub struct AppContext {
    pub database: database::Database,
    pub repo: Repository,
    pub config: config::Config,
}

impl AppContext {
    pub async fn new(repo: Repository) -> Result<Self> {
        let db_path = repo.root().join(".ddrive").join("metadata.sqlite3");
        let database_url = format!("sqlite://{}", db_path.display());
        let database = database::Database::new(&database_url, repo.root().clone()).await?;

        let config = config::Config::load(repo.root())?;

        Ok(Self {
            database,
            repo,
            config,
        })
    }

    /// Get a reference to the database
    pub fn database(&self) -> &database::Database {
        &self.database
    }
}
