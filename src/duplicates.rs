use crate::{AppContext, Result, database::FileRecord, utils};
use std::collections::HashMap;
use tracing::info;

pub struct DuplicatesCommand<'a> {
    context: &'a AppContext,
}

#[derive(Debug)]
pub struct DuplicateGroup {
    pub checksum: String,
    pub files: Vec<String>,
    pub file_size: i64,
}

impl<'a> DuplicatesCommand<'a> {
    pub fn new(context: &'a AppContext) -> Self {
        Self { context }
    }

    pub async fn execute(&self) -> Result<Vec<DuplicateGroup>> {
        let all_files = self.context.database.find_duplicates().await?;
        let duplicates = self.group_duplicates(all_files);

        if duplicates.is_empty() {
            info!("No duplicate files found");
        } else {
            self.display_duplicates(&duplicates)?;
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

        info!("Found {} duplicate groups", total_groups);

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
                    let absolute_path = self.context.repo_root.join(file_path);
                    info!("  {}", absolute_path.display());
                }
            } else {
                info!("  {} files (showing first 3):", group.files.len());
                for file_path in group.files.iter().take(3) {
                    let absolute_path = self.context.repo_root.join(file_path);
                    info!("  {}", absolute_path.display());
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
}
