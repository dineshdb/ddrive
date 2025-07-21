use crate::{
    AppContext, DdriveError, Result, config::Config, database::FileRecord, utils::FileProcessor,
};
use chrono::DateTime;
use glob::Pattern;
use tracing::{debug, info, warn};

pub struct VerifyCommand<'a> {
    context: &'a AppContext,
    processor: FileProcessor<'a>,
}

#[derive(Debug)]
pub struct VerifyResult {
    pub checked_files: usize,
    pub passed_files: usize,
    pub failed_files: usize,
    pub skipped_files: usize,
    pub failures: Vec<IntegrityFailure>,
}

#[derive(Debug)]
pub struct IntegrityFailure {
    pub file_path: String,
    pub expected_checksum: String,
    pub actual_checksum: String,
}

impl<'a> VerifyCommand<'a> {
    pub fn new(context: &'a AppContext) -> Self {
        VerifyCommand {
            context,
            processor: FileProcessor::new(context),
        }
    }

    /// Execute the verify command with optional filters and force option
    pub async fn execute(
        &self,
        path_filter: Option<&Pattern>,
        force: bool,
    ) -> Result<VerifyResult> {
        // Load configuration
        let config = Config::load(&self.context.repo_root)?;

        // Get all files that match the filter
        let files_to_check = self
            .get_files_for_verification(path_filter, force, &config)
            .await?;

        if files_to_check.is_empty() {
            info!("No files need verification at this time");
            return Ok(VerifyResult {
                checked_files: 0,
                passed_files: 0,
                failed_files: 0,
                skipped_files: 0,
                failures: Vec::new(),
            });
        }

        info!("Verifying {} files", files_to_check.len());

        let mut result = VerifyResult {
            checked_files: 0,
            passed_files: 0,
            failed_files: 0,
            skipped_files: 0,
            failures: Vec::new(),
        };

        for file_record in &files_to_check {
            match self.verify_file(file_record, force).await {
                Ok(verification_result) => {
                    result.checked_files += 1;

                    if verification_result.passed {
                        result.passed_files += 1;
                        info!("✓ {}", file_record.path);

                        let absolute_path = self.resolve_absolute_path(&file_record.path)?;
                        if let Err(e) = self
                            .context
                            .database
                            .update_last_checked(&absolute_path.to_string_lossy())
                            .await
                        {
                            warn!(
                                "Failed to update last_checked timestamp for {}: {}",
                                file_record.path, e
                            );
                        }
                    } else {
                        result.failed_files += 1;
                        warn!("✗ {}", file_record.path);

                        result.failures.push(IntegrityFailure {
                            file_path: file_record.path.clone(),
                            expected_checksum: file_record.b3sum.clone(),
                            actual_checksum: verification_result.actual_checksum,
                        });
                    }
                }
                Err(e) => {
                    warn!("Error verifying {}: {}", file_record.path, e);
                    result.failed_files += 1;
                }
            }
        }

        self.display_summary(&result);
        Ok(result)
    }

    /// Get files that need verification based on last_checked timestamps and optional path filter
    async fn get_files_for_verification(
        &self,
        path_filter: Option<&Pattern>,
        force: bool,
        config: &Config,
    ) -> Result<Vec<FileRecord>> {
        let mut files = if force {
            // When force is true, get all files regardless of last_checked timestamp
            self.context.database.get_all_files().await?
        } else {
            // Otherwise, get files that haven't been checked within the configured interval
            self.context
                .database
                .get_files_not_checked_since(config.verify.cutoff_date())
                .await?
        };

        if let Some(filter) = path_filter {
            files.retain(|file| filter.matches(&file.path));
        }

        Ok(files)
    }

