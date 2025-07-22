use crate::{DdriveError, Result};
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, info};

#[derive(Default)]
pub struct Repository {
    repo_root: PathBuf,
}

impl Repository {
    pub fn get_repo_root(&self) -> &PathBuf {
        &self.repo_root
    }

    /// Search for .ddrive/metadata.sqlite3 in given and parent directories
    pub fn find_repository(path: PathBuf) -> Result<Repository> {
        let mut search_path = path.as_path().canonicalize()?;
        loop {
            let ddrive_path = search_path.join(".ddrive");
            let db_path = ddrive_path.join("metadata.sqlite3");

            if db_path.exists() && db_path.is_file() {
                return Ok(Repository {
                    repo_root: search_path.to_path_buf().canonicalize()?,
                });
            }

            // Move to parent directory
            match search_path.parent() {
                Some(parent) => search_path = parent.into(),
                None => break, // Reached filesystem root
            }
        }

        Err(DdriveError::InvalidDirectory)
    }

    /// Validate that the repository has a valid database structure
    pub fn is_valid(&self) -> Result<bool> {
        let repo_path = self.repo_root.as_path();
        let ddrive_path = repo_path.join(".ddrive");
        let db_path = ddrive_path.join("metadata.sqlite3");

        // Check if .ddrive directory exists
        if !ddrive_path.exists() || !ddrive_path.is_dir() {
            return Ok(false);
        }

        // Check if metadata.sqlite3 file exists and is accessible
        if !db_path.exists() || !db_path.is_file() {
            return Ok(false);
        }

        // Try to read the database file to ensure it's accessible
        match fs::metadata(&db_path) {
            Ok(metadata) => {
                // Ensure it's a regular file and has some size (not empty)
                Ok(metadata.is_file() && metadata.len() > 0)
            }
            Err(_) => Ok(false),
        }
    }

    /// Initialize a new ddrive repository in the current working directory
    pub async fn init_repository(repo_root: PathBuf) -> Result<Repository> {
        let ddrive_path = repo_root.join(".ddrive");
        let objects_dir = ddrive_path.join("objects");
        let db_path = ddrive_path.join("metadata.sqlite3");
        let repo = Repository { repo_root };

        if ddrive_path.exists() && repo.is_valid()? {
            info!("Repository already initialized");
            return Ok(repo);
        }

        fs::create_dir_all(&ddrive_path)?;
        fs::create_dir_all(&objects_dir)?;

        debug!("Creating database and running migrations");
        repo.init_database(&db_path).await?;

        info!("Repository initialized successfully");
        Ok(repo)
    }

    /// Create the SQLite database with proper schema using sqlx migrations
    async fn init_database(&self, db_path: &Path) -> Result<()> {
        // Create the database file if it doesn't exist
        if !db_path.exists() {
            std::fs::File::create(db_path)?;
        }

        let database_url = format!("sqlite://{}", db_path.display());
        let pool = sqlx::SqlitePool::connect(&database_url).await?;

        sqlx::migrate!("./migrations").run(&pool).await?;
        pool.close().await;
        Ok(())
    }
}
