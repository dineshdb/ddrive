use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;
use std::{collections::HashSet, time::UNIX_EPOCH};
use tracing::{debug, warn};
use unicode_segmentation::UnicodeSegmentation;

use crate::{
    AppContext, Result, checksum::ChecksumCalculator, database::FileRecord, scanner::FileInfo,
};
use rayon::prelude::*;

/// Shared utilities for file processing operations
pub struct FileProcessor<'a> {
    context: &'a AppContext,
    checksum_calculator: ChecksumCalculator,
}

impl<'a> FileProcessor<'a> {
    pub fn new(context: &'a AppContext) -> Self {
        Self {
            context,
            checksum_calculator: ChecksumCalculator::new(),
        }
    }

    /// Process files in parallel for checksum calculation, reusing existing checksums
    pub fn calculate_checksums_parallel(&self, files: &[&FileInfo]) -> Vec<(String, String, i64)> {
        let start_time = Instant::now();

        // Separate files that need calculation from those with existing checksums
        let (files_with_checksums, files_needing_calculation): (Vec<_>, Vec<_>) =
            files.iter().partition(|file| file.b3sum.is_some());

        // Process files with existing checksums (no calculation needed)
        let mut results: Vec<_> = files_with_checksums
            .into_iter()
            .map(|file: &FileInfo| {
                let file_path_str = file.path.to_string_lossy().into_owned();
                let checksum = file.b3sum.as_ref().unwrap().clone();
                (file_path_str, checksum, file.size as i64)
            })
            .collect();

        // Calculate checksums for remaining files in parallel
        let calculated_results: Vec<_> = files_needing_calculation
            .par_iter()
            .filter_map(
                |file| match self.checksum_calculator.calculate_checksum(&file.path) {
                    Ok(checksum) => {
                        let file_path_str = file.path.to_string_lossy().into_owned();
                        Some((file_path_str, checksum, file.size as i64))
                    }
                    Err(e) => {
                        warn!("Checksum error for {}: {}", file.path.display(), e);
                        None
                    }
                },
            )
            .collect();

        results.extend(calculated_results);

        let reused_count = results.len() - files_needing_calculation.len();
        debug!(
            "Processed {} checksums ({} calculated, {} reused) in {:.2}ms",
            results.len(),
            files_needing_calculation.len(),
            reused_count,
            start_time.elapsed().as_millis()
        );
        results
    }

    /// Internal method that handles both lightweight and full change detection
    pub async fn detect_changes(
        &self,
        scanned_files: &[FileInfo],
        tracked_files: &[FileRecord],
        use_checksums: bool,
    ) -> Result<(
        Vec<FileInfo>,
        Vec<FileInfo>,
        Vec<FileInfo>,
        Vec<(FileInfo, FileInfo)>,
    )> {
        let mut new_files = Vec::new();
        let mut changed_files = Vec::new();
        let mut deleted_files = Vec::new();

        // Build a hash map of scanned paths for quick lookups (avoid cloning paths)
        let scanned_paths: HashMap<&PathBuf, &FileInfo> = scanned_files
            .iter()
            .map(|file| (&file.path, file))
            .collect();

        // Find deleted files (avoid creating PathBuf for each lookup)
        for tracked_file in tracked_files {
            let tracked_path = PathBuf::from(&tracked_file.path);
            if !scanned_paths.contains_key(&tracked_path) {
                deleted_files.push(tracked_file.into());
            }
        }

        // Create a lookup map from tracked files for O(1) access (avoid database calls)
        let tracked_lookup: HashMap<&str, &FileRecord> = tracked_files
            .iter()
            .map(|record| (record.path.as_str(), record))
            .collect();

        // Process scanned files using the lookup map
        for file in scanned_files {
            let file_path_str = file.path.to_string_lossy();
            match tracked_lookup.get(file_path_str.as_ref()) {
                Some(record) => {
                    let modified_time = file
                        .modified
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_err(|e| crate::DdriveError::FileSystem {
                            message: format!("Invalid modification time: {e:?}"),
                        })?
                        .as_secs();

                    // Skip if size and time haven't changed
                    if file.size == record.size as u64
                        && modified_time <= record.updated_at.and_utc().timestamp() as u64
                    {
                        continue;
                    }

                    if use_checksums {
                        // Reuse existing checksum if available, otherwise calculate
                        let current_checksum = if let Some(ref existing_checksum) = file.b3sum {
                            existing_checksum.clone()
                        } else {
                            self.checksum_calculator.calculate_checksum(&file.path)?
                        };

                        if current_checksum != record.b3sum {
                            let mut changed_file = file.clone();
                            changed_file.b3sum = Some(current_checksum);
                            changed_files.push(changed_file);
                        }
                    } else {
                        // For lightweight mode, assume file changed if size/time differs
                        let mut changed_file = file.clone();
                        changed_file.b3sum = None;
                        changed_files.push(changed_file);
                    }
                }
                None => {
                    new_files.push(file.clone());
                }
            }
        }

        // Detect potential renames based on metadata
        let potential_renames = if use_checksums {
            // Full rename detection with checksums
            let new_files_with_checksums = self.ensure_checksums_for_files(&new_files).await?;
            self.context
                .database
                .find_potential_renames(&deleted_files, &new_files_with_checksums)
                .await?
        } else {
            // Lightweight rename detection based on size and modification time
            self.find_potential_renames_by_metadata(&deleted_files, &new_files)
        };

        // Remove renamed files from new_files and deleted_files lists
        let rename_new_paths: HashSet<_> = potential_renames
            .iter()
            .map(|(_, new_file)| &new_file.path)
            .collect();
        let rename_old_paths: HashSet<_> = potential_renames
            .iter()
            .map(|(old_file, _)| &old_file.path)
            .collect();

        // Filter out files involved in renames
        new_files.retain(|f| !rename_new_paths.contains(&f.path));
        deleted_files.retain(|f| !rename_old_paths.contains(&f.path));

        Ok((new_files, changed_files, deleted_files, potential_renames))
    }

    /// Find potential renames based on file metadata (size and creation time) without checksums
    fn find_potential_renames_by_metadata(
        &self,
        deleted_files: &[FileInfo],
        new_files: &[FileInfo],
    ) -> Vec<(FileInfo, FileInfo)> {
        fn creation_time_secs(file: &FileInfo) -> Option<u64> {
            file.created
                .duration_since(UNIX_EPOCH)
                .ok()
                .map(|d| d.as_secs())
        }

        fn group_by_key(files: &[FileInfo]) -> HashMap<(u64, Option<u64>), Vec<&FileInfo>> {
            let mut map: HashMap<(u64, Option<u64>), Vec<&FileInfo>> = HashMap::new();
            for file in files {
                let key = (file.size, creation_time_secs(file));
                map.entry(key).or_default().push(file);
            }
            map
        }

        let deleted_by_key = group_by_key(deleted_files);
        let new_by_key = group_by_key(new_files);

        let mut renames = Vec::new();

        for (key, deleted_group) in deleted_by_key {
            if let Some(new_group) = new_by_key.get(&key) {
                // Match first deleted with first new file of same metadata
                if let (Some(&deleted), Some(&new)) = (deleted_group.first(), new_group.first()) {
                    let mut new_file = new.clone();
                    new_file.b3sum = None; // Clear checksum for lightweight mode
                    renames.push((deleted.clone(), new_file));
                }
            }
        }

        renames
    }

    /// Ensure checksums are present for a list of files, reusing existing ones
    async fn ensure_checksums_for_files(&self, files: &[FileInfo]) -> Result<Vec<FileInfo>> {
        // Separate files that already have checksums from those that need calculation
        let (files_with_checksums, files_needing_checksums): (Vec<_>, Vec<_>) =
            files.iter().partition(|file| file.b3sum.is_some());

        let mut result = Vec::with_capacity(files.len());

        // Add files that already have checksums (no cloning needed for checksum calculation)
        result.extend(files_with_checksums.into_iter().cloned());

        // Calculate checksums for remaining files
        // Use parallel processing if we have many files to process
        if files_needing_checksums.len() > 10 {
            let calculated_files: Result<Vec<_>> = files_needing_checksums
                .par_iter()
                .map(|file| {
                    let checksum = self.checksum_calculator.calculate_checksum(&file.path)?;
                    let mut file_with_checksum = (*file).clone();
                    file_with_checksum.b3sum = Some(checksum);
                    Ok(file_with_checksum)
                })
                .collect();
            result.extend(calculated_files?);
        } else {
            // Sequential processing for small numbers of files
            for file in files_needing_checksums {
                let checksum = self.checksum_calculator.calculate_checksum(&file.path)?;
                let mut file_with_checksum = file.clone();
                file_with_checksum.b3sum = Some(checksum);
                result.push(file_with_checksum);
            }
        }

        Ok(result)
    }

    /// Calculate checksum for a single file
    pub fn calculate_single_checksum<P: AsRef<std::path::Path>>(&self, path: P) -> Result<String> {
        self.checksum_calculator.calculate_checksum(path)
    }
}

