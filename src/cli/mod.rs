mod add;
mod dedup;
mod log;
mod prune;
mod rm;
mod status;
mod verify;

use std::path::PathBuf;

use crate::{AppContext, Result, database::ActionType, repository::Repository};
use add::AddCommand;
use dedup::DedupCommand;
use log::HistoryCommand;
use prune::PruneCommand;
use rm::RmCommand;
use status::StatusCommand;
use verify::VerifyCommand;

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
    /// Remove files from tracking
    Rm {
        #[command(subcommand)]
        action: RmAction,
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
    Dedup {
        /// Optional path pattern to filter which files to consider for deduplication
        #[arg(short, long)]
        path: Option<String>,
    },
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

#[derive(Subcommand, Clone)]
pub enum RmAction {
    Tracked { pattern: Pattern },
    Deleted { pattern: Option<Pattern> },
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

pub async fn run_command(cli: Cli) -> Result<()> {
    let current_dir = std::env::current_dir()?;
    match cli.command {
        Some(Commands::Init) => {
            Repository::init_repository(current_dir).await?;
            Ok(())
        }
        Some(Commands::Add { path }) => {
            let repo = Repository::find_repository(current_dir)?;
            let context = AppContext::new(repo).await?;
            let add_command = AddCommand::new(&context);

            debug!("Tracking files in: {}", path.display());
            let result = add_command.execute(&path).await?;

            if result.new_files > 0 || result.changed_files > 0 {
                info!(
                    "Added {} new, {} changed",
                    result.new_files, result.changed_files,
                );
            } else {
                info!("No changes detected - all files are up to date");
            }
            Ok(())
        }
        Some(Commands::Rm { action }) => {
            let repo = Repository::find_repository(current_dir)?;
            let context = AppContext::new(repo).await?;
            let rm_command = RmCommand::new(&context);

            match action {
                RmAction::Tracked { pattern } => rm_command.tracked(pattern).await?,
                RmAction::Deleted { pattern } => rm_command.deleted(pattern).await?,
            };
            Ok(())
        }
        Some(Commands::Verify { path, force }) => {
            let repo = Repository::find_repository(current_dir)?;
            let context = AppContext::new(repo).await?;
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
        Some(Commands::Dedup { path }) => {
            let repo = Repository::find_repository(current_dir)?;
            let context = AppContext::new(repo).await?;

            let dedup_command = if let Some(path_filter) = path {
                DedupCommand::with_path_filter(&context, path_filter)
            } else {
                DedupCommand::new(&context)
            };

            dedup_command.execute().await?;
            Ok(())
        }
        Some(Commands::Status) => {
            let repo = Repository::find_repository(current_dir)?;
            let context = AppContext::new(repo).await?;
            let status_command = StatusCommand::new(&context);
            status_command.execute().await?;
            Ok(())
        }

        Some(Commands::Prune) => {
            let repo = Repository::find_repository(current_dir)?;
            let context = AppContext::new(repo).await?;
            let prune_command = PruneCommand::new(&context);
            let result = prune_command.execute().await?;
            info!(
                "Pruning complete: {} old entries removed, {} duplicate groups processed",
                result.pruned_backups, result.duplicates_processed
            );
            Ok(())
        }
        Some(Commands::Log { action }) => {
            let repo = Repository::find_repository(current_dir)?;
            let context = AppContext::new(repo).await?;
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
            let repo = Repository::find_repository(current_dir)?;
            let context = AppContext::new(repo).await?;
            let status_command = StatusCommand::new(&context);
            status_command.execute().await?;
            Ok(())
        }
    }
}
