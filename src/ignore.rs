use crate::{DdriveError, Result};
use glob::Pattern;
use std::fs;
use std::path::Path;

pub const DEFAULT_IGNORE_PATTERNS: &str = include_str!("./ignore");

#[derive(Debug, Clone, Default)]
pub struct IgnorePatterns {
    patterns: Vec<Pattern>,
}

impl IgnorePatterns {
    pub fn new() -> Self {
        let mut patterns = Vec::new();
        Self::load_default_patterns(&mut patterns);
        Self { patterns }
    }

    fn load_default_patterns(patterns: &mut Vec<Pattern>) {
        for line in DEFAULT_IGNORE_PATTERNS.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Ok(pattern) = Pattern::new(line) {
                patterns.push(pattern);
            }
        }
    }

    pub fn load<P: AsRef<Path>>(ignore_file: P) -> Result<Self> {
        let ignore_file = ignore_file.as_ref();
        let mut patterns = Vec::new();
        patterns.push(Pattern::new(".ddrive/*").unwrap());

        // Always load default patterns first
        Self::load_default_patterns(&mut patterns);

        // Then load user-specific patterns if they exist
        if ignore_file.exists() {
            let content = fs::read_to_string(ignore_file)?;
            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }

                let pattern = Pattern::new(line).map_err(|e| DdriveError::IgnorePattern {
                    message: format!("Invalid pattern '{line}': {e:?}"),
                })?;
                patterns.push(pattern);
            }
        }

        Ok(Self { patterns })
    }

    pub fn should_ignore<P: AsRef<Path>>(&self, path: P) -> bool {
        let path = path.as_ref();
        let path_str = path.to_string_lossy();

        for pattern in &self.patterns {
            if pattern.matches(&path_str) {
                return true;
            }
        }

        false
    }

    pub fn add_pattern(&mut self, pattern_str: &str) -> Result<()> {
        let pattern = Pattern::new(pattern_str).map_err(|e| DdriveError::IgnorePattern {
            message: format!("Invalid pattern '{pattern_str}': {e:?}"),
        })?;
        self.patterns.push(pattern);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ignore::IgnorePatterns;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_ignore_patterns() {
        let temp_dir = TempDir::new().unwrap();
        let ignore_file = temp_dir.path().join("ignore.txt");
        fs::write(&ignore_file, DEFAULT_IGNORE_PATTERNS).unwrap();

        let ignore_patterns = IgnorePatterns::load(&ignore_file).unwrap();

        assert!(ignore_patterns.should_ignore(".ddrive/file.tmp"));
        assert!(ignore_patterns.should_ignore(".ddrive/file.log"));
        assert!(ignore_patterns.should_ignore(".ddrive/file.txt"));
        assert!(ignore_patterns.should_ignore(".vscode/settings.json"));
        assert!(ignore_patterns.should_ignore("node_modules/package.json"));
        assert!(ignore_patterns.should_ignore("notes.log"));
        assert!(ignore_patterns.should_ignore("Documents/notes.log"));

        assert!(!ignore_patterns.should_ignore("Documents/notes.txt"));
        assert!(!ignore_patterns.should_ignore("test/ignore.rs"));
        assert!(!ignore_patterns.should_ignore("file.txt"));
    }
}
