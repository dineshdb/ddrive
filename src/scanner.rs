use crate::database::FileRecord;
use crate::{DdriveError, Result, ignore::IgnorePatterns};
use rayon::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};
use tracing::{debug, warn};
use walkdir::WalkDir;

pub struct FileScanner<'a> {
    ignore_patterns: &'a IgnorePatterns,
    repo_root: PathBuf,
}

impl<'a> FileScanner<'a> {
    pub fn new(ignore_patterns: &'a IgnorePatterns, repo_root: PathBuf) -> Self {
        FileScanner {
            ignore_patterns,
            repo_root,
        }
    }

    /// Recursively scan directory structure and return file information
    pub fn scan_directory<P: AsRef<Path>>(&self, path: P) -> Result<Vec<FileInfo>> {
        let file_paths = self.get_all_files(path.as_ref())?;
        self.fetch_metadata(file_paths.as_slice())
    }

    /// Recursively scan directory structure and return file information
    pub fn fetch_metadata(&self, file_paths: &[PathBuf]) -> Result<Vec<FileInfo>> {
        let start_time = Instant::now();

        // Process metadata in parallel with progress tracking
        let files: Vec<FileInfo> = file_paths
            .par_iter()
            .map(|path| {
                match self.get_file_metadata(path) {
                    Ok(file_info) => Some(file_info),
                    Err(e) => {
                        // Log as debug for permission errors, warn for other issues
                        if e.to_string().contains("Permission denied") {
                            debug!("Permission denied for {}: {}", path.display(), e);
                        } else {
                            warn!("Could not read metadata for {}: {}", path.display(), e);
                        }
                        None
                    }
                }
            })
            .filter_map(|x| x)
            .collect();

        debug!(
            "Fetched metadata for {} files in {:.2}ms",
            files.len(),
            start_time.elapsed().as_millis()
        );
        Ok(files)
    }

    /// Recursively scan directory structure and return paths
    pub fn get_all_files<P: AsRef<Path>>(&self, path: P) -> Result<Vec<PathBuf>> {
        let instant = Instant::now();
        let path = path.as_ref();

        // Collect all file paths first (fast sequential scan)
        let file_paths: Vec<PathBuf> = WalkDir::new(path)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| !self.should_skip_entry(e.path()))
            .filter_map(|entry| match entry {
                Ok(entry) => {
                    let entry_path = entry.path();
                    let relative_path = entry_path
                        .strip_prefix(&self.repo_root)
                        .unwrap_or(entry_path);
                    if entry_path.is_file() && !self.ignore_patterns.should_ignore(relative_path) {
                        Some(relative_path.to_path_buf())
                    } else {
                        None
                    }
                }
                Err(e) => {
                    warn!("Error accessing path: {}", e);
                    None
                }
            })
            .collect();

        debug!(
            "Found {} files in {}ms",
            file_paths.len(),
            instant.elapsed().as_millis()
        );

        Ok(file_paths)
    }

    fn should_skip_entry(&self, path: &Path) -> bool {
        let path = path.strip_prefix(&self.repo_root).unwrap_or(path);
        self.ignore_patterns.should_ignore(path)
    }

    /// Extract file metadata for a single file
    pub fn get_file_metadata<P: AsRef<Path>>(&self, path: P) -> Result<FileInfo> {
        let path = path.as_ref();

        let metadata = fs::metadata(path).map_err(|e| DdriveError::FileSystem {
            message: format!("Could not read metadata for {}: {}", path.display(), e),
        })?;

        if !metadata.is_file() {
            return Err(DdriveError::FileSystem {
                message: format!("Path is not a regular file: {}", path.display()),
            });
        }

        Ok(FileInfo {
            path: path.to_path_buf(),
            size: metadata.len(),
            modified: metadata.modified().map_err(|e| DdriveError::FileSystem {
                message: format!(
                    "Could not read modification time for {}: {}",
                    path.display(),
                    e
                ),
            })?,
        })
    }
}

#[derive(Debug, Clone)]
pub struct FileInfo {
    pub path: PathBuf,
    pub size: u64,
    pub modified: std::time::SystemTime,
}

#[derive(Debug, Clone)]
pub struct CheckSumFileInfo {
    pub path: PathBuf,
    pub size: u64,
    pub modified: std::time::SystemTime,
    pub b3sum: String,
}

impl From<&FileRecord> for FileInfo {
    fn from(value: &FileRecord) -> Self {
        Self {
            path: value.path.clone().into(),
            size: value.size as u64,
            modified: UNIX_EPOCH
                + Duration::from_secs(value.updated_at.and_utc().timestamp() as u64),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ignore::IgnorePatterns;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_scan_directory_nonexistent() {
        let ignore_patterns = IgnorePatterns::new();
        let scanner = FileScanner::new(&ignore_patterns, PathBuf::from("nonexistent_directory"));
        let result = scanner.scan_directory("nonexistent_directory");
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_file_metadata_success() {
        let temp_dir = TempDir::new().unwrap();
        let ignore_patterns = IgnorePatterns::new();
        let file_path = temp_dir.path().join("test.txt");
        let content = "test content";
        fs::write(&file_path, content).unwrap();

        let scanner = FileScanner::new(&ignore_patterns, temp_dir.path().to_path_buf());
        let file_info = scanner.get_file_metadata(&file_path).unwrap();

        assert_eq!(file_info.path, file_path);
        assert_eq!(file_info.size, content.len() as u64);
        assert!(file_info.modified.elapsed().unwrap().as_secs() < 5);
    }

    #[test]
    fn test_get_file_metadata_nonexistent() {
        let ignore_patterns = IgnorePatterns::new();
        let scanner = FileScanner::new(&ignore_patterns, PathBuf::from("nonexistent_directory"));
        let result = scanner.get_file_metadata("nonexistent_file.txt");

        assert!(result.is_err());
        match result.unwrap_err() {
            DdriveError::FileSystem { message } => {
                assert!(message.contains("Could not read metadata"));
            }
            _ => panic!("Expected FileSystem error"),
        }
    }

    #[test]
    fn test_get_file_metadata_directory() {
        let temp_dir = TempDir::new().unwrap();
        let ignore_patterns = IgnorePatterns::new();

        let scanner = FileScanner::new(&ignore_patterns, temp_dir.path().to_path_buf());
        let result = scanner.get_file_metadata(temp_dir.path());

        assert!(result.is_err());
        match result.unwrap_err() {
            DdriveError::FileSystem { message } => {
                assert!(message.contains("Path is not a regular file"));
            }
            _ => panic!("Expected FileSystem error"),
        }
    }
}