    /// Verify a single file's integrity
    /// Optimized to check metadata first before calculating expensive checksums
    async fn verify_file(
        &self,
        file_record: &FileRecord,
        force: bool,
    ) -> Result<VerificationResult> {
        let absolute_path = self.resolve_absolute_path(&file_record.path)?;

        if !absolute_path.exists() {
            return Err(DdriveError::FileSystem {
                message: format!("File no longer exists: {}", absolute_path.display()),
            });
        }

        // If force is true, skip metadata check and go straight to checksum verification
        if !force {
            // First check metadata (size, modified time) before expensive checksum calculation
            // This is a significant optimization for large files that haven't changed
            if let Ok(metadata_changed) = self.check_metadata_changes(&absolute_path, file_record) {
                if !metadata_changed {
                    // Metadata hasn't changed, assume file is still valid without calculating checksum
                    debug!(
                        "Skipping checksum verification for {} (metadata unchanged)",
                        file_record.path
                    );
                    return Ok(VerificationResult {
                        passed: true,
                        actual_checksum: file_record.b3sum.clone(),
                    });
                }
            }
        }

        // Metadata changed or couldn't be read, or force is true, do full checksum verification
        debug!(
            "Performing full checksum verification for {}",
            file_record.path
        );
        let actual_checksum = self.processor.calculate_single_checksum(&absolute_path)?;
        let passed = actual_checksum == file_record.b3sum;

        Ok(VerificationResult {
            passed,
            actual_checksum,
        })
    }

    /// Check if file metadata (size, modified time) has changed
    /// This is a fast pre-check before doing expensive checksum calculation
    fn check_metadata_changes(
        &self,
        file_path: &std::path::Path,
        file_record: &FileRecord,
    ) -> Result<bool> {
        let metadata = std::fs::metadata(file_path).map_err(|e| DdriveError::FileSystem {
            message: format!("Could not read metadata for {}: {}", file_path.display(), e),
        })?;

        let current_size = metadata.len();
        let stored_size = file_record.size as u64;

        // If size is different, we know the file has changed
        let size_changed = current_size != stored_size;
        if size_changed {
            return Ok(true);
        }

        // Only check modified time if size is the same
        let modified_time_changed = if let Ok(modified) = metadata.modified() {
            // Convert file's modified time to naive datetime for comparison
            if let Ok(system_time_since_epoch) = modified.duration_since(std::time::UNIX_EPOCH) {
                let file_modified = DateTime::from_timestamp(
                    system_time_since_epoch.as_secs() as i64,
                    system_time_since_epoch.subsec_nanos(),
                )
                .map(|dt| dt.naive_utc());

                if let Some(file_modified) = file_modified {
                    // Allow for small timestamp differences (1 second) due to filesystem precision
                    let time_diff = (file_modified - file_record.updated_at).num_seconds().abs();
                    time_diff > 1
                } else {
                    true // Couldn't parse time, assume changed
                }
            } else {
                true // Couldn't get duration, assume changed
            }
        } else {
            true // Couldn't get modified time, assume changed
        };

        Ok(size_changed || modified_time_changed)
    }

    /// Convert relative path from database to absolute path for file access
    fn resolve_absolute_path(&self, relative_path: &str) -> Result<std::path::PathBuf> {
        Ok(self.context.repo_root.join(relative_path))
    }

    /// Display summary of check results
    fn display_summary(&self, result: &VerifyResult) {
        info!(
            "Verification complete: {}/{} passed, {} failed, {} skipped",
            result.passed_files, result.checked_files, result.failed_files, result.skipped_files
        );

        if !result.failures.is_empty() {
            warn!("Integrity failures:");
            for failure in &result.failures {
                warn!("  {}: checksum mismatch", failure.file_path);
                warn!("    Expected: {}", failure.expected_checksum);
                warn!("    Actual:   {}", failure.actual_checksum);
            }
        }

        if result.failed_files > 0 {
            warn!(
                "⚠️  {} file(s) failed integrity verification!",
                result.failed_files
            );
        } else if result.checked_files > 0 {
            info!("✅ All files passed integrity verification!");
        }
    }
}

#[derive(Debug)]
struct VerificationResult {
    passed: bool,
    actual_checksum: String,
}
