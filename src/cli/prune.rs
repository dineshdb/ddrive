use crate::{AppContext, Result, cli::dedup::DedupCommand, database::ActionType};
use tracing::info;

pub struct PruneCommand<'a> {
    context: &'a AppContext,
}

#[derive(Debug, Default)]
pub struct PruneResult {
    pub duplicates_processed: usize,
    pub pruned_backups: usize,
    pub orphaned_objects_deleted: usize,
}

impl<'a> PruneCommand<'a> {
    pub fn new(context: &'a AppContext) -> Self {
        Self { context }
    }

    pub async fn execute(&self) -> Result<PruneResult> {
        info!("Starting prune operation...");

        // Clean up old history entries first
        let old_deleted_history_entry = self
            .context
            .database
            .cleanup_old_history(
                ActionType::Delete,
                self.context.config.prune.cutoff_date().timestamp(),
            )
            .await?;
        info!("Pruned {old_deleted_history_entry} old history entries for deleted files",);

        // Clean up orphaned objects from object store
        let orphaned_objects_deleted = self.context.database.cleanup_orphaned_objects().await?;
        info!(
            "Deleted {} orphaned objects from object store",
            orphaned_objects_deleted
        );

        // Handle duplicates
        let dedup_command = DedupCommand::new(self.context);
        let duplicate_groups = dedup_command.execute().await?;
        if !duplicate_groups.is_empty() {
            info!(
                "Found {} duplicate groups. Use 'ddrive dedup' command to handle them.",
                duplicate_groups.len()
            );
        }

        let result = PruneResult {
            pruned_backups: old_deleted_history_entry,
            duplicates_processed: duplicate_groups.len(),
            orphaned_objects_deleted,
        };

        info!("Prune operation completed successfully");
        Ok(result)
    }
}
