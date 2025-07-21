-- Files table - tracks all active files with their metadata and checksums
CREATE TABLE IF NOT EXISTS files (
    id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
    path TEXT NOT NULL UNIQUE,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    last_checked DATETIME NULL,
    b3sum TEXT NOT NULL,
    size INTEGER NOT NULL
);

-- History table - tracks all actions with multiple entries per action_id
-- Deleted files are preserved here for trash-like functionality
CREATE TABLE IF NOT EXISTS history (
    id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
    action_id INTEGER NOT NULL, -- Unix epoch timestamp (multiple entries can share same action_id)
    action_type INTEGER NOT NULL, -- 1=track, 2=delete (integer for performance)
    path TEXT NOT NULL, -- Path of affected file (relative to repo root)
    b3sum TEXT NOT NULL, -- BLAKE3 checksum of the file at time of action
    size INTEGER NOT NULL, -- File size at time of action (for deleted files)
    metadata TEXT NULL -- JSON metadata for action-specific data
);

-- Indexes for files table
CREATE INDEX IF NOT EXISTS idx_files_path ON files(path);
CREATE INDEX IF NOT EXISTS idx_files_last_checked ON files(last_checked);
CREATE INDEX IF NOT EXISTS idx_files_updated_at ON files(updated_at);
CREATE INDEX IF NOT EXISTS idx_files_b3sum ON files(b3sum); -- For duplicate detection

-- Indexes for history table
CREATE INDEX IF NOT EXISTS idx_history_action_id ON history(action_id);
CREATE INDEX IF NOT EXISTS idx_history_action_type ON history(action_type);
CREATE INDEX IF NOT EXISTS idx_history_path ON history(path);
CREATE INDEX IF NOT EXISTS idx_history_b3sum ON history(b3sum);