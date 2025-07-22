use crate::{
    AppContext, Result,
    scanner::FileScanner,
    utils::{display_directory_listing, format_size, group_files_by_directory},
};
use std::collections::HashMap;
use tracing::info;

pub struct StatusCommand<'a> {
    context: &'a AppContext,
}

#[derive(Debug)]
pub struct RepositoryStats {
    pub tracked_files: usize,
    pub total_tracked_size: u64,
    pub untracked_files: usize,
    pub total_untracked_size: u64,
    pub duplicate_groups: usize,
    pub duplicate_files: usize,
    pub wasted_space: u64,
    pub files_needing_check: usize,
    pub newest_tracked: Option<chrono::NaiveDateTime>,
    pub new_files: Vec<String>,
    pub deleted_files: Vec<String>,
}

impl<'a> StatusCommand<'a> {
    pub fn new(context: &'a AppContext) -> Self {
        Self { context }
    }

    pub async fn execute(&self) -> Result<RepositoryStats> {
        let stats = self.gather_stats().await?;
        self.display_status(&stats);
        Ok(stats)
    }

    async fn gather_stats(&self) -> Result<RepositoryStats> {
        // Get lightweight tracked file info for status
        let tracked_files = self.context.database.get_tracked_file_paths().await?;
        let (tracked_count, total_tracked_size, newest_tracked) =
            self.analyze_tracked_file_info(&tracked_files);

        let files_needing_check = self.context.database.get_files_for_check().await?.len();

        // Get all file paths from the filesystem (lightweight scan)
        let scanner = FileScanner::new(self.context.repo.root().clone());
        let all_file_paths: Vec<_> = scanner
            .get_all_files(self.context.repo.root())?
            .iter()
            .map(|f| f.path.clone())
            .collect();

        // Compare filesystem and database to find new and deleted files
        let (new_files, deleted_files, untracked_count, total_untracked_size) = self
            .compare_file_paths_lightweight(&all_file_paths, &tracked_files)
            .await?;

        // Calculate duplicate statistics
        let (duplicate_groups, duplicate_files, wasted_space) = self.get_duplicate_stats().await?;

        Ok(RepositoryStats {
            tracked_files: tracked_count,
            total_tracked_size,
            untracked_files: untracked_count,
            total_untracked_size,
            duplicate_groups,
            duplicate_files,
            wasted_space,
            files_needing_check,
            newest_tracked,
            new_files,
            deleted_files,
        })
    }

    fn analyze_tracked_file_info(
        &self,
        tracked_files: &[crate::database::TrackedFileInfo],
    ) -> (usize, u64, Option<chrono::NaiveDateTime>) {
        let tracked_count = tracked_files.len();
        let total_tracked_size: u64 = tracked_files.iter().map(|f| f.size as u64).sum();
        let newest_tracked = tracked_files.iter().map(|f| f.created_at).max();

        (tracked_count, total_tracked_size, newest_tracked)
    }

