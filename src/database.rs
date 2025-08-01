use crate::{
    DdriveError, Result,
    scanner::{FileInfo, get_all_files},
};
use chrono::{DateTime, Utc};
use serde_json::Value as JsonValue;
use sqlx::{FromRow, QueryBuilder, SqlitePool};
use std::{
    path::{Path, PathBuf},
    time::{Duration, UNIX_EPOCH},
};
use strum::{Display, EnumString};
use tracing::info;

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
    Rename = 4,
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
            4 => Self::Rename,
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

    /// Insert multiple file records in a single transaction for better performance
    pub async fn batch_insert_file_records(
        &self,
        action_id: i64,
        records: &[&crate::scanner::FileInfo],
    ) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }

        let mut tx = self.pool.begin().await?;
        for file_info in records {
            let relative_path = self.convert_to_relative_path(&file_info.path.to_string_lossy())?;
            let b3sum = file_info.b3sum.as_ref().expect("b3sum should be present");
            let file_size = file_info.size as i64;

            // Convert creation time to NaiveDateTime
            let created_at = file_info.created_at();
            let modified_at = file_info.modified_at();

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

            // Insert into files table
            sqlx::query(
                r#"
                INSERT INTO files (path, b3sum, size, created_at, updated_at)
                VALUES (?1, ?2, ?3, ?4, ?5)
                "#,
            )
            .bind(&relative_path)
            .bind(b3sum)
            .bind(file_size)
            .bind(created_at)
            .bind(modified_at)
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
        records: &[&FileInfo], // (file_path, b3sum, file_size)
    ) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }

        let mut tx = self.pool.begin().await?;
        for file in records {
            let b3sum = file.b3sum.as_ref().expect("b3sum");
            let relative_path = file.path.to_str().expect("relative path");

            // Insert into history for tracking
            sqlx::query(
                r#"
                INSERT INTO history (action_id, action_type, path, b3sum, size)
                VALUES (?1, ?2, ?3, ?4, ?5)
                "#,
            )
            .bind(action_id)
            .bind(ActionType::Update.to_i32())
            .bind(relative_path)
            .bind(b3sum)
            .bind(file.size as i64)
            .execute(&mut *tx)
            .await?;

            let updated_at = file.modified_at();

            // Update files table
            sqlx::query(
                r#"
                UPDATE files 
                SET b3sum = ?1, 
                    size = ?2, 
                    updated_at = ?3, 
                    last_checked = NULL
                WHERE path = ?4
                "#,
            )
            .bind(b3sum)
            .bind(file.size as i64)
            .bind(updated_at)
            .bind(relative_path)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    /// Batch delete file records in a single transaction
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
            // Insert into history for tracking
            sqlx::query(
                r#"
                INSERT INTO history (action_id, action_type, path, b3sum, size)
                VALUES (?1, ?2, ?3, ?4, ?5)
                "#,
            )
            .bind(action_id)
            .bind(ActionType::Delete.to_i32())
            .bind(file_path)
            .bind(b3sum)
            .bind(file_size)
            .execute(&mut *tx)
            .await?;

            // Delete from files table
            sqlx::query("DELETE FROM files WHERE path = ?1")
                .bind(file_path)
                .execute(&mut *tx)
                .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    /// Get all checksums referenced in the database (both files and history tables)
    pub async fn get_all_referenced_checksums(&self) -> Result<std::collections::HashSet<String>> {
        let mut checksums = std::collections::HashSet::new();

        // Get checksums from active files
        let active_checksums = sqlx::query!(
            r#"
            SELECT DISTINCT b3sum
            FROM files
            WHERE b3sum IS NOT NULL
            "#
        )
        .fetch_all(&self.pool)
        .await?;

        for record in active_checksums {
            checksums.insert(record.b3sum);
        }

        // Get checksums from history (to preserve deleted files)
        let history_checksums = sqlx::query!(
            r#"
            SELECT DISTINCT b3sum
            FROM history
            "#
        )
        .fetch_all(&self.pool)
        .await?;

        for record in history_checksums {
            checksums.insert(record.b3sum);
        }

        Ok(checksums)
    }

    /// Clean up orphaned objects from the object store
    pub async fn cleanup_orphaned_objects(&self) -> Result<usize> {
        let referenced_checksums = self.get_all_referenced_checksums().await?;
        let objects_dir = self.repo_root.join(".ddrive").join("objects");

        if !objects_dir.exists() {
            return Ok(0);
        }

        let mut deleted_count = 0;

        // Walk through the object store directory structure
        let files = get_all_files(&self.repo_root, &objects_dir, true, false)?;

        info!("Active objects: {}", referenced_checksums.len());
        info!("Available objects: {}", files.len());

        for file in files {
            let checksum = file
                .path
                .file_name()
                .expect("filename")
                .to_str()
                .expect("filename");

            if referenced_checksums.contains(checksum) {
                continue;
            }
            deleted_count += 1;
            std::fs::remove_file(&file.path)?;
            info!("Deleted orphaned object: {}", file.path.display());
        }

        Ok(deleted_count)
    }

    /// Get a file record by path
    pub async fn get_file_by_path(&self, file_path: &str) -> Result<Option<FileRecord>> {
        let relative_path = self.convert_to_relative_path(file_path)?;

        let record = sqlx::query_as!(
            FileRecord,
            r#"
            SELECT id, path, created_at, updated_at, last_checked, b3sum, size
            FROM files 
            WHERE path = ?1
            "#,
            relative_path
        )
        .fetch_optional(&self.pool)
        .await?;

        Ok(record)
    }

    /// Get all the records matching given path
    pub async fn get_files_by_paths(&self, file_paths: &Vec<&str>) -> Result<Vec<FileRecord>> {
        let mut query_builder = QueryBuilder::new(
            "SELECT id, path, created_at, updated_at, last_checked, b3sum, size FROM files WHERE path IN (",
        );

        query_builder.push_values(file_paths, |mut b, path| {
            b.push_bind(path);
        });

        query_builder.push(")");
        let query = query_builder.build_query_as::<FileRecord>();
        let records = query.fetch_all(&self.pool).await?;
        Ok(records)
    }

    /// Update the last_checked timestamp for a file
    pub async fn update_last_checked(&self, file_path: &str) -> Result<()> {
        let relative_path = self.convert_to_relative_path(file_path)?;

        sqlx::query!(
            r#"
            UPDATE files 
            SET last_checked = CURRENT_TIMESTAMP
            WHERE path = ?1
            "#,
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

    /// Get files that match a path prefix
    pub async fn get_files_by_path_prefix(&self, path_prefix: &str) -> Result<Vec<FileRecord>> {
        let records = sqlx::query_as!(
            FileRecord,
            r#"
            SELECT id, path, created_at, updated_at, last_checked, b3sum, size
            FROM files 
            WHERE path LIKE ?1 || '%'
            ORDER BY path
            "#,
            path_prefix
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

    /// Insert history entries for a batch of files
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

        let mut tx = self.pool.begin().await?;
        let metadata_json = metadata
            .map(|m| serde_json::to_string(&m).unwrap_or_default())
            .unwrap_or_default();

        for (file_path, b3sum, size) in file_entries {
            let relative_path = self.convert_to_relative_path(file_path)?;

            sqlx::query(
                r#"
                INSERT INTO history (action_id, action_type, path, b3sum, size, metadata)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                "#,
            )
            .bind(action_id)
            .bind(action_type.to_i32())
            .bind(&relative_path)
            .bind(b3sum)
            .bind(size)
            .bind(&metadata_json)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    /// Get history entries with optional limit and filter
    pub async fn get_history_entries(
        &self,
        limit: Option<usize>,
        action_filter: Option<ActionType>,
    ) -> Result<Vec<HistoryRecord>> {
        let limit = limit.unwrap_or(20) as i64;

        let records = match action_filter {
            Some(action_type) => {
                let action_type = action_type.to_i32();
                sqlx::query_as!(
                    HistoryRecord,
                    r#"
                    SELECT id, action_id, action_type, path, b3sum, size, metadata
                    FROM history
                    WHERE action_type = ?1
                    LIMIT ?2
                    "#,
                    action_type,
                    limit
                )
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query_as!(
                    HistoryRecord,
                    r#"
                    SELECT id, action_id, action_type, path, b3sum, size, metadata
                    FROM history
                    LIMIT ?1
                    "#,
                    limit
                )
                .fetch_all(&self.pool)
                .await?
            }
        };

        Ok(records)
    }

    /// Get history entries by action ID (base58 encoded)
    pub async fn get_history_entries_by_action_id_base58(
        &self,
        action_id_base58: &str,
    ) -> Result<Vec<HistoryRecord>> {
        // Decode base58 action ID
        let decoded =
            bs58::decode(action_id_base58)
                .into_vec()
                .map_err(|_| DdriveError::Validation {
                    message: "Invalid action ID format".to_string(),
                })?;

        if decoded.len() != 8 {
            return Err(DdriveError::Validation {
                message: "Invalid action ID length".to_string(),
            });
        }

        // Convert bytes to i64
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&decoded);
        let action_id = i64::from_be_bytes(bytes);

        let records = sqlx::query_as!(
            HistoryRecord,
            r#"
            SELECT id, action_id, action_type, path, b3sum, size, metadata
            FROM history
            WHERE action_id = ?1
            ORDER BY path
            "#,
            action_id
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }

    /// Get files that need verification based on configuration
    pub async fn get_files_for_check(&self) -> Result<Vec<FileRecord>> {
        let records = sqlx::query_as!(
            FileRecord,
            r#"
            SELECT id, path, created_at, updated_at, last_checked, b3sum, size
            FROM files
            WHERE last_checked IS NULL
            ORDER BY path
            "#
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }

    /// Get lightweight file info for status display
    pub async fn get_tracked_file_paths(&self) -> Result<Vec<TrackedFileInfo>> {
        let records = sqlx::query_as!(
            TrackedFileInfo,
            r#"
            SELECT path, size, created_at
            FROM files
            ORDER BY path
            "#
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }

    /// Clean up old history entries
    pub async fn cleanup_old_history(
        &self,
        action_type: ActionType,
        cutoff_timestamp: i64,
    ) -> Result<usize> {
        let action_type = action_type.to_i32();
        let result = sqlx::query!(
            r#"
            DELETE FROM history
            WHERE action_type = ?1 AND action_id < ?2
            "#,
            action_type,
            cutoff_timestamp
        )
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() as usize)
    }

    /// Find potential renames by matching deleted files with new files by checksum and size
    pub async fn find_potential_renames(
        &self,
        deleted_files: &[FileInfo],
        new_files: &[FileInfo],
    ) -> Result<Vec<(FileInfo, FileInfo)>> {
        let mut potential_renames = Vec::new();

        // Create lookup maps for efficient matching
        let mut deleted_by_checksum: std::collections::HashMap<String, Vec<&FileInfo>> =
            std::collections::HashMap::new();
        let mut new_by_checksum: std::collections::HashMap<String, Vec<&FileInfo>> =
            std::collections::HashMap::new();

        // Group deleted files by checksum (if available)
        for file in deleted_files {
            if let Some(ref checksum) = file.b3sum {
                deleted_by_checksum
                    .entry(checksum.clone())
                    .or_default()
                    .push(file);
            }
        }

        // Group new files by checksum (if available)
        for file in new_files {
            if let Some(ref checksum) = file.b3sum {
                new_by_checksum
                    .entry(checksum.clone())
                    .or_default()
                    .push(file);
            }
        }

        // Find matches by checksum and size
        for (checksum, deleted_list) in deleted_by_checksum {
            if let Some(new_list) = new_by_checksum.get(&checksum) {
                // Match files with same checksum and size
                for deleted_file in deleted_list {
                    for new_file in new_list {
                        if deleted_file.size == new_file.size {
                            potential_renames.push(((*deleted_file).clone(), (*new_file).clone()));
                            break; // Only match each deleted file once
                        }
                    }
                }
            }
        }

        Ok(potential_renames)
    }

    /// Process file renames in batch
    pub async fn batch_rename_files(
        &self,
        action_id: i64,
        renames: &[(String, String)], // (old_path, new_path)
    ) -> Result<()> {
        if renames.is_empty() {
            return Ok(());
        }

        let mut tx = self.pool.begin().await?;

        for (old_path, new_path) in renames {
            let old_relative_path = self.convert_to_relative_path(old_path)?;
            let new_relative_path = self.convert_to_relative_path(new_path)?;

            // Get the file record to preserve checksum and size
            let file_record = sqlx::query!(
                "SELECT b3sum, size FROM files WHERE path = ?1",
                old_relative_path
            )
            .fetch_optional(&mut *tx)
            .await?;

            if let Some(record) = file_record {
                // Insert rename history entry with metadata containing old path
                let metadata = serde_json::json!({
                    "old_path": old_relative_path
                });
                let metadata_str = serde_json::to_string(&metadata).unwrap_or_default();

                sqlx::query(
                    r#"
                    INSERT INTO history (action_id, action_type, path, b3sum, size, metadata)
                    VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                    "#,
                )
                .bind(action_id)
                .bind(ActionType::Rename.to_i32())
                .bind(&new_relative_path)
                .bind(&record.b3sum)
                .bind(record.size)
                .bind(&metadata_str)
                .execute(&mut *tx)
                .await?;

                // Update the file record with new path
                sqlx::query(
                    r#"
                    UPDATE files 
                    SET path = ?1, updated_at = CURRENT_TIMESTAMP
                    WHERE path = ?2
                    "#,
                )
                .bind(&new_relative_path)
                .bind(&old_relative_path)
                .execute(&mut *tx)
                .await?;
            }
        }

        tx.commit().await?;
        Ok(())
    }

    /// Convert an absolute path to a path relative to the repository root
    fn convert_to_relative_path(&self, file_path: &str) -> Result<String> {
        let path = Path::new(file_path);
        let absolute_path = if path.is_absolute() {
            path.to_path_buf().canonicalize()?
        } else {
            self.repo_root.join(path).canonicalize()?
        };

        match absolute_path.strip_prefix(&self.repo_root) {
            Ok(relative) => Ok(relative.to_string_lossy().into_owned()),
            Err(_) => Err(DdriveError::FileSystem {
                message: format!(
                    "Path {} is not within repository root {}",
                    file_path,
                    self.repo_root.display()
                ),
            }),
        }
    }
}

/// File record from the database
#[derive(Debug, FromRow)]
pub struct FileRecord {
    pub id: i64,
    pub path: String,
    pub created_at: chrono::NaiveDateTime,
    pub updated_at: chrono::NaiveDateTime,
    pub last_checked: Option<chrono::NaiveDateTime>,
    pub b3sum: String,
    pub size: i64,
}

impl From<&FileRecord> for crate::scanner::FileInfo {
    fn from(record: &FileRecord) -> Self {
        Self {
            path: std::path::PathBuf::from(&record.path),
            size: record.size as u64,
            modified: UNIX_EPOCH
                + Duration::from_secs(record.updated_at.and_utc().timestamp() as u64),
            created: UNIX_EPOCH
                + Duration::from_secs(record.created_at.and_utc().timestamp() as u64),
            b3sum: Some(record.b3sum.clone()),
        }
    }
}

/// Lightweight file info for status display
#[derive(Debug, FromRow)]
pub struct TrackedFileInfo {
    pub path: String,
    pub size: i64,
    pub created_at: chrono::NaiveDateTime,
}

/// History record from the database
#[derive(Debug, FromRow)]
pub struct HistoryRecord {
    pub id: i64,
    pub action_id: i64,
    pub action_type: i64,
    pub path: String,
    pub b3sum: Option<String>,
    pub size: Option<i64>,
    pub metadata: Option<String>,
}

impl HistoryRecord {
    pub fn action_type_enum(&self) -> ActionType {
        ActionType::from(self.action_type)
    }

    pub fn action_timestamp(&self) -> DateTime<Utc> {
        DateTime::from_timestamp(self.action_id, 0).unwrap_or_else(Utc::now)
    }

    pub fn action_id_base58(&self) -> String {
        bs58::encode(self.action_id.to_be_bytes()).into_string()
    }
}