/// Format file size in human-readable format
pub fn format_size(size: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;

    if size < KB {
        format!("{size} B",)
    } else if size < MB {
        format!("{:.1} KB", size as f64 / KB as f64)
    } else if size < GB {
        format!("{:.1} MB", size as f64 / MB as f64)
    } else if size < TB {
        format!("{:.2} GB", size as f64 / GB as f64)
    } else {
        format!("{:.2} TB", size as f64 / TB as f64)
    }
}

/// Shorten a path with ellipsis if it's too long, with proper Unicode support
pub fn shorten_path(path: &str, max_length: usize) -> String {
    // Count grapheme clusters (visible characters) instead of bytes or code points
    let graphemes: Vec<&str> = path.graphemes(true).collect();

    if graphemes.len() <= max_length {
        return path.to_string();
    }

    // For very short max_length, just truncate with ellipsis
    if max_length <= 10 {
        let prefix: String = graphemes.iter().take(max_length - 3).copied().collect();
        return format!("{prefix}...",);
    }

    // Find path components with Unicode awareness
    // We need to handle path separators that might be different in different systems
    let components: Vec<&str> = path.split(['/', '\\']).filter(|s| !s.is_empty()).collect();

    if components.len() <= 2 {
        // If there are only one or two components, truncate with grapheme awareness
        let prefix: String = graphemes.iter().take(max_length - 3).copied().collect();
        return format!("{prefix}...",);
    }

    // Keep first and last component, replace middle with ellipsis
    let first = components.first().unwrap_or(&"");
    let last = components.last().unwrap_or(&"");

    // Calculate how much space we have for first and last parts
    let available_space = max_length - 3; // 3 for "..."

    // Count graphemes in components
    let first_graphemes: Vec<&str> = first.graphemes(true).collect();
    let last_graphemes: Vec<&str> = last.graphemes(true).collect();

    let first_len = first_graphemes.len().min(available_space / 2);
    let last_len = last_graphemes.len().min(available_space - first_len);

    // Adjust first_len if needed
    let first_len = first_len.min(available_space - last_len);

    // Build the shortened path
    let first_part: String = first_graphemes.iter().take(first_len).copied().collect();
    let last_part: String = last_graphemes.iter().take(last_len).copied().collect();

    format!("{first_part}...{last_part}",)
}

/// Group files by directory for better summary display, focusing on top-level directories
/// with proper Unicode support
pub fn group_files_by_directory(
    files: &[String],
) -> std::collections::BTreeMap<String, Vec<(String, u64)>> {
    let mut dir_groups: std::collections::BTreeMap<String, Vec<(String, u64)>> =
        std::collections::BTreeMap::new();
    let mut top_level_dirs: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut deep_paths: std::collections::HashMap<String, Vec<(String, u64)>> =
        std::collections::HashMap::new();

    // First pass: identify top-level directories and collect files
    for file_path in files {
        // Handle paths with both forward and backward slashes for cross-platform compatibility
        let normalized_path = file_path.replace('\\', "/");
        let path = std::path::Path::new(&normalized_path);

        // Get the top-level directory with proper Unicode handling
        let top_dir = path
            .components()
            .skip(1) // Skip the root if present
            .take(1) // Take only the first component
            .next()
            .map(|c| c.as_os_str().to_string_lossy().to_string())
            .unwrap_or_else(|| String::from("./"));

        // Store the top-level directory
        if top_dir != "./" {
            top_level_dirs.insert(top_dir);
        }

        // Get the full parent directory path
        let parent_dir = path
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| String::from(""));

        // Use empty string for root directory
        let dir_key = if parent_dir.is_empty() {
            String::from("./")
        } else {
            parent_dir
        };

        // Get file name with proper Unicode handling
        let file_name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| normalized_path.clone());

        // Store the file with its full path directory
        deep_paths.entry(dir_key).or_default().push((file_name, 0));
    }

    // Second pass: organize files by top-level directory or full path as needed
    for (dir_path, files) in deep_paths {
        if dir_path == "./" {
            // Root directory files go directly to the output
            dir_groups.entry(dir_path).or_default().extend(files);
        } else {
            // For other directories, check if it's a top-level directory or a subdirectory
            let is_top_level = top_level_dirs.contains(&dir_path);
            let top_dir = std::path::Path::new(&dir_path)
                .components()
                .skip(1)
                .take(1)
                .next()
                .map(|c| c.as_os_str().to_string_lossy().to_string())
                .unwrap_or_else(|| dir_path.clone());

            if is_top_level || top_dir == "./" {
                // This is a top-level directory, add files directly
                dir_groups.entry(dir_path).or_default().extend(files);
            } else {
                // This is a subdirectory, add files to the parent top-level directory
                // with subdirectory prefix in the filename
                let subdir_name = dir_path
                    .strip_prefix(&format!("{top_dir}/",))
                    .unwrap_or(&dir_path);

                for (file_name, size) in files {
                    let prefixed_name = if !subdir_name.is_empty() {
                        format!("{subdir_name}/{file_name}",)
                    } else {
                        file_name
                    };
                    dir_groups
                        .entry(top_dir.clone())
                        .or_default()
                        .push((prefixed_name, size));
                }
            }
        }
    }

    // Limit the number of top directories if there are too many
    const MAX_TOP_DIRS: usize = 10;
    if dir_groups.len() > MAX_TOP_DIRS {
        // Create a new map with limited entries
        let mut limited_groups = std::collections::BTreeMap::new();
        let mut other_files = Vec::new();
        let mut count = 0;

        for (dir, files) in dir_groups {
            if count < MAX_TOP_DIRS - 1 {
                limited_groups.insert(dir, files);
                count += 1;
            } else {
                // Collect remaining files under "Other directories"
                other_files.extend(
                    files
                        .into_iter()
                        .map(|(name, size)| (format!("{dir}/{name}",), size)),
                );
            }
        }

        // Add the "Other directories" entry if we have files there
        if !other_files.is_empty() {
            limited_groups.insert("Other directories".to_string(), other_files);
        }

        return limited_groups;
    }

    dir_groups
}

