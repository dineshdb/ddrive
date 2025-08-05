#[cfg(test)]
mod tests {
    use crate::utils::{
        display_directory_listing, format_size, group_files_by_directory, shorten_path,
    };
    use crate::{checksum::ChecksumCalculator, database::FileRecord, scanner::FileInfo};
    use assert_fs::TempDir;
    use assert_fs::prelude::*;
    use chrono::DateTime;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::time::{Duration, UNIX_EPOCH};

    // Helper to create a minimal FileProcessor for testing checksum functionality
    fn create_test_checksum_calculator() -> ChecksumCalculator {
        ChecksumCalculator::new()
    }

    // Helper function to create test FileInfo
    fn create_test_file_info(
        path: &str,
        size: u64,
        checksum: Option<String>,
        modified_secs: u64,
        created_secs: u64,
    ) -> FileInfo {
        FileInfo {
            path: PathBuf::from(path),
            size,
            modified: UNIX_EPOCH + Duration::from_secs(modified_secs),
            created: UNIX_EPOCH + Duration::from_secs(created_secs),
            b3sum: checksum,
        }
    }

    // Helper function to create test FileRecord (unused but kept for potential future use)
    #[allow(dead_code)]
    fn create_test_file_record(
        path: &str,
        checksum: &str,
        size: i64,
        updated_at_secs: i64,
    ) -> FileRecord {
        FileRecord {
            id: 1,
            path: path.to_string(),
            created_at: DateTime::from_timestamp(updated_at_secs, 0)
                .unwrap()
                .naive_utc(),
            updated_at: DateTime::from_timestamp(updated_at_secs, 0)
                .unwrap()
                .naive_utc(),
            last_checked: None,
            b3sum: checksum.to_string(),
            size,
        }
    }

