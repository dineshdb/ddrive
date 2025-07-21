use std::path::PathBuf;

use crate::{
    AppContext, Result, add::AddCommand, database::ActionType, duplicates::DuplicatesCommand,
    log::HistoryCommand, prune::PruneCommand, repository::RepositoryFinder, status::StatusCommand,
    verify::VerifyCommand,
};
use clap::{Parser, Subcommand};
use glob::Pattern;
use tracing::{debug, info};

#[derive(Parser)]
#[command(name = "ddrive")]
#[command(about = "A backup health monitoring application that tracks file integrity over time")]
#[command(version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Initialize a new ddrive repository
    Init,
    /// Add files for tracking (and update existing files)
    Add {
        /// Path to track (file or directory). Only files within this path will be considered for deletion.
        path: PathBuf,
    },
    /// Verify integrity of tracked files
    Verify {
        /// Optional path prefix to verify only files under this path
        #[arg(long)]
        path: Option<Pattern>,

        /// Force verification of all files regardless of last check time
        #[arg(short, long)]
        force: bool,
    },
    /// Find duplicate files based on BLAKE3 checksums
    Dedup,
    /// Show repository status and statistics
    Status,
    /// Prune deleted files and handle duplicates
    Prune,
    /// View and manage command history
    Log {
        #[command(subcommand)]
        action: Option<HistoryAction>,
    },
}

#[derive(Subcommand)]
pub enum HistoryAction {
    /// List command history
    List {
        /// Maximum number of entries to show
        #[arg(short, long, default_value = "20")]
        limit: usize,
        /// Filter by action type (add, delete)
        #[arg(short, long)]
        filter: Option<ActionType>,
    },
    /// Show details of a specific history entry
    Show {
        /// History entry action ID to show
        id: String,
    },
}

pub fn parse_args() -> Cli {
    Cli::parse()
}

pub async fn run_command(cli: Cli) -> Result<()> {
    let repo_finder = RepositoryFinder::new();

    match cli.command {
        Some(Commands::Init) => {
            repo_finder.init_repository().await?;
            Ok(())
        }
        Some(Commands::Add { path }) => {
            if !path.exists() {
                return Err(crate::DdriveError::FileSystem {
                    message: format!("Path does not exist: {}", path.display()),
                });
            }

            // Check if path is readable
            if let Err(e) = std::fs::metadata(&path) {
                return Err(crate::DdriveError::PermissionDenied {
                    message: format!("Cannot access path '{}': {e}", path.display()),
                });
            }

            let repo_root = repo_finder.ensure_repository_exists()?;
            let context = AppContext::new(repo_root.clone()).await?;
            let add_command = AddCommand::new(&context);

            debug!("Tracking files in: {}", path.display());
            let result = add_command.execute(&path).await?;

            if result.new_files > 0 || result.changed_files > 0 || result.deleted_files > 0 {
                info!(
                    "Added {} new, {} changed, {} deleted files",
                    result.new_files, result.changed_files, result.deleted_files
                );
            } else {
                info!("No changes detected - all files are up to date");
            }
            Ok(())
        }
        Some(Commands::Verify { path, force }) => {
            let repo_root = repo_finder.ensure_repository_exists()?;
            let context = AppContext::new(repo_root).await?;
            let verify_command = VerifyCommand::new(&context);

            let result = verify_command.execute(path.as_ref(), force).await?;

            if result.failed_files > 0 {
                return Err(crate::DdriveError::Validation {
                    message: format!(
                        "{} file(s) failed integrity verification. Run 'ddrive status' for details.",
                        result.failed_files
                    ),
                });
            }
            Ok(())
        }
        Some(Commands::Dedup) => {
            let repo_root = repo_finder.ensure_repository_exists()?;
            let context = AppContext::new(repo_root).await?;
            let duplicates_command = DuplicatesCommand::new(&context);
            duplicates_command.execute().await?;
            Ok(())
        }
        Some(Commands::Status) => {
            let repo_root = repo_finder.ensure_repository_exists()?;
            let context = AppContext::new(repo_root.clone()).await?;
            let status_command = StatusCommand::new(&context);
            status_command.execute().await?;
            Ok(())
        }

        Some(Commands::Prune) => {
            let repo_root = repo_finder.ensure_repository_exists()?;
            let context = AppContext::new(repo_root).await?;
            let prune_command = PruneCommand::new(&context);
            prune_command.execute().await?;
            Ok(())
        }
        Some(Commands::Log { action }) => {
            let repo_root = repo_finder.ensure_repository_exists()?;
            let context = AppContext::new(repo_root).await?;
            let history_command = HistoryCommand::new(&context);
            let Some(action) = action else {
                history_command.list(None, None).await?;
                return Ok(());
            };

            match action {
                HistoryAction::List { limit, filter } => {
                    history_command.list(Some(limit), filter).await?;
                    Ok(())
                }
                HistoryAction::Show { id } => {
                    history_command.show(&id).await?;
                    Ok(())
                }
            }
        }
        None => {
            info!("Showing ddrive status (default command)...");
            let repo_root = repo_finder.ensure_repository_exists()?;
            let context = AppContext::new(repo_root).await?;
            let status_command = StatusCommand::new(&context);
            status_command.execute().await?;
            Ok(())
        }
    }
}