    async fn compare_file_paths_lightweight(
        &self,
        file_paths: &[std::path::PathBuf],
        tracked_files: &[crate::database::TrackedFileInfo],
    ) -> Result<(Vec<String>, Vec<String>, usize, u64)> {
        let mut new_files = Vec::new();
        // We don't check for updated files in status command - that's what check is for
        let mut deleted_files = Vec::new();
        let mut untracked_count = 0;
        let mut total_untracked_size = 0u64;

        // Create efficient lookup structures
        let tracked_paths: std::collections::HashSet<String> =
            tracked_files.iter().map(|f| f.path.clone()).collect();

        let fs_paths: std::collections::HashSet<String> = file_paths
            .iter()
            .map(|path| {
                path.strip_prefix(self.context.repo.root())
                    .unwrap_or(path)
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();

        // Find new files (in filesystem but not tracked)
        for file_path in file_paths {
            let relative_path = file_path
                .strip_prefix(self.context.repo.root())
                .unwrap_or(file_path)
                .to_string_lossy();

            if !tracked_paths.contains(relative_path.as_ref()) {
                new_files.push(relative_path.into_owned());
                untracked_count += 1;

                // Only get file size for untracked files to calculate total
                if let Ok(metadata) = std::fs::metadata(file_path) {
                    total_untracked_size += metadata.len();
                }
            }
        }

        // Find deleted files (tracked but not in filesystem)
        for tracked_path in &tracked_paths {
            if !fs_paths.contains(tracked_path) {
                deleted_files.push(tracked_path.clone());
            }
        }

        Ok((
            new_files,
            deleted_files,
            untracked_count,
            total_untracked_size,
        ))
    }

    async fn get_duplicate_stats(&self) -> Result<(usize, usize, u64)> {
        let all_files = self.context.database.find_duplicates().await?;
        let mut checksum_groups: HashMap<String, Vec<_>> = HashMap::new();

        // Group files by checksum
        for file in all_files {
            checksum_groups
                .entry(file.b3sum.clone())
                .or_default()
                .push(file);
        }

        let mut duplicate_groups = 0;
        let mut duplicate_files = 0;
        let mut wasted_space = 0u64;

        for (_, files) in checksum_groups {
            if files.len() > 1 {
                duplicate_groups += 1;
                duplicate_files += files.len();
                wasted_space += (files[0].size as u64) * (files.len() as u64 - 1);
            }
        }

        Ok((duplicate_groups, duplicate_files, wasted_space))
    }

    // This method has been moved to utils.rs as a utility function

    fn display_status(&self, stats: &RepositoryStats) {
        info!("Any tracked files that have changed in filesystem are not shown here");
        // Define constants for path display
        const MAX_PATH_LENGTH: usize = 50; // Maximum length for displayed paths
        const MAX_SAMPLES: usize = 3; // Maximum number of sample files to show per directory

        // New files summary by directory
        if !stats.new_files.is_empty() {
            info!("New files found:");

            // Group files by directory using the utility function
            let grouped_files = group_files_by_directory(&stats.new_files);

            // Display directory listing using the utility function
            for line in display_directory_listing(&grouped_files, MAX_PATH_LENGTH, MAX_SAMPLES) {
                info!("{}", line);
            }
            info!("");
        }

        // Make it clear that check command should be used to verify file changes

        // Deleted files with more friendly wording
        if !stats.deleted_files.is_empty() {
            info!("Files no longer present:");

            // Group files by directory using the utility function
            let grouped_files = group_files_by_directory(&stats.deleted_files);

            // Display directory listing using the utility function
            for line in display_directory_listing(&grouped_files, MAX_PATH_LENGTH, MAX_SAMPLES) {
                info!("{}", line);
            }
            info!("");
        }

        // Integrity status section with more friendly wording
        if stats.files_needing_check > 0 {
            info!(
                "Files due for verification: {} files",
                stats.files_needing_check
            );
            info!("Run 'ddrive verify' to verify if any tracked files have changed");
        } else {
            info!("All your files have been verified recently");
        }
        info!("");

        // Tracked files section with more friendly wording
        info!("Protected files:");
        info!(
            "  {} files ({})",
            stats.tracked_files,
            format_size(stats.total_tracked_size)
        );

        if let Some(newest) = stats.newest_tracked {
            info!("  Last backup: {}", newest.format("%B %d, %Y at %H:%M"));
        }
        info!("");

        // Untracked files section with more friendly wording
        if stats.untracked_files > 0 {
            info!("Files not yet protected:");
            info!(
                "  {} files ({})",
                stats.untracked_files,
                format_size(stats.total_untracked_size)
            );
            info!("  Run 'ddrive add <path>' to protect these files");
            info!("");
        }

        // Duplicates section with more friendly wording
        if stats.duplicate_groups > 0 {
            info!("Duplicate files found:");
            info!(
                "  {} sets of duplicates with {} total files",
                stats.duplicate_groups, stats.duplicate_files
            );
            info!(
                "  Storage used by duplicates: {}",
                format_size(stats.wasted_space)
            );
            info!("  Run 'ddrive dedup' to see details");
            info!("");
        }

        // Repository summary with more friendly wording
        let total_files = stats.tracked_files + stats.untracked_files;
        let total_size = stats.total_tracked_size + stats.total_untracked_size;

        info!("Summary:");
        info!(
            "  Total: {} files ({})",
            total_files,
            format_size(total_size)
        );

        if stats.tracked_files > 0 && total_files > 0 {
            let tracking_percentage = (stats.tracked_files as f64 / total_files as f64) * 100.0;
            info!("  Protection coverage: {:.1}%", tracking_percentage);
        }
    }
}
