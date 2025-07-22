use crate::Result;
use ignore::WalkBuilder;
use std::path::{Path, PathBuf};
use std::time::Instant;
use tracing::{debug, warn};

pub struct FileScanner {
    repo_root: PathBuf,
}

impl FileScanner {
    pub fn new(repo_root: PathBuf) -> Self {
        FileScanner { repo_root }
    }

    /// Recursively scan directory structure and return paths
    pub fn get_all_files<P: AsRef<Path>>(&self, path: P) -> Result<Vec<FileInfo>> {
        let instant = Instant::now();
        let path = path.as_ref();

        let file_paths: Vec<_> = WalkBuilder::new(path)
            .follow_links(false)
            .hidden(false)
            .build()
            .filter_map(|entry| match entry {
                Ok(entry) => {
                    let path = entry
                        .path()
                        .strip_prefix(&self.repo_root)
                        .unwrap_or(entry.path());
                    let metadata = std::fs::metadata(path).ok()?;
                    let modified = metadata.modified().ok()?;
                    if metadata.is_file() {
                        Some(FileInfo {
                            path: path.to_path_buf(),
                            size: metadata.len(),
                            modified,
                        })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scan_directory_nonexistent() {
        let scanner = FileScanner::new(PathBuf::from("nonexistent_directory"));
        let result = scanner.get_all_files("nonexistent_directory");
        assert!(result.is_ok());
    }
}