/// Display a directory listing with files in a user-friendly format
/// with proper Unicode support for paths and emojis
pub fn display_directory_listing(
    dir_groups: &std::collections::BTreeMap<String, Vec<(String, u64)>>,
    max_path_length: usize,
    max_samples: usize,
) -> Vec<String> {
    let mut output = Vec::new();

    // Define file type emojis based on extensions
    let get_file_emoji = |name: &str| -> &str {
        let lowercase = name.to_lowercase();
        if lowercase.ends_with(".jpg")
            || lowercase.ends_with(".jpeg")
            || lowercase.ends_with(".png")
            || lowercase.ends_with(".gif")
            || lowercase.ends_with(".webp")
            || lowercase.ends_with(".svg")
        {
            "🖼️ " // Image files
        } else if lowercase.ends_with(".mp4")
            || lowercase.ends_with(".mov")
            || lowercase.ends_with(".avi")
            || lowercase.ends_with(".mkv")
        {
            "🎬 " // Video files
        } else if lowercase.ends_with(".mp3")
            || lowercase.ends_with(".wav")
            || lowercase.ends_with(".ogg")
            || lowercase.ends_with(".flac")
        {
            "🎵 " // Audio files
        } else if lowercase.ends_with(".pdf") {
            "📄 " // PDF documents
        } else if lowercase.ends_with(".doc")
            || lowercase.ends_with(".docx")
            || lowercase.ends_with(".txt")
            || lowercase.ends_with(".md")
        {
            "📝 " // Text documents
        } else if lowercase.ends_with(".xls")
            || lowercase.ends_with(".xlsx")
            || lowercase.ends_with(".csv")
        {
            "📊 " // Spreadsheets
        } else if lowercase.ends_with(".zip")
            || lowercase.ends_with(".tar")
            || lowercase.ends_with(".gz")
            || lowercase.ends_with(".7z")
        {
            "🗜️ " // Archives
        } else if lowercase.ends_with(".exe")
            || lowercase.ends_with(".app")
            || lowercase.ends_with(".sh")
            || lowercase.ends_with(".bat")
        {
            "⚙️ " // Executables
        } else if name.contains("/") {
            "📂 " // Subdirectory
        } else {
            "📄 " // Default file
        }
    };

    for (dir, files) in dir_groups {
        let file_count = files.len();

        // Handle directory name with proper Unicode support
        let dir_display = if dir == "./" {
            "Root directory".to_string()
        } else {
            shorten_path(dir, max_path_length)
        };

        // Use folder emoji with proper Unicode handling
        output.push(format!("  📁 {dir_display} - {file_count} files"));

        // Show sample files from each directory with appropriate emoji
        let sample_count = std::cmp::min(max_samples, file_count);
        for file in files.iter().take(sample_count) {
            let file_name = shorten_path(&file.0, max_path_length);
            let emoji = get_file_emoji(&file.0);
            output.push(format!("    • {emoji}{file_name}",));
        }

        // Show count of remaining files if there are more than the samples
        if file_count > sample_count {
            output.push(format!(
                "    • ... and {} more files",
                file_count - sample_count
            ));
        }
    }

    output
}
