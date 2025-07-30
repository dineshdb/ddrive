use crate::{AppContext, Result, database::FileRecord, utils};
use glob::Pattern;
use reflink_copy;
use std::collections::HashMap;
use tracing::{debug, error, info};

pub struct DedupCommand<'a> {
    context: &'a AppContext,
    path_filter: Option<String>,
}

#[derive(Debug)]
pub struct DuplicateGroup {
    pub checksum: String,
    pub files: Vec<String>,
    pub file_size: i64,
}

impl<'a> DedupCommand<'a> {
    pub fn new(context: &'a AppContext) -> Self {
        Self {
            context,
            path_filter: None,
        }
    }

    pub fn with_path_filter(context: &'a AppContext, path_filter: String) -> Self {
        Self {
            context,
            path_filter: Some(path_filter),
        }
    }

    pub async fn execute(&self) -> Result<Vec<DuplicateGroup>> {
        let all_files = self.context.database.find_duplicates().await?;

        // Apply path filter if specified
        let filtered_files = if let Some(filter) = &self.path_filter {
            info!("Filtering duplicates with pattern: {}", filter);
            let pattern = Pattern::new(filter)?;
            all_files
                .into_iter()
                .filter(|file| pattern.matches(&file.path))
                .collect()
        } else {
            all_files
        };

        let duplicates = self.group_duplicates(filtered_files);

        if duplicates.is_empty() {
            info!("No duplicate files found");
            return Ok(duplicates);
        } else {
            self.display_duplicates(&duplicates)?;
            self.process_duplicates(&duplicates)?;
        }

        Ok(duplicates)
    }

    fn group_duplicates(&self, files: Vec<FileRecord>) -> Vec<DuplicateGroup> {
        // Pre-allocate HashMap with estimated capacity for better performance
        let mut checksum_groups: HashMap<String, Vec<FileRecord>> =
            HashMap::with_capacity(files.len() / 2);

        // Group files by checksum
        for file in files {
            checksum_groups
                .entry(file.b3sum.clone())
                .or_default()
                .push(file);
        }

        // Convert to duplicates using iterator chain for better memory efficiency
        let mut duplicates: Vec<_> = checksum_groups
            .into_iter()
            .filter_map(|(checksum, files)| {
                if files.len() > 1 {
                    Some(DuplicateGroup {
                        checksum,
                        file_size: files[0].size,
                        files: files.into_iter().map(|f| f.path).collect(),
                    })
                } else {
                    None
                }
            })
            .collect();

        // Sort by wasted space (descending)
        duplicates.sort_by_key(|group| {
            std::cmp::Reverse(group.file_size * (group.files.len() as i64 - 1))
        });

        duplicates
    }

    fn display_duplicates(&self, duplicates: &[DuplicateGroup]) -> Result<()> {
        let mut total_wasted_space = 0i64;
        let total_groups = duplicates.len();

        if let Some(filter) = &self.path_filter {
            info!(
                "Found {} duplicate groups matching filter: {}",
                total_groups, filter
            );
        } else {
            info!("Found {} duplicate groups", total_groups);
        }

        // Show only top 10 largest duplicates (by wasted space)
        let display_count = std::cmp::min(10, duplicates.len());

        if display_count < total_groups {
            info!("Showing top {} largest duplicate groups:", display_count);
        }

        for (i, group) in duplicates.iter().take(display_count).enumerate() {
            info!(
                "Group {} ({}): {} files, {} each",
                i + 1,
                &group.checksum[..8],
                group.files.len(),
                utils::format_size(group.file_size as u64)
            );

            // Show files for smaller groups, or just count for large groups
            if group.files.len() <= 5 {
                for file_path in &group.files {
                    info!("  {file_path}");
                }
            } else {
                info!("  {} files (showing first 3):", group.files.len());
                for file_path in group.files.iter().take(3) {
                    info!("  {file_path}");
                }
                info!("  ... and {} more", group.files.len() - 3);
            }

            let wasted = group.file_size * (group.files.len() as i64 - 1);
            total_wasted_space += wasted;
            info!("  Wasted: {}", utils::format_size(wasted as u64));
        }

        // If there are more groups than we displayed, show a summary
        if display_count < total_groups {
            info!(
                "... and {} more duplicate groups",
                total_groups - display_count
            );
        }

        info!(
            "Total wasted space: {}",
            utils::format_size(total_wasted_space as u64)
        );

        Ok(())
    }

    /// Process duplicate groups by automatically reflinking duplicates and creating backups in .ddrive/objects
    fn process_duplicates(&self, duplicates: &[DuplicateGroup]) -> Result<()> {
        // Create the objects directory if it doesn't exist
        let objects_dir = ".ddrive/objects";
        std::fs::create_dir_all(objects_dir)?;

        for (i, group) in duplicates.iter().enumerate() {
            // Always keep the first file and replace others with reflinks
            let file_to_keep = &group.files[0];
            debug!(
                "Processing duplicate group {} of {} ({}). Keeping: {}",
                i + 1,
                duplicates.len(),
                &group.checksum[..8],
                file_to_keep
            );

            // Create a copy at object store
            let object_dir = self.context.repo.object_dir(&group.checksum);
            let backup_path = object_dir.join(group.checksum.clone());
            std::fs::create_dir_all(&object_dir)?;
            if !std::path::Path::new(&backup_path).exists() {
                reflink_copy::reflink_or_copy(file_to_keep, &backup_path)?;
            }

            // Process each file except the one we're keeping
            for other_file in group.files.iter().skip(1) {
                debug!("Replacing {other_file} with reflink to {file_to_keep}");

                // Delete the file first
                if let Err(e) = std::fs::remove_file(other_file) {
                    error!("Error removing file {other_file}: {e}");
                    continue;
                }

                // Create reflink copy
                if let Err(e) = reflink_copy::reflink_or_copy(file_to_keep, other_file) {
                    error!("Error creating reflink: {e}",);
                }
            }
        }

        if let Some(filter) = &self.path_filter {
            info!("\nDeduplication process completed for files matching: {filter}");
        } else {
            info!("\nDeduplication process completed.");
        }
        Ok(())
    }
}
