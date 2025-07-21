use crate::{DdriveError, Result};
use blake3::Hasher;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;
use tracing::debug;

/// Default buffer size for checksum calculation (8KB)
const DEFAULT_BUFFER_SIZE: usize = 8192;

/// Calculator for BLAKE3 checksums with configurable buffer size
pub struct ChecksumCalculator {
    buffer_size: usize,
}

impl Default for ChecksumCalculator {
    fn default() -> Self {
        ChecksumCalculator {
            buffer_size: DEFAULT_BUFFER_SIZE,
        }
    }
}

impl ChecksumCalculator {
    /// Create a new checksum calculator with default 8KB buffer
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new checksum calculator with custom buffer size
    pub fn with_buffer_size(buffer_size: usize) -> Self {
        ChecksumCalculator { buffer_size }
    }

    /// Calculate BLAKE3 checksum for a file
    pub fn calculate_checksum<P: AsRef<Path>>(&self, file_path: P) -> Result<String> {
        let file_path = file_path.as_ref();

        let file = File::open(file_path).map_err(|e| DdriveError::Checksum {
            message: format!("Could not open file {}: {}", file_path.display(), e),
        })?;

        let mut reader = BufReader::new(file);
        let mut hasher = Hasher::new();
        let mut buffer = vec![0; self.buffer_size];

        loop {
            let bytes_read = reader
                .read(&mut buffer)
                .map_err(|e| DdriveError::Checksum {
                    message: format!("Could not read file {}: {}", file_path.display(), e),
                })?;

            if bytes_read == 0 {
                break;
            }

            hasher.update(&buffer[..bytes_read]);
        }

        let hash = hasher.finalize();
        let checksum = hash.to_hex().to_string();
        debug!("Calculated checksum: {}", &checksum[..16]);
        Ok(checksum)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_calculate_checksum_success() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.txt");
        fs::write(&file_path, "Hello, World!").unwrap();

        let calculator = ChecksumCalculator::new();
        let checksum = calculator.calculate_checksum(&file_path).unwrap();

        // BLAKE3 hash of "Hello, World!"
        assert_eq!(
            checksum,
            "288a86a79f20a3d6dccdca7713beaed178798296bdfa7913fa2a62d9727bf8f8"
        );
    }

    #[test]
    fn test_calculate_checksum_empty_file() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("empty.txt");
        fs::write(&file_path, "").unwrap();

        let calculator = ChecksumCalculator::new();
        let checksum = calculator.calculate_checksum(&file_path).unwrap();

        // BLAKE3 hash of empty string
        assert_eq!(
            checksum,
            "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
        );
    }

    #[test]
    fn test_calculate_checksum_nonexistent_file() {
        let calculator = ChecksumCalculator::new();
        let result = calculator.calculate_checksum("nonexistent_file.txt");

        assert!(result.is_err());
        match result.unwrap_err() {
            DdriveError::Checksum { message } => {
                assert!(message.contains("Could not open file"));
            }
            _ => panic!("Expected Checksum error"),
        }
    }

    #[test]
    fn test_calculate_checksum_different_content() {
        let temp_dir = TempDir::new().unwrap();
        let file1_path = temp_dir.path().join("file1.txt");
        let file2_path = temp_dir.path().join("file2.txt");

        fs::write(&file1_path, "Content A").unwrap();
        fs::write(&file2_path, "Content B").unwrap();

        let calculator = ChecksumCalculator::new();
        let checksum1 = calculator.calculate_checksum(&file1_path).unwrap();
        let checksum2 = calculator.calculate_checksum(&file2_path).unwrap();

        assert_ne!(checksum1, checksum2);
    }

    #[test]
    fn test_calculate_checksum_same_content() {
        let temp_dir = TempDir::new().unwrap();
        let file1_path = temp_dir.path().join("file1.txt");
        let file2_path = temp_dir.path().join("file2.txt");

        let content = "Same content in both files";
        fs::write(&file1_path, content).unwrap();
        fs::write(&file2_path, content).unwrap();

        let calculator = ChecksumCalculator::new();
        let checksum1 = calculator.calculate_checksum(&file1_path).unwrap();
        let checksum2 = calculator.calculate_checksum(&file2_path).unwrap();

        assert_eq!(checksum1, checksum2);
    }
}
