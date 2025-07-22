//! File removal functionality for removing files from repository.
//!
//! This module provides the `RmCommand` which handles the workflow
//! of removing files from tracking in the database without affecting
//! the actual files on disk.

use crate::{AppContext, Result, scanner::FileScanner, utils::FileProcessor};
use glob::Pattern;
use tracing::info;

pub struct RmCommand<'a> {
    context: &'a AppContext,
}

impl<'a> RmCommand<'a> {
    pub fn new(context: &'a AppContext) -> Self {
        RmCommand { context }
    }

    /// Remove tracked files
    pub async fn tracked(&self, pattern: Pattern) -> Result<usize> {
        let tracked_files = self.context.database.get_all_files().await?;
        let files_to_remove: Vec<_> = tracked_files
            .into_iter()
            .filter(|file| pattern.matches(&file.path))
            .collect();

        if files_to_remove.is_empty() {
            info!("No matching files found to remove from tracking");
            return Ok(0);
        }

        self.display_files_to_remove(&files_to_remove);

        let file_records: Vec<(String, String, i64)> = files_to_remove
            .iter()
            .map(|file| (file.path.clone(), file.b3sum.clone(), file.size))
            .collect();

        let action_id = chrono::Utc::now().timestamp();
        self.context
            .database
            .batch_delete_file_records(action_id, &file_records)
            .await?;

        info!("Removed {} files from tracking", files_to_remove.len());
        Ok(file_records.len())
    }

    /// Remove the deleted files from tracking
    pub async fn deleted(&self, pattern: Option<Pattern>) -> Result<usize> {
        let pattern = pattern.as_ref();
        let repo_root = &self.context.repo_root.canonicalize()?;
        let processor = FileProcessor::new(self.context);
        let scanner = FileScanner::new(repo_root.clone());

        let tracked_files = self.context.database.get_all_files().await?;
        let files = scanner.get_all_files(repo_root)?;

        let (_, _, deleted_files) = processor
            .detect_changes(&files, tracked_files.as_slice())
            .await?;

        let deleted_files: Vec<_> = deleted_files
            .iter()
            .filter(|f| pattern.is_none_or(|p| p.matches_path(f.path.as_path())))
            .collect();

        if deleted_files.is_empty() {
            info!("No matching files found to remove from tracking");
            return Ok(0);
        }

        let deleted_file_records: Vec<_> = self
            .context
            .database
            .get_files_by_paths(
                &deleted_files
                    .iter()
                    .filter_map(|f| f.path.to_str())
                    .collect(),
            )
            .await?;

        self.display_files_to_remove(&deleted_file_records);

        let deleted_file_records: Vec<_> = deleted_file_records
            .iter()
            .map(|file| (file.path.clone(), file.b3sum.clone(), file.size))
            .collect();

        let action_id = chrono::Utc::now().timestamp();
        self.context
            .database
            .batch_delete_file_records(action_id, deleted_file_records.as_slice())
            .await?;
        info!(
            "Removed {} deleted files from tracking",
            deleted_file_records.len()
        );
        Ok(deleted_file_records.len())
    }

    /// Display files that will be removed from tracking
    fn display_files_to_remove(&self, files: &[crate::database::FileRecord]) {
        if files.len() <= 5 {
            info!("Files to remove from tracking:");
            for file in files {
                info!("  {}", file.path);
            }
        } else {
            info!(
                "Files to remove from tracking (showing 5 out of {}):",
                files.len()
            );
            for file in files.iter().take(5) {
                info!("  {}", file.path);
            }
            info!("  ... and {} more", files.len() - 5);
        }
    }
}
