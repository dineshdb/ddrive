use std::path::StripPrefixError;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum DdriveError {
    #[error("Invalid directory")]
    InvalidDirectory,

    #[error("error getting relative path: {0}")]
    InvalidPath(#[from] StripPrefixError),

    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("File system error: {message}")]
    FileSystem { message: String },

    #[error("Hard link error: {message}")]
    HardLink { message: String },

    #[error("Checksum calculation error: {message}")]
    Checksum { message: String },

    #[error("Repository error: {message}")]
    Repository { message: String },

    #[error("Validation error: {message}")]
    Validation { message: String },

    #[error("Database migration error: {0}")]
    SqlxMigration(#[from] sqlx::migrate::MigrateError),

    #[error("Ignore pattern error: {message}")]
    IgnorePattern { message: String },

    #[error("Glob pattern error: {0}")]
    GlobPattern(#[from] glob::PatternError),

    #[error("Permission denied: {message}")]
    PermissionDenied { message: String },

    #[error("Configuration error: {message}")]
    Configuration { message: String },

    #[error("User cancelled operation")]
    UserCancelled,
}

impl DdriveError {
    pub fn exit_code(&self) -> i32 {
        match self {
            DdriveError::Repository { .. } => 2,
            DdriveError::Database(_) | DdriveError::SqlxMigration(_) => 3,
            DdriveError::FileSystem { .. }
            | DdriveError::InvalidDirectory
            | DdriveError::InvalidPath(_) => 4,
            DdriveError::HardLink { .. } => 4,
            DdriveError::Checksum { .. } => 5,
            DdriveError::Validation { .. } => 6,
            DdriveError::IgnorePattern { .. } | DdriveError::GlobPattern(_) => 7,
            DdriveError::Io(_) => 8,
            DdriveError::PermissionDenied { .. } => 9,
            DdriveError::Configuration { .. } => 10,
            DdriveError::UserCancelled => 11,
        }
    }
}

pub type Result<T> = std::result::Result<T, DdriveError>;
