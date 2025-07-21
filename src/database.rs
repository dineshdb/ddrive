use crate::{DdriveError, Result};
use chrono::{DateTime, Utc};
use serde_json::Value as JsonValue;
use sqlx::{FromRow, SqlitePool};
use std::path::{Path, PathBuf};
use strum::{Display, EnumString};

/// Action types for history tracking
#[derive(
    Debug, Clone, Copy, Display, EnumString, PartialEq, Eq, serde::Serialize, serde::Deserialize,
)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
#[repr(i32)]
pub enum ActionType {
    Unknown = 0,
    Add = 1,
    Delete = 2,
    Update = 3,
}

impl ActionType {
    pub fn to_i32(self) -> i32 {
        self as i32
    }
}

impl From<i64> for ActionType {
    fn from(value: i64) -> Self {
        match value {
            1 => Self::Add,
            2 => Self::Delete,
            3 => Self::Update,
            _ => Self::Unknown,
        }
    }
}

/// Database abstraction layer for ddrive file tracking
///
/// Manages SQLite database operations including file record storage,
/// integrity checking, and duplicate detection. All file paths are
/// stored as relative paths from the repository root.
#[derive(Clone)]
pub struct Database {
    pub pool: SqlitePool,
    pub repo_root: PathBuf,
}

impl Database {
    pub async fn new(database_url: &str, repo_root: PathBuf) -> Result<Self> {
        let pool = SqlitePool::connect(database_url).await?;

        // Run migrations to ensure database schema is up to date
        // This is safe to run multiple times as sqlx tracks which migrations have been applied
        sqlx::migrate!("./migrations").run(&pool).await?;

        Ok(Database { pool, repo_root })
    }

    /// Insert a new file record with created_at timestamp and relative path
    pub async fn insert_file_record(
        &self,
        file_path: &str,
        b3sum: &str,
        file_size: i64,
    ) -> Result<()> {
        let relative_path = self.convert_to_relative_path(file_path)?;

        sqlx::query!(
            r#"
            INSERT INTO files (path, b3sum, size, created_at, updated_at)
            VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
            "#,
            relative_path,
            b3sum,
            file_size
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Insert multiple file records in a single transaction for better performance
    pub async fn batch_insert_file_records(
        &self,
        action_id: i64,
        records: &[&(String, String, i64)], // (file_path, b3sum, file_size)
    ) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }

        let mut tx = self.pool.begin().await?;
        for (file_path, b3sum, file_size) in records {
            let relative_path = self.convert_to_relative_path(file_path)?;

            // Insert into history for tracking
            sqlx::query(
                r#"
             INSERT INTO history (action_id, action_type, path, b3sum, size)
                VALUES (?, ?, ?, ?, ?)
            "#,
            )
            .bind(action_id)
            .bind(ActionType::Add.to_i32())
            .bind(&relative_path)
            .bind(b3sum)
            .bind(file_size)
            .execute(&mut *tx)
            .await?;

            // Create a new query for each record
            sqlx::query(
                r#"
                INSERT INTO files (path, b3sum, size, created_at, updated_at)
                VALUES (?, ?, ?, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
                "#,
            )
            .bind(relative_path)
            .bind(b3sum)
            .bind(file_size)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    pub async fn batch_delete_file_records(
        &self,
        action_id: i64,
        records: &[(String, String, i64)], // (file_path, b3sum, file_size)
    ) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }

        let mut tx = self.pool.begin().await?;
        for (file_path, b3sum, file_size) in records {
            let relative_path = self.convert_to_relative_path(file_path)?;

            // Insert into history for tracking
            sqlx::query(
                r#"
             INSERT INTO history (action_id, action_type, path, b3sum, size)
                VALUES (?, ?, ?, ?, ?)
            "#,
            )
            .bind(action_id)
            .bind(ActionType::Delete.to_i32())
            .bind(&relative_path)
            .bind(b3sum)
            .bind(file_size)
            .execute(&mut *tx)
            .await?;

            // Create a new query for each record
            sqlx::query("DELETE FROM files WHERE path = ? ")
                .bind(relative_path)
                .execute(&mut *tx)
                .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    /// Update multiple file records in a single transaction for better performance
    pub async fn batch_update_file_records(
        &self,
        action_id: i64,
        records: &[(String, String, i64)], // (file_path, b3sum, file_size)
    ) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }

        let mut tx = self.pool.begin().await?;
        for (file_path, b3sum, file_size) in records {
            let relative_path = self.convert_to_relative_path(file_path)?;

            // Insert into history for tracking
            sqlx::query(
                r#"
             INSERT INTO history (action_id, action_type, path, b3sum, size, metadata)
                VALUES (?, ?, ?, ?, ?, ?)
            "#,
            )
            .bind(action_id)
            .bind(ActionType::Update.to_i32())
            .bind(&relative_path)
            .bind(b3sum)
            .bind(file_size)
            .execute(&mut *tx)
            .await?;

            // Create a new query for each record
            sqlx::query(
                r#"
                UPDATE files 
                SET b3sum = ?, size = ?, updated_at = CURRENT_TIMESTAMP, last_checked = NULL
                WHERE path = ?
                "#,
            )
            .bind(b3sum)
            .bind(file_size)
            .bind(relative_path)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    /// Check if a file already exists in the database using relative path
    pub async fn check_file_exists(&self, file_path: &str) -> Result<bool> {
        let relative_path = self.convert_to_relative_path(file_path)?;

        let result = sqlx::query!(
            "SELECT COUNT(*) as count FROM files WHERE path = ?1",
            relative_path
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(result.count > 0)
    }

    /// Get file record for comparison using relative path
    pub async fn get_file_by_path(&self, file_path: &str) -> Result<Option<FileRecord>> {
        let relative_path = self.convert_to_relative_path(file_path)?;

        let record = sqlx::query_as!(
            FileRecord,
            r#"
            SELECT id, path, created_at, updated_at, last_checked, b3sum, size as "size: i64"
            FROM files 
            WHERE path = ?1
            "#,
            relative_path
        )
        .fetch_optional(&self.pool)
        .await?;

        Ok(record)
    }

    /// Convert absolute file paths to paths relative to repository root
    pub fn convert_to_relative_path(&self, file_path: &str) -> Result<String> {
        let file_path = Path::new(file_path);
        let absolute_path = if file_path.is_absolute() {
            file_path.to_path_buf()
        } else {
            std::env::current_dir()?.join(file_path)
        };

        let relative_path =
            absolute_path
                .strip_prefix(&self.repo_root)
                .map_err(|_| DdriveError::Validation {
                    message: format!(
                        "File path {} is not within repository root {}",
                        absolute_path.display(),
                        self.repo_root.display()
                    ),
                })?;

        // Convert to string efficiently
        Ok(relative_path.to_string_lossy().into_owned())
    }

    /// Query files that need checking (last_checked > 1 month or null)
    pub async fn get_files_for_check(&self) -> Result<Vec<FileRecord>> {
        let records = sqlx::query_as!(
            FileRecord,
            r#"
            SELECT id, path, created_at, updated_at, last_checked, b3sum, size as "size: i64"
            FROM files 
            WHERE (last_checked IS NULL OR last_checked < datetime('now', '-1 month'))
            ORDER BY path
            "#
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }

    /// Update last_checked timestamp after successful verification
    pub async fn update_last_checked(&self, file_path: &str) -> Result<()> {
        let relative_path = self.convert_to_relative_path(file_path)?;

        sqlx::query!(
            "UPDATE files SET last_checked = CURRENT_TIMESTAMP WHERE path = ?1",
            relative_path
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Batch update last_checked timestamps for multiple files
    pub async fn batch_update_last_checked(&self, file_paths: &[String]) -> Result<()> {
        if file_paths.is_empty() {
            return Ok(());
        }

        // Start a transaction
        let mut tx = self.pool.begin().await?;

        for file_path in file_paths {
            let relative_path = self.convert_to_relative_path(file_path)?;

            // Create a new query for each record
            sqlx::query("UPDATE files SET last_checked = CURRENT_TIMESTAMP WHERE path = ?")
                .bind(relative_path)
                .execute(&mut *tx)
                .await?;
        }

        // Commit the transaction
        tx.commit().await?;

        Ok(())
    }

    /// Find all active files for duplicate detection
    pub async fn find_duplicates(&self) -> Result<Vec<FileRecord>> {
        let records = sqlx::query_as!(
            FileRecord,
            r#"
            SELECT id, path, created_at, updated_at, last_checked, b3sum, size
            FROM files 
            ORDER BY b3sum, path
            "#
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }

    /// Delete a file record from the database (hard delete)
    pub async fn delete_file_record(&self, file_path: &str) -> Result<()> {
        let relative_path = self.convert_to_relative_path(file_path)?;
        sqlx::query!("DELETE FROM files WHERE path = ?1", relative_path)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// Get all tracked files
    pub async fn get_all_files(&self) -> Result<Vec<FileRecord>> {
        let records = sqlx::query_as!(
            FileRecord,
            r#"
            SELECT id, path, created_at, updated_at, last_checked, b3sum, size
            FROM files 
            ORDER BY path
            "#
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }

    /// Get files that haven't been checked since a specific date
    pub async fn get_files_not_checked_since(
        &self,
        cutoff_date: chrono::DateTime<Utc>,
    ) -> Result<Vec<FileRecord>> {
        let records = sqlx::query_as!(
            FileRecord,
            r#"
            SELECT id, path, created_at, updated_at, last_checked, b3sum, size
            FROM files
            WHERE (last_checked IS NULL OR last_checked < ?)
            "#,
            cutoff_date
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }

    /// Add a history entry for a batch of files
    pub async fn add_history_entry(
        &self,
        action_type: ActionType,
        file_entries: Vec<(String, Option<String>, Option<i64>)>, // (file_path, file_b3sum, file_size)
    ) -> Result<i64> {
        let action_id = chrono::Utc::now().timestamp();
        self.insert_history_entries(action_id, action_type, &file_entries, None)
            .await?;
        Ok(action_id)
    }

    /// Rename a file record in the database (for tracking file renames)
    pub async fn rename_file_record(&self, old_path: &str, new_path: &str) -> Result<()> {
        let old_relative_path = self.convert_to_relative_path(old_path)?;
        let new_relative_path = self.convert_to_relative_path(new_path)?;

        sqlx::query!(
            r#"
            UPDATE files
            SET path = ?, updated_at = CURRENT_TIMESTAMP
            WHERE path = ?
            "#,
            new_relative_path,
            old_relative_path
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Find deleted files by comparing database records with history
    pub async fn find_deleted_files(&self) -> Result<Vec<String>> {
        // Get all files that have a delete action in history but still exist in files table
        let deleted_paths = sqlx::query!(
            r#"
            SELECT DISTINCT h.path
            FROM history h
            JOIN files f ON h.path = f.path
            WHERE h.action_type = ?
            "#,
            ActionType::Delete as i32
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(deleted_paths.into_iter().map(|r| r.path).collect())
    }

    /// Get files that were deleted before a specific date based on history
    pub async fn get_files_deleted_before(
        &self,
        cutoff_date: chrono::NaiveDateTime,
    ) -> Result<Vec<String>> {
        let deleted_paths = sqlx::query!(
            r#"
            SELECT h.path
            FROM history h
            WHERE h.action_type = ?
            AND action_id < ?
            "#,
            ActionType::Delete as i32,
            cutoff_date
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(deleted_paths.into_iter().map(|r| r.path).collect())
    }

    /// Get only file paths and basic info for status checks (lightweight)
    pub async fn get_tracked_file_paths(&self) -> Result<Vec<TrackedFileInfo>> {
        let records = sqlx::query_as!(
            TrackedFileInfo,
            r#"
            SELECT path, size as "size: i64", created_at, updated_at
            FROM files 
            ORDER BY path
            "#
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }

    /// Insert a history entry for an action affecting a specific file
    pub async fn insert_history_entry(
        &self,
        action_id: i64,
        action_type: ActionType,
        file_path: &str,
        file_b3sum: Option<&str>,
        file_size: Option<i64>,
        metadata: Option<JsonValue>,
    ) -> Result<()> {
        let relative_path = self.convert_to_relative_path(file_path)?;
        let metadata_str = metadata.map(|m| m.to_string());
        let action_type_int = action_type.to_i32();

        sqlx::query!(
            r#"
            INSERT INTO history (action_id, action_type, path, b3sum, size, metadata)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
            action_id,
            action_type_int,
            relative_path,
            file_b3sum,
            file_size,
            metadata_str
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Insert multiple history entries for a single action
    pub async fn insert_history_entries(
        &self,
        action_id: i64,
        action_type: ActionType,
        file_entries: &[(String, Option<String>, Option<i64>)], // (file_path, file_b3sum, file_size)
        metadata: Option<JsonValue>,
    ) -> Result<()> {
        if file_entries.is_empty() {
            return Ok(());
        }

        let action_type_int = action_type.to_i32();
        let metadata_str = metadata.map(|m| m.to_string());

        // Start a transaction for batch insert
        let mut tx = self.pool.begin().await?;
        for (file_path, file_b3sum, file_size) in file_entries {
            let relative_path = self.convert_to_relative_path(file_path)?;

            sqlx::query(
                r#"
                INSERT INTO history (action_id, action_type, path, b3sum, size, metadata)
                VALUES (?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(action_id)
            .bind(action_type_int)
            .bind(relative_path)
            .bind(file_b3sum)
            .bind(file_size)
            .bind(&metadata_str)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;

        Ok(())
    }

    /// Get history entries with optional filters
    pub async fn get_history_entries(
        &self,
        limit: Option<usize>,
        action_type_filter: Option<ActionType>,
    ) -> Result<Vec<HistoryRecord>> {
        let limit_clause = limit.map(|l| l as i64).unwrap_or(i64::MAX);

        let records = if let Some(filter) = action_type_filter {
            let filter_int = filter.to_i32();
            sqlx::query_as!(
                HistoryRecord,
                r#"
                SELECT id, action_id, action_type, path, b3sum, size, metadata
                FROM history 
                WHERE action_type = ?1
                ORDER BY action_id DESC
                LIMIT ?2
                "#,
                filter_int,
                limit_clause
            )
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query_as!(
                HistoryRecord,
                r#"
                SELECT id, action_id , action_type , path, b3sum, size, metadata
                FROM history 
                ORDER BY action_id DESC
                LIMIT ?1
                "#,
                limit_clause
            )
            .fetch_all(&self.pool)
            .await?
        };

        Ok(records)
    }

    /// Get all history entries for a specific action_id
    pub async fn get_history_entries_by_action_id(
        &self,
        action_id: i64,
    ) -> Result<Vec<HistoryRecord>> {
        let records = sqlx::query_as!(
            HistoryRecord,
            r#"
            SELECT id, action_id, action_type , path, b3sum, size, metadata
            FROM history 
            WHERE action_id = ?1
            ORDER BY id ASC
            "#,
            action_id
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }

    /// Get all history entries for a specific action_id from base58 string
    pub async fn get_history_entries_by_action_id_base58(
        &self,
        action_id_base58: &str,
    ) -> Result<Vec<HistoryRecord>> {
        let action_id_bytes =
            bs58::decode(action_id_base58)
                .into_vec()
                .map_err(|e| DdriveError::Validation {
                    message: format!("Invalid base58 action ID: {e}"),
                })?;

        if action_id_bytes.len() != 8 {
            return Err(DdriveError::Validation {
                message: "Invalid action ID length".to_string(),
            });
        }

        let action_id = i64::from_be_bytes(action_id_bytes.try_into().map_err(|_| {
            DdriveError::Validation {
                message: "Invalid action ID format".to_string(),
            }
        })?);

        self.get_history_entries_by_action_id(action_id).await
    }

    /// Clean up old history entries based on action type and age
    pub async fn cleanup_old_history(
        &self,
        action_type: ActionType,
        cutoff_timestamp: i64,
    ) -> Result<usize> {
        let action_type_int = action_type.to_i32();

        let result = sqlx::query!(
            "DELETE FROM history WHERE action_type = ?1 AND action_id < ?2",
            action_type_int,
            cutoff_timestamp
        )
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() as usize)
    }

    /// Create a backup of the database to the specified file
    pub async fn backup_to_file(&self, backup_path: &Path) -> Result<()> {
        // For now, use a simple file copy approach
        // In the future, we could use SQLite's backup API directly
        let current_db_path = self.repo_root.join(".ddrive").join("metadata.sqlite3");

        if !current_db_path.exists() {
            return Err(DdriveError::FileSystem {
                message: "Database file does not exist".to_string(),
            });
        }

        // Ensure the backup directory exists
        if let Some(parent) = backup_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| DdriveError::FileSystem {
                message: format!("Failed to create backup directory: {e}"),
            })?;
        }

        // Copy the database file
        std::fs::copy(&current_db_path, backup_path).map_err(|e| DdriveError::FileSystem {
            message: format!("Failed to create database backup: {e}"),
        })?;

        Ok(())
    }

    /// Restore database from a backup file
    pub async fn restore_from_file(&self, backup_path: &Path) -> Result<()> {
        if !backup_path.exists() {
            return Err(DdriveError::FileSystem {
                message: format!("Backup file does not exist: {}", backup_path.display()),
            });
        }

        // Close existing connections
        self.pool.close().await;

        // Copy the backup file over the current database
        let current_db_path = self.repo_root.join(".ddrive").join("metadata.sqlite3");

        // Create backup of current database first
        let backup_current = current_db_path.with_extension("sqlite3.backup");
        if current_db_path.exists() {
            std::fs::copy(&current_db_path, &backup_current).map_err(|e| {
                DdriveError::FileSystem {
                    message: format!("Failed to backup current database: {e}"),
                }
            })?;
        }

        // Copy the restore file to the database location
        std::fs::copy(backup_path, &current_db_path).map_err(|e| {
            // Try to restore the original database if copy failed
            if backup_current.exists() {
                let _ = std::fs::copy(&backup_current, &current_db_path);
            }
            DdriveError::FileSystem {
                message: format!("Failed to restore database from backup: {e}"),
            }
        })?;

        // Clean up the backup of current database
        if backup_current.exists() {
            let _ = std::fs::remove_file(&backup_current);
        }

        Ok(())
    }

    /// Get object store path for a given b3sum using double directory strategy
    pub fn get_object_path(&self, b3sum: &str) -> PathBuf {
        if b3sum.len() < 4 {
            // Fallback for short checksums
            return self.repo_root.join(".ddrive").join("objects").join(b3sum);
        }

        let dir1 = &b3sum[0..2];
        let dir2 = &b3sum[2..4];
        self.repo_root
            .join(".ddrive")
            .join("objects")
            .join(dir1)
            .join(dir2)
            .join(b3sum)
    }

    /// Store a file in the object store using hard links
    pub async fn store_object(&self, b3sum: &str, source_path: &Path) -> Result<()> {
        let object_path = self.get_object_path(b3sum);

        // Create parent directories
        if let Some(parent) = object_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| DdriveError::FileSystem {
                message: format!("Failed to create object directory: {e}"),
            })?;
        }

        // Create a hard link to the source file if the object doesn't exist
        if !object_path.exists() {
            std::fs::hard_link(source_path, &object_path).map_err(|e| DdriveError::HardLink {
                message: format!("Failed to create hard link for object: {e}"),
            })?;
        }

        Ok(())
    }

    /// Delete object from filesystem
    pub async fn delete_object(&self, b3sum: &str) -> Result<()> {
        let object_path = self.get_object_path(b3sum);

        // Remove from filesystem if it exists
        if object_path.exists() {
            std::fs::remove_file(&object_path).map_err(|e| DdriveError::FileSystem {
                message: format!("Failed to delete object file: {e}"),
            })?;
        }

        Ok(())
    }

    /// Check if an object exists in the object store
    pub fn object_exists(&self, b3sum: &str) -> bool {
        self.get_object_path(b3sum).exists()
    }

    /// Find orphaned objects by checking which objects in the filesystem
    /// are not referenced in the files or history tables
    pub async fn find_orphaned_objects(&self) -> Result<Vec<String>> {
        // Get all unique b3sums from files and history tables
        let referenced_checksums = sqlx::query!(
            r#"
            SELECT DISTINCT b3sum FROM (
                SELECT b3sum FROM files
                UNION
                SELECT b3sum FROM history WHERE b3sum IS NOT NULL
            )
            "#
        )
        .fetch_all(&self.pool)
        .await?;

        // Convert to HashSet for efficient lookup
        let referenced_set: std::collections::HashSet<String> = referenced_checksums
            .into_iter()
            .map(|row| row.b3sum)
            .collect();

        // Get all objects from the filesystem
        let objects_dir = self.repo_root.join(".ddrive").join("objects");
        let mut orphaned = Vec::new();

        if objects_dir.exists() {
            Self::scan_objects_directory(&objects_dir, &referenced_set, &mut orphaned)?;
        }

        Ok(orphaned)
    }

    /// Recursively scan the objects directory to find orphaned objects
    fn scan_objects_directory(
        dir: &Path,
        referenced_checksums: &std::collections::HashSet<String>,
        orphaned: &mut Vec<String>,
    ) -> Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                // Recursively scan subdirectories
                Self::scan_objects_directory(&path, referenced_checksums, orphaned)?;
            } else if path.is_file() {
                // Check if this object is referenced
                if let Some(file_name) = path.file_name() {
                    let checksum = file_name.to_string_lossy().to_string();
                    if !referenced_checksums.contains(&checksum) {
                        orphaned.push(checksum);
                    }
                }
            }
        }

        Ok(())
    }

    /// Close the database connection pool gracefully
    pub async fn close(&self) {
        self.pool.close().await;
    }
}

#[derive(Debug, Clone, FromRow)]
pub struct FileRecord {
    pub id: i64,
    pub path: String,
    pub created_at: chrono::NaiveDateTime,
    pub updated_at: chrono::NaiveDateTime,
    pub last_checked: Option<chrono::NaiveDateTime>,
    pub b3sum: String,
    pub size: i64,
}

#[derive(Debug, Clone, FromRow)]
pub struct TrackedFileInfo {
    pub path: String,
    pub size: i64,
    pub created_at: chrono::NaiveDateTime,
    pub updated_at: chrono::NaiveDateTime,
}

#[derive(Debug, Clone, FromRow)]
pub struct HistoryRecord {
    pub id: i64,
    pub action_id: i64,
    pub action_type: ActionType,
    pub path: String,
    pub b3sum: String,
    pub size: i64,
    pub metadata: Option<String>,
}

impl HistoryRecord {
    /// Convert action_id (Unix timestamp) to DateTime
    pub fn action_timestamp(&self) -> DateTime<Utc> {
        DateTime::from_timestamp(self.action_id, 0).unwrap_or_else(Utc::now)
    }

    /// Get action_id as base58 string for display
    pub fn action_id_base58(&self) -> String {
        bs58::encode(self.action_id.to_be_bytes()).into_string()
    }

    /// Parse metadata as JSON
    pub fn metadata_json(&self) -> Result<Option<JsonValue>> {
        match &self.metadata {
            Some(json_str) => {
                let value =
                    serde_json::from_str(json_str).map_err(|e| DdriveError::Validation {
                        message: format!("Invalid JSON in history metadata: {e}"),
                    })?;
                Ok(Some(value))
            }
            None => Ok(None),
        }
    }
}

impl FileRecord {
    /// Convert created_at to UTC DateTime
    pub fn created_at_utc(&self) -> DateTime<Utc> {
        self.created_at.and_utc()
    }

    /// Convert updated_at to UTC DateTime
    pub fn updated_at_utc(&self) -> DateTime<Utc> {
        self.updated_at.and_utc()
    }

    /// Convert last_checked to UTC DateTime if present
    pub fn last_checked_utc(&self) -> Option<DateTime<Utc>> {
        self.last_checked.map(|dt| dt.and_utc())
    }
}
