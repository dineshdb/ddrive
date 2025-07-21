# ddrive

> A full-on vibe-coding experience with Kiro. I liked it first, it got me
> started faster. But later, it no longer did what I wanted and wrote 10 things
> that I didn't like. Remove around 40 percent of the code written by it and
> hence reducing the features I had originally planned for it. I will grow it
> later. For now, it still contains more than half, maybe even 70% code written
> by AI. Rest is me. I will still use AI in the future but I'm not sure if going
> full on, vibe-coding is quite there yet.

A backup health monitoring application that tracks file integrity over time
using cryptographic checksums. Tries to be cli-compatible with `git`.

## Key Features

- Uses BLAKE3 hashing for fast, secure file verification
- SQLite database for metadata storage (`.ddrive/metadata.sqlite3`)
- Object store with Copy-on-Write(CoW) for efficient storage
- Configurable verification intervals and retention policies

## Configuration

Configuration is stored in `.ddrive/config.toml` and includes:

```toml
[general]
verbose = false

[verify]
interval_days = 30

[prune]
retention_days = 90
```

## Usage

```bash
# Initialize a repository
ddrive init

# Add files for tracking (only considers files within the specified path for deletion)
ddrive add <path>

# Verify file integrity
ddrive verify [--path <pattern>] [--force]

# Show repository status
ddrive status

# Prune old deleted files
ddrive prune [--dry-run] [--force]

# Manage configuration
ddrive config show
ddrive config set verify.interval_days 60
```

## Object Store

Files are stored in the object store using their BLAKE3 checksums as
identifiers, with a two-level directory structure:

```
.ddrive/objects/
  ├── aa/
  │   └── bb/
  │       └── aabb1234...
  └── cc/
      └── dd/
          └── ccdd5678...
```

CoW is used when possible to save disk space.

## Deletion Tracking

When files are deleted:

1. Deletions are tracked in the history table with the file's path and checksum
2. After the retention period (default: 90 days), they are pruned from the
   database
3. Object store files are retained as long as they are referenced by at least
   one file record or history entry

## License

[MIT](LICENSE)