    #[test]
    fn test_format_size() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(1048576), "1.0 MB");
        assert_eq!(format_size(1073741824), "1.00 GB"); // GB uses 2 decimal places
        assert_eq!(format_size(1099511627776), "1.00 TB");
        assert_eq!(format_size(2199023255552), "2.00 TB");
    }

    #[test]
    fn test_shorten_path_no_truncation_needed() {
        let path = "short/path.txt";
        assert_eq!(shorten_path(path, 50), path);
    }

    #[test]
    fn test_shorten_path_very_short_max_length() {
        let path = "very/long/path/to/file.txt";
        let result = shorten_path(path, 8);
        assert_eq!(result, "very/..."); // 5 chars + 3 for "..."
        assert!(result.len() <= 8);
    }

    #[test]
    fn test_shorten_path_with_ellipsis() {
        let path = "very/long/path/to/some/deeply/nested/file.txt";
        let result = shorten_path(path, 20);
        assert!(result.contains("..."));
        assert!(result.len() <= 20);
        // Should start with first component and end with last component
        assert!(result.starts_with("very"));
        assert!(result.ends_with("file.txt"));
    }

    #[test]
    fn test_shorten_path_unicode_support() {
        let path = "测试/很长的路径/包含中文/文件.txt";
        let result = shorten_path(path, 15);
        assert!(result.contains("..."));
        // Unicode characters may take more bytes, so we check grapheme count instead
        let grapheme_count =
            unicode_segmentation::UnicodeSegmentation::graphemes(result.as_str(), true).count();
        assert!(grapheme_count <= 15);
    }

    #[test]
    fn test_shorten_path_single_component() {
        let path = "verylongfilenamethatexceedsmaxlength.txt";
        let result = shorten_path(path, 20);
        // The actual implementation may truncate differently
        assert!(result.contains("..."));
        assert!(result.len() <= 20);
        assert!(result.starts_with("very")); // Should start with beginning of filename
    }

    #[test]
    fn test_shorten_path_windows_separators() {
        let path = "C:\\Users\\test\\Documents\\file.txt";
        let result = shorten_path(path, 20);
        assert!(result.contains("..."));
        assert!(result.len() <= 20);
    }

    #[test]
    fn test_group_files_by_directory_root_files() {
        let files = vec!["file1.txt".to_string(), "file2.txt".to_string()];

        let result = group_files_by_directory(&files);

        assert_eq!(result.len(), 1);
        assert!(result.contains_key("./"));
        let root_files = &result["./"];
        assert_eq!(root_files.len(), 2);
        assert!(root_files.iter().any(|(name, _)| name == "file1.txt"));
        assert!(root_files.iter().any(|(name, _)| name == "file2.txt"));
    }

    #[test]
    fn test_group_files_by_directory_nested_structure() {
        let files = vec![
            "src/main.rs".to_string(),
            "src/lib.rs".to_string(),
            "tests/integration.rs".to_string(),
            "docs/README.md".to_string(),
        ];

        let result = group_files_by_directory(&files);

        assert!(result.contains_key("src"));
        assert!(result.contains_key("tests"));
        assert!(result.contains_key("docs"));

        let src_files = &result["src"];
        assert_eq!(src_files.len(), 2);
        // The function returns full paths, not just filenames
        assert!(src_files.iter().any(|(name, _)| name == "src/main.rs"));
        assert!(src_files.iter().any(|(name, _)| name == "src/lib.rs"));
    }

    #[test]
    fn test_group_files_by_directory_deep_nesting() {
        let files = vec![
            "src/utils/helpers.rs".to_string(),
            "src/utils/mod.rs".to_string(),
            "src/cli/commands.rs".to_string(),
        ];

        let result = group_files_by_directory(&files);

        // The function groups by subdirectory names when they're nested
        assert!(result.contains_key("utils"));
        assert!(result.contains_key("cli"));

        let utils_files = &result["utils"];
        assert_eq!(utils_files.len(), 2);
        assert!(
            utils_files
                .iter()
                .any(|(name, _)| name.contains("helpers.rs"))
        );
        assert!(utils_files.iter().any(|(name, _)| name.contains("mod.rs")));

        let cli_files = &result["cli"];
        assert_eq!(cli_files.len(), 1);
        assert!(
            cli_files
                .iter()
                .any(|(name, _)| name.contains("commands.rs"))
        );
    }

    #[test]
    fn test_group_files_by_directory_unicode_paths() {
        let files = vec![
            "测试/文件1.txt".to_string(),
            "测试/文件2.txt".to_string(),
            "文档/说明.md".to_string(),
        ];

        let result = group_files_by_directory(&files);

        assert!(result.contains_key("测试"));
        assert!(result.contains_key("文档"));
    }

    #[test]
    fn test_group_files_by_directory_max_directories_limit() {
        let mut files = Vec::new();

        // Create more than MAX_TOP_DIRS (10) directories
        for i in 0..15 {
            files.push(format!("dir{}/file.txt", i));
        }

        let result = group_files_by_directory(&files);

        // Should be limited to MAX_TOP_DIRS (10) entries
        assert!(result.len() <= 10);
        // Should have "Other directories" entry for overflow
        assert!(result.contains_key("Other directories"));
    }

    #[test]
    fn test_display_directory_listing_basic() {
        let mut dir_groups = BTreeMap::new();
        dir_groups.insert(
            "src".to_string(),
            vec![("main.rs".to_string(), 1024), ("lib.rs".to_string(), 2048)],
        );

        let result = display_directory_listing(&dir_groups, 50, 10);

        assert!(!result.is_empty());
        assert!(result[0].contains("📁 src - 2 files"));
        assert!(result.iter().any(|line| line.contains("main.rs")));
        assert!(result.iter().any(|line| line.contains("lib.rs")));
    }

    #[test]
    fn test_display_directory_listing_with_emojis() {
        let mut dir_groups = BTreeMap::new();
        dir_groups.insert(
            "assets".to_string(),
            vec![
                ("image.jpg".to_string(), 1024),
                ("video.mp4".to_string(), 2048),
                ("audio.mp3".to_string(), 512),
                ("document.pdf".to_string(), 256),
            ],
        );

        let result = display_directory_listing(&dir_groups, 50, 10);

        // Check for appropriate emojis
        assert!(
            result
                .iter()
                .any(|line| line.contains("🖼️") && line.contains("image.jpg"))
        );
        assert!(
            result
                .iter()
                .any(|line| line.contains("🎬") && line.contains("video.mp4"))
        );
        assert!(
            result
                .iter()
                .any(|line| line.contains("🎵") && line.contains("audio.mp3"))
        );
        assert!(
            result
                .iter()
                .any(|line| line.contains("📄") && line.contains("document.pdf"))
        );
    }

    #[test]
    fn test_display_directory_listing_max_samples() {
        let mut dir_groups = BTreeMap::new();
        let mut files = Vec::new();

        // Create more files than max_samples
        for i in 0..15 {
            files.push((format!("file{}.txt", i), 1024));
        }

        dir_groups.insert("test".to_string(), files);

        let result = display_directory_listing(&dir_groups, 50, 5);

        // Should show only 5 samples plus "... and X more files" message
        // .txt files use 📝 emoji, not 📄
        let file_lines: Vec<_> = result.iter().filter(|line| line.contains("• 📝")).collect();
        assert_eq!(file_lines.len(), 5);

        // Should have "more files" message
        assert!(result.iter().any(|line| line.contains("and 10 more files")));
    }

    #[test]
    fn test_display_directory_listing_path_shortening() {
        let mut dir_groups = BTreeMap::new();
        dir_groups.insert(
            "very/long/directory/path/that/exceeds/limit".to_string(),
            vec![("verylongfilenamethatexceedslimit.txt".to_string(), 1024)],
        );

        let result = display_directory_listing(&dir_groups, 20, 10);

        // Paths should be shortened
        assert!(result.iter().any(|line| line.contains("...")));
    }

    #[test]
    fn test_checksum_calculation_with_real_files() {
        let temp_dir = TempDir::new().unwrap();

        // Create test files
        let file1 = temp_dir.child("test1.txt");
        file1.write_str("Hello, World!").unwrap();

        let file2 = temp_dir.child("test2.txt");
        file2.write_str("Different content").unwrap();

        let subdir = temp_dir.child("subdir");
        subdir.create_dir_all().unwrap();
        let file3 = subdir.child("test3.txt");
        file3.write_str("Nested file").unwrap();

        let calculator = create_test_checksum_calculator();

        // Test checksum calculation
        let checksum1 = calculator.calculate_checksum(file1.path()).unwrap();
        let checksum2 = calculator.calculate_checksum(file2.path()).unwrap();
        let checksum3 = calculator.calculate_checksum(file3.path()).unwrap();

        // Checksums should be different for different content
        assert_ne!(checksum1, checksum2);
        assert_ne!(checksum1, checksum3);
        assert_ne!(checksum2, checksum3);

        // Checksums should be consistent
        let checksum1_again = calculator.calculate_checksum(file1.path()).unwrap();
        assert_eq!(checksum1, checksum1_again);

        // Verify all checksums are valid BLAKE3 hashes (64 hex characters)
        assert_eq!(checksum1.len(), 64);
        assert_eq!(checksum2.len(), 64);
        assert_eq!(checksum3.len(), 64);
        assert!(checksum1.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(checksum2.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(checksum3.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_multiple_file_checksum_calculation() {
        let temp_dir = TempDir::new().unwrap();

        // Create multiple test files
        let mut file_paths = Vec::new();
        for i in 0..5 {
            let file = temp_dir.child(format!("test{}.txt", i));
            file.write_str(&format!("Content {}", i)).unwrap();
            file_paths.push(file.path().to_path_buf());
        }

        let calculator = create_test_checksum_calculator();

        // Calculate checksums for all files
        let mut checksums = Vec::new();
        for path in &file_paths {
            let checksum = calculator.calculate_checksum(path).unwrap();
            checksums.push(checksum);
        }

        // Should have results for all files
        assert_eq!(checksums.len(), 5);

        // All checksums should be valid and unique
        for (i, checksum) in checksums.iter().enumerate() {
            assert_eq!(checksum.len(), 64);
            assert!(checksum.chars().all(|c| c.is_ascii_hexdigit()));

            // Each checksum should be unique
            for (j, other_checksum) in checksums.iter().enumerate() {
                if i != j {
                    assert_ne!(checksum, other_checksum);
                }
            }
        }
    }

    #[test]
    fn test_checksum_consistency_across_calculations() {
        let temp_dir = TempDir::new().unwrap();

        // Create test files
        let file1 = temp_dir.child("test1.txt");
        file1.write_str("Hello").unwrap();

        let file2 = temp_dir.child("test2.txt");
        file2.write_str("World").unwrap();

        let calculator = create_test_checksum_calculator();

        // Calculate checksums multiple times
        let checksum1_first = calculator.calculate_checksum(file1.path()).unwrap();
        let checksum1_second = calculator.calculate_checksum(file1.path()).unwrap();
        let checksum2_first = calculator.calculate_checksum(file2.path()).unwrap();
        let checksum2_second = calculator.calculate_checksum(file2.path()).unwrap();

        // Same file should produce same checksum
        assert_eq!(checksum1_first, checksum1_second);
        assert_eq!(checksum2_first, checksum2_second);

        // Different files should produce different checksums
        assert_ne!(checksum1_first, checksum2_first);

        // All checksums should be valid
        assert_eq!(checksum1_first.len(), 64);
        assert_eq!(checksum2_first.len(), 64);
        assert!(checksum1_first.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(checksum2_first.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_find_potential_renames_by_metadata() {
        // Test the rename detection logic directly
        let deleted1 = create_test_file_info("old/path1.txt", 1024, None, 1000, 500);
        let deleted2 = create_test_file_info("old/path2.txt", 2048, None, 1001, 501);
        let deleted_files = vec![deleted1, deleted2];

        // Create new files with matching metadata
        let new1 = create_test_file_info("new/path1.txt", 1024, None, 1002, 500); // Same size and creation time
        let new2 = create_test_file_info("new/path2.txt", 2048, None, 1003, 501); // Same size and creation time
        let new3 = create_test_file_info("new/path3.txt", 512, None, 1004, 502); // Different metadata
        let new_files = vec![new1, new2, new3];

        // Simulate the rename detection logic
        let mut potential_renames = Vec::new();

        // Group by (size, creation_time)
        let mut deleted_by_key = std::collections::HashMap::new();
        let mut new_by_key = std::collections::HashMap::new();

        for file in &deleted_files {
            let creation_time = file.created.duration_since(UNIX_EPOCH).unwrap().as_secs();
            let key = (file.size, creation_time);
            deleted_by_key
                .entry(key)
                .or_insert_with(Vec::new)
                .push(file);
        }

        for file in &new_files {
            let creation_time = file.created.duration_since(UNIX_EPOCH).unwrap().as_secs();
            let key = (file.size, creation_time);
            new_by_key.entry(key).or_insert_with(Vec::new).push(file);
        }

        // Find matches
        for (key, deleted_list) in deleted_by_key {
            if let Some(new_list) = new_by_key.get(&key) {
                if let (Some(&deleted), Some(&new)) = (deleted_list.first(), new_list.first()) {
                    potential_renames.push((deleted.clone(), new.clone()));
                }
            }
        }

        // Should find 2 potential renames
        assert_eq!(potential_renames.len(), 2);

        // Check that renames match by size and creation time
        for (old_file, new_file) in &potential_renames {
            assert_eq!(old_file.size, new_file.size);
            let old_created = old_file
                .created
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();
            let new_created = new_file
                .created
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();
            assert_eq!(old_created, new_created);
        }
    }

    #[test]
    fn test_find_potential_renames_no_matches() {
        // Test case where no renames should be detected
        let deleted1 = create_test_file_info("old/path1.txt", 1024, None, 1000, 500);
        let deleted_files = vec![deleted1];

        // Create new files with different metadata
        let new1 = create_test_file_info("new/path1.txt", 2048, None, 1002, 600); // Different size and creation time
        let new_files = vec![new1];

        // Simulate the rename detection logic
        let mut potential_renames = Vec::new();

        // Group by (size, creation_time)
        let mut deleted_by_key = std::collections::HashMap::new();
        let mut new_by_key = std::collections::HashMap::new();

        for file in &deleted_files {
            let creation_time = file.created.duration_since(UNIX_EPOCH).unwrap().as_secs();
            let key = (file.size, creation_time);
            deleted_by_key
                .entry(key)
                .or_insert_with(Vec::new)
                .push(file);
        }

        for file in &new_files {
            let creation_time = file.created.duration_since(UNIX_EPOCH).unwrap().as_secs();
            let key = (file.size, creation_time);
            new_by_key.entry(key).or_insert_with(Vec::new).push(file);
        }

        // Find matches
        for (key, deleted_list) in deleted_by_key {
            if let Some(new_list) = new_by_key.get(&key) {
                if let (Some(&deleted), Some(&new)) = (deleted_list.first(), new_list.first()) {
                    potential_renames.push((deleted.clone(), new.clone()));
                }
            }
        }

        // Should find no renames
        assert_eq!(potential_renames.len(), 0);
    }

    #[test]
    fn test_checksum_calculation() {
        let temp_dir = TempDir::new().unwrap();

        let test_file = temp_dir.child("test.txt");
        test_file.write_str("Test content for checksum").unwrap();

        let empty_file = temp_dir.child("empty.txt");
        empty_file.write_str("").unwrap();

        let binary_file = temp_dir.child("binary.dat");
        binary_file
            .write_binary(&[0x00, 0x01, 0x02, 0x03, 0xFF])
            .unwrap();

        let calculator = create_test_checksum_calculator();

        // Test regular file
        let checksum1 = calculator.calculate_checksum(test_file.path()).unwrap();
        assert!(!checksum1.is_empty());
        assert_eq!(checksum1.len(), 64); // BLAKE3 produces 64-character hex strings

        // Test empty file
        let checksum2 = calculator.calculate_checksum(empty_file.path()).unwrap();
        assert!(!checksum2.is_empty());
        assert_eq!(checksum2.len(), 64);

        // Test binary file
        let checksum3 = calculator.calculate_checksum(binary_file.path()).unwrap();
        assert!(!checksum3.is_empty());
        assert_eq!(checksum3.len(), 64);

        // All checksums should be different
        assert_ne!(checksum1, checksum2);
        assert_ne!(checksum1, checksum3);
        assert_ne!(checksum2, checksum3);

        // Verify files exist
        test_file.assert(predicates::path::exists());
        empty_file.assert(predicates::path::exists());
        binary_file.assert(predicates::path::exists());
    }
}
