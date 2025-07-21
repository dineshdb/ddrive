use crate::{
    AppContext, Result, config::Config, database::ActionType, duplicates::DuplicatesCommand,
};
use tracing::info;

pub struct PruneCommand<'a> {
    context: &'a AppContext,
}

#[derive(Debug, Default)]
pub struct PruneResult {
    pub duplicates_processed: usize,
    pub duplicates_deleted: usize,
    pub duplicates_hardlinked: usize,
    pub pruned_backups: usize,
    pub new_deletions_tracked: usize,
}

impl<'a> PruneCommand<'a> {
    pub fn new(context: &'a AppContext) -> Self {
        Self { context }
    }

    pub async fn execute(&self) -> Result<()> {
        let config = Config::load(&self.context.repo_root)?;
        let old_deleted_history_entry = self
            .context
            .database
            .cleanup_old_history(ActionType::Delete, config.prune.cutoff_date().timestamp())
            .await?;
        info!("Pruned {old_deleted_history_entry} old history entries for deleted files.");

        let duplicates_command = DuplicatesCommand::new(self.context);
        let duplicate_groups = duplicates_command.execute().await?;
        if !duplicate_groups.is_empty() {
            info!(
                "Found {} duplicate groups. Use 'ddrive dedup' command to handle them.",
                duplicate_groups.len()
            );
        }
        Ok(())
    }
}
