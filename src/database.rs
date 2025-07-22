use crate::{DdriveError, Result};
use chrono::{DateTime, Utc};
use serde_json::Value as JsonValue;
use sqlx::{FromRow, QueryBuilder, SqlitePool};
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

            // Insert into files table
            sqlx::query(
                r#"
                INSERT INTO files (path, b3sum, size, created_at, updated_at)
                VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
                "#,
            )
            .bind(&relative_path)
            .bind(b3sum)
            .bind(file_size)
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
                INSERT INTO history (action_id, action_type, path, b3sum, size)
                VALUES (?1, ?2, ?3, ?4, ?5)
                "#,
            )
            .bind(action_id)
            .bind(ActionType::Update.to_i32())
            .bind(&relative_path)
            .bind(b3sum)
            .bind(file_size)
            .execute(&mut *tx)
            .await?;

            // Update files table
            sqlx::query(
                r#"
                UPDATE files 
                SET b3sum = ?1, size = ?2, updated_at = CURRENT_TIMESTAMP
                WHERE path = ?3
                "#,
            )
            .bind(b3sum)
            .bind(file_size)
            .bind(&relative_path)
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
            modified: std::time::SystemTime::UNIX_EPOCH,
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
