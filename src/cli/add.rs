//! File tracking functionality for monitoring file changes and metadata storage.
//!
//! This module provides the `AddCommand` which handles the complete workflow
//! of scanning directories, detecting file changes, and updating the database
//! with new or modified file records. It also copies files to the object store
//! with CoW if supported.

use crate::{
    AppContext, DdriveError, Result,
    config::Config,
    scanner::{FileInfo, FileScanner},
    utils::FileProcessor,
};
use std::fs;
use std::path::Path;
use tracing::{debug, error, info, warn};

#[derive(Debug)]
pub struct AddResult {
    pub new_files: usize,
    pub changed_files: usize,
}

pub struct AddCommand<'a> {
    context: &'a AppContext,
    processor: FileProcessor<'a>,
}

impl<'a> AddCommand<'a> {
    pub fn new(context: &'a AppContext) -> Self {
        AddCommand {
            context,
            processor: FileProcessor::new(context),
        }
    }

    /// Execute the complete file tracking workflow
    pub async fn execute<P: AsRef<Path>>(&self, path: P) -> Result<AddResult> {
        let repo_root = &self.context.repo_root.canonicalize()?;
        let path = path.as_ref();
        let config = Config::load(&self.context.repo_root)?;

        let object_store_path = config.object_store_path(repo_root);
        let scanner = FileScanner::new(repo_root.clone());

        let add_path = &repo_root.join(path).canonicalize()?;
        if !add_path.starts_with(repo_root) {
            error!(
                "given path is not inside repo {}: {}",
                path.display(),
                repo_root.display()
            );
            return Err(DdriveError::InvalidDirectory);
        }

        if add_path == repo_root {
            info!("Adding all files to repo")
        } else {
            info!(
                "Adding {} to {}",
                path.display(),
                self.context.repo_root.display()
            );
        }

        let files = scanner.get_all_files(add_path)?;
        if files.is_empty() {
            info!("No files found in {}", add_path.display());
            return Ok(AddResult {
                new_files: 0,
                changed_files: 0,
            });
        }

        let path = path.to_str().expect("path error");
        let tracked_files = self.context.database.get_all_files().await?;
        let tracked_files = if add_path == repo_root {
            tracked_files
        } else {
            tracked_files
                .into_iter()
                .filter(|f| f.path.starts_with(path))
                .collect()
        };
        let (new_files, changed_files, deleted_files) = self
            .processor
            .detect_changes(&files, tracked_files.as_slice())
            .await?;

        self.display_summary(&changed_files, deleted_files.as_slice());

        let action_id = chrono::Utc::now().timestamp();
        if !new_files.is_empty() {
            info!("Processing {} new files...", new_files.len());
            self.process_new_files(action_id, &new_files, &object_store_path)
                .await?;
        }

        // Process changed files
        if !changed_files.is_empty() {
            info!("Processing {} changed files...", changed_files.len());
            let changed_files: Vec<_> = changed_files.iter().collect();
            self.process_changed_files(action_id, &changed_files, &object_store_path)
                .await?;
        }
        Ok(AddResult {
            new_files: new_files.len(),
            changed_files: changed_files.len(),
        })
    }

    /// Display summary of files to be processed
    fn display_summary(&self, changed_files: &[FileInfo], deleted_files: &[FileInfo]) {
        if !changed_files.is_empty() && changed_files.len() <= 5 {
            info!("Changed files:");
            for file in changed_files {
                info!("  {}", file.path.display());
            }
        } else if changed_files.len() > 5 {
            info!("Changed files (showing 5 out of {}):", changed_files.len());
            for file in changed_files.iter().take(5) {
                info!("  {}", file.path.display());
            }
            info!("  ... and {} more", changed_files.len() - 5);
        }

        if !deleted_files.is_empty() && deleted_files.len() <= 5 {
            info!("Deleted files:");
            for file in deleted_files {
                info!("  {}", file.path.display());
            }
        } else if deleted_files.len() > 5 {
            info!("Deleted files (showing first 5):");
            for file in deleted_files.iter().take(5) {
                info!("  {}", file.path.display());
            }
            info!("  ... and {} more", deleted_files.len() - 5);
        }
    }

    /// Process new files by calculating checksums, inserting records, and copying to object store
    async fn process_new_files(
        &self,
        action_id: i64,
        files: &[&FileInfo],
        object_store_path: &Path,
    ) -> Result<usize> {
        let checksums = self.processor.calculate_checksums_parallel(files);

        let mut failed_count = 0;
        for (file_info, checksum) in files.iter().zip(checksums.iter()) {
            if let Err(e) =
                self.copy_to_object_store(&file_info.path, &checksum.1, object_store_path)
            {
                warn!(
                    "Failed to copy {} to object store: {}",
                    file_info.path.display(),
                    e
                );
                failed_count += 1;
                continue;
            }

            self.context
                .database
                .batch_insert_file_records(action_id, &[checksum])
                .await?;
        }

        Ok(failed_count)
    }

    /// Process changed files by updating records and copying to object store
    async fn process_changed_files(
        &self,
        action_id: i64,
        files: &[&FileInfo],
        object_store_path: &Path,
    ) -> Result<usize> {
        let mut failed_count = 0;
        for file_info in files.iter() {
            let b3sum = file_info.b3sum.as_ref().expect("b3sum");
            if let Err(e) = self.copy_to_object_store(&file_info.path, b3sum, object_store_path) {
                warn!(
                    "Failed to copy {} to object store: {}",
                    file_info.path.display(),
                    e
                );
                failed_count += 1;
                continue;
            }

            self.context
                .database
                .batch_update_file_records(action_id, &[file_info])
                .await?;
        }

        Ok(failed_count)
    }

    /// Copy a file to the object store, using hard links when possible
    fn copy_to_object_store(
        &self,
        file_path: &Path,
        checksum: &str,
        object_store_path: &Path,
    ) -> Result<()> {
        // Create object store directory structure (first 2 chars / next 2 chars)
        let prefix1 = &checksum[0..2];
        let prefix2 = &checksum[2..4];
        let object_dir = object_store_path.join(prefix1).join(prefix2);

        if !object_dir.exists() {
            fs::create_dir_all(&object_dir).map_err(|e| DdriveError::FileSystem {
                message: format!("Failed to create object directory: {e}"),
            })?;
        }

        let object_path = object_dir.join(checksum);

        // If object already exists, no need to copy again
        if object_path.exists() {
            debug!("Object {} already exists in store", checksum);
            return Ok(());
        }

        reflink_copy::reflink_or_copy(file_path, object_path)?;
        Ok(())
    }
}
