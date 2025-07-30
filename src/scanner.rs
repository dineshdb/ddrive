use crate::Result;
use chrono::NaiveDateTime;
use ignore::WalkBuilder;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};
use tracing::{debug, warn};

pub struct FileScanner {
    repo_root: PathBuf,
}

impl FileScanner {
    pub fn new(repo_root: PathBuf) -> Self {
        FileScanner { repo_root }
    }

    /// Recursively scan directory structure and return paths
    pub fn get_all_files(&self, path: &PathBuf) -> Result<Vec<FileInfo>> {
        let instant = Instant::now();
        let file_paths: Vec<_> = get_all_files(&self.repo_root, path, false, true)?;

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
    pub modified: SystemTime,
    pub created: SystemTime,
    pub b3sum: Option<String>,
}

impl FileInfo {
    pub fn created_at(&self) -> Option<NaiveDateTime> {
        self.created
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .and_then(|duration| {
                chrono::DateTime::from_timestamp(duration.as_secs() as i64, 0)
                    .map(|dt| dt.naive_utc())
            })
    }

    pub fn modified_at(&self) -> Option<NaiveDateTime> {
        self.modified
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .and_then(|duration| {
                chrono::DateTime::from_timestamp(duration.as_secs() as i64, 0)
                    .map(|dt| dt.naive_utc())
            })
    }
}

pub fn get_all_files<P: AsRef<Path>>(
    repo_root: P,
    path: P,
    hidden: bool,
    ignore: bool,
) -> Result<Vec<FileInfo>> {
    let instant = Instant::now();
    let path = path.as_ref();

    let file_paths: Vec<_> = WalkBuilder::new(path)
        .follow_links(false)
        .hidden(hidden)
        .ignore(ignore)
        .build()
        .filter_map(|entry| match entry {
            Ok(entry) => {
                let path = entry
                    .path()
                    .strip_prefix(&repo_root)
                    .unwrap_or(entry.path());
                let metadata = std::fs::metadata(path).ok()?;
                let modified = metadata.modified().ok()?;
                let created = metadata.created().ok()?; // Birth time/creation time
                if metadata.is_file() {
                    Some(FileInfo {
                        path: path.to_path_buf(),
                        size: metadata.len(),
                        modified,
                        created,
                        b3sum: None,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scan_directory_nonexistent() {
        let scanner = FileScanner::new(PathBuf::from("nonexistent_directory"));
        let result = scanner.get_all_files(&PathBuf::from("nonexistent_directory"));
        assert!(result.is_ok());
    }
}
