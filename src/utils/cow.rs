use std::fs;
use tracing::debug;
use tracing::warn;

use crate::DdriveError;
use crate::Result;

pub fn cow_copy(src: &str, dest: &str) -> Result<()> {
    match copy_on_write::reflink_file_sync(src, dest) {
        Ok(_) => {
            debug!("Created hard link for {src} -> {dest}",);
            return Ok(());
        }
        Err(e) => {
            warn!("Hard link failed, falling back to copy: {e}");
            // Fall through to copy
        }
    }

    // Copy the file if CoW failed or is disabled
    fs::copy(src, dest).map_err(|e| DdriveError::FileSystem {
        message: format!("Failed to copy file to object store: {e}"),
    })?;

    debug!("Copied {src} -> {dest}",);
    Ok(())
}
