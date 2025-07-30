//! File tracking functionality for monitoring file changes and metadata storage.
//!
//! This module provides the `AddCommand` which handles the complete workflow
//! of scanning directories, detecting file changes, and updating the database
//! with new or modified file records. It also copies files to the object store
//! with CoW if supported.

use crate::{
    AppContext, DdriveError, Result,
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
    pub renamed_files: usize,
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
        let repo_root = &self.context.repo.root().canonicalize()?;
        let path = path.as_ref();
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
                self.context.repo.root().display()
            );
        }

        let files = scanner.get_all_files(add_path)?;
        if files.is_empty() {
            info!("No files found in {}", add_path.display());
            return Ok(AddResult {
                new_files: 0,
                changed_files: 0,
                renamed_files: 0,
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
        let (new_files, changed_files, deleted_files, renames) = self
            .processor
            .detect_changes(&files, tracked_files.as_slice(), true)
            .await?;

        self.display_summary(&changed_files, deleted_files.as_slice(), &renames);

        let action_id = chrono::Utc::now().timestamp();

        // Process renames first (most efficient)
        if !renames.is_empty() {
            info!("Processing {} file renames...", renames.len());
            self.process_renames(action_id, &renames).await?;
        }

        if !new_files.is_empty() {
            info!("Processing {} new files...", new_files.len());
            let new_files_refs: Vec<_> = new_files.iter().collect();
            self.process_new_files(action_id, &new_files_refs).await?;
        }

        // Process changed files
        if !changed_files.is_empty() {
            info!("Processing {} changed files...", changed_files.len());
            let changed_files: Vec<_> = changed_files.iter().collect();
            self.process_changed_files(action_id, &changed_files)
                .await?;
        }

        Ok(AddResult {
            new_files: new_files.len(),
            changed_files: changed_files.len(),
            renamed_files: renames.len(),
        })
    }

    /// Display summary of files to be processed
    fn display_summary(
        &self,
        changed_files: &[FileInfo],
        deleted_files: &[FileInfo],
        renames: &[(FileInfo, FileInfo)],
    ) {
        // Display renames
        if !renames.is_empty() && renames.len() <= 5 {
            info!("Renamed files:");
            for (old_file, new_file) in renames {
                info!(
                    "  {} → {}",
                    old_file.path.display(),
                    new_file.path.display()
                );
            }
        } else if renames.len() > 5 {
            info!("Renamed files (showing 5 out of {}):", renames.len());
            for (old_file, new_file) in renames.iter().take(5) {
                info!(
                    "  {} → {}",
                    old_file.path.display(),
                    new_file.path.display()
                );
            }
            info!("  ... and {} more", renames.len() - 5);
        }

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
    async fn process_new_files(&self, action_id: i64, files: &[&FileInfo]) -> Result<usize> {
        // Calculate checksums and create FileInfo objects with checksums
        let mut files_with_checksums = Vec::new();
        let mut failed_count = 0;

        for file_info in files {
            match self.processor.calculate_single_checksum(&file_info.path) {
                Ok(checksum) => {
                    if let Err(e) = self.copy_to_object_store(&file_info.path, &checksum) {
                        warn!(
                            "Failed to copy {} to object store: {}",
                            file_info.path.display(),
                            e
                        );
                        failed_count += 1;
                        continue;
                    }

                    let mut file_with_checksum = (*file_info).clone();
                    file_with_checksum.b3sum = Some(checksum);
                    files_with_checksums.push(file_with_checksum);
                }
                Err(e) => {
                    warn!(
                        "Failed to calculate checksum for {}: {}",
                        file_info.path.display(),
                        e
                    );
                    failed_count += 1;
                }
            }
        }

        if !files_with_checksums.is_empty() {
            let file_refs: Vec<&FileInfo> = files_with_checksums.iter().collect();
            self.context
                .database
                .batch_insert_file_records(action_id, &file_refs)
                .await?;
        }

        Ok(failed_count)
    }

    /// Process changed files by updating records and copying to object store
    async fn process_changed_files(&self, action_id: i64, files: &[&FileInfo]) -> Result<usize> {
        let mut failed_count = 0;
        for file_info in files.iter() {
            let b3sum = file_info.b3sum.as_ref().expect("b3sum");
            if let Err(e) = self.copy_to_object_store(&file_info.path, b3sum) {
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
    fn copy_to_object_store(&self, file_path: &Path, checksum: &str) -> Result<()> {
        // Create object store directory structure (first 2 chars / next 2 chars)
        let object_dir = self.context.repo.object_dir(checksum);

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

    /// Process file renames efficiently without recalculating checksums or copying files
    async fn process_renames(
        &self,
        action_id: i64,
        renames: &[(FileInfo, FileInfo)],
    ) -> Result<()> {
        let rename_pairs: Vec<(String, String)> = renames
            .iter()
            .map(|(old_file, new_file)| {
                (
                    old_file.path.to_string_lossy().into_owned(),
                    new_file.path.to_string_lossy().into_owned(),
                )
            })
            .collect();

        // For renames, we don't need to copy files to object store since the content is the same
        // and the object already exists from when the file was originally added
        self.context
            .database
            .batch_rename_files(action_id, &rename_pairs)
            .await?;

        Ok(())
    }
}
