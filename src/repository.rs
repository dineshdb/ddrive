use crate::ignore::DEFAULT_IGNORE_PATTERNS;
use crate::{DdriveError, Result};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, info};

#[derive(Default)]
pub struct RepositoryFinder;

impl RepositoryFinder {
    pub fn new() -> Self {
        RepositoryFinder
    }

    /// Search for .ddrive/metadata.sqlite3 in current and parent directories
    pub fn find_repository(&self) -> Result<Option<PathBuf>> {
        let current_dir = env::current_dir()?;
        let mut search_path = current_dir.as_path();

        loop {
            let ddrive_path = search_path.join(".ddrive");
            let db_path = ddrive_path.join("metadata.sqlite3");

            if db_path.exists() && db_path.is_file() {
                return Ok(Some(search_path.to_path_buf()));
            }

            // Move to parent directory
            match search_path.parent() {
                Some(parent) => search_path = parent,
                None => break, // Reached filesystem root
            }
        }

        Ok(None)
    }

    /// Validate that the repository has a valid database structure
    pub fn validate_repository<P: AsRef<Path>>(&self, repo_path: P) -> Result<bool> {
        let repo_path = repo_path.as_ref();
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

    /// Ensure repository exists and return its root path
    pub fn ensure_repository_exists(&self) -> Result<PathBuf> {
        debug!("Searching for ddrive repository...");

        match self.find_repository()? {
            Some(repo_path) => {
                debug!("Found repository at: {}", repo_path.display());
                if self.validate_repository(&repo_path)? {
                    Ok(repo_path)
                } else {
                    Err(DdriveError::Repository {
                        message:
                            "Found repository directory but database is invalid or inaccessible."
                                .to_string(),
                    })
                }
            }
            None => Err(DdriveError::Repository {
                message: "No ddrive repository found.".to_string(),
            }),
        }
    }

    /// Initialize a new ddrive repository in the current working directory
    pub async fn init_repository(&self) -> Result<()> {
        let current_dir = env::current_dir()?;
        let ddrive_path = current_dir.join(".ddrive");
        let db_path = ddrive_path.join("metadata.sqlite3");

        if ddrive_path.exists() && self.validate_repository(&current_dir)? {
            info!("Repository already initialized");
            return Ok(());
        }

        fs::create_dir_all(&ddrive_path)?;

        // Create objects directory structure
        let objects_dir = ddrive_path.join("objects");
        fs::create_dir_all(&objects_dir)?;

        debug!("Creating database and running migrations");
        self.create_database(&db_path).await?;

        // Create default ignore file
        let ignore_file = ddrive_path.join("ignore");
        if !ignore_file.exists() {
            debug!("Creating default ignore file");
            fs::write(&ignore_file, DEFAULT_IGNORE_PATTERNS)?;
        }

        info!("Repository initialized successfully");
        Ok(())
    }

    /// Create the SQLite database with proper schema using sqlx migrations
    async fn create_database(&self, db_path: &Path) -> Result<()> {
        // Create the database file if it doesn't exist
        if !db_path.exists() {
            std::fs::File::create(db_path)?;
        }

        // Connect to the database and run migrations using sqlx
        let database_url = format!("sqlite://{}", db_path.display());
        let pool = sqlx::SqlitePool::connect(&database_url).await?;

        // Run migrations to set up the schema
        sqlx::migrate!("./migrations").run(&pool).await?;

        // Close the connection
        pool.close().await;

        Ok(())
    }
}
