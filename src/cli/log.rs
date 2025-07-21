use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use tracing::info;

use crate::{
    AppContext, Result,
    database::{ActionType, HistoryRecord},
};

/// A grouped history entry representing an action that may affect multiple files
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub action_id: String,
    pub action_type: ActionType,
    pub timestamp: DateTime<Utc>,
    pub description: String,
    pub files_affected: Vec<String>,
    pub metadata: Option<JsonValue>,
}

/// Simplified history manager for tracking actions
pub struct HistoryManager<'a> {
    context: &'a AppContext,
}

impl<'a> HistoryManager<'a> {
    /// Create a new history manager
    pub fn new(context: &'a AppContext) -> Self {
        Self { context }
    }

    /// List history entries, optionally filtered by action type
    pub async fn list_history(
        &self,
        limit: Option<usize>,
        action_filter: Option<ActionType>,
    ) -> Result<Vec<HistoryRecord>> {
        let history_records = self
            .context
            .database
            .get_history_entries(limit, action_filter)
            .await?;

        Ok(history_records)
    }

    /// Get a specific history entry by action ID (base58 string)
    pub async fn get_history_entry(&self, action_id_base58: &str) -> Result<Vec<HistoryRecord>> {
        self.context
            .database
            .get_history_entries_by_action_id_base58(action_id_base58)
            .await
    }
}

pub struct HistoryCommand<'a> {
    history_manager: HistoryManager<'a>,
}

impl<'a> HistoryCommand<'a> {
    pub fn new(context: &'a AppContext) -> Self {
        let history_manager = HistoryManager::new(context);
        Self { history_manager }
    }

    /// List history entries
    pub async fn list(
        &self,
        limit: Option<usize>,
        action_filter: Option<ActionType>,
    ) -> Result<()> {
        let entries = self
            .history_manager
            .list_history(limit, action_filter)
            .await?;

        if entries.is_empty() {
            info!("No history entries found");
            return Ok(());
        }

        let entries =
            entries
                .iter()
                .fold(HashMap::new(), |h: HashMap<i64, Vec<&HistoryRecord>>, e| {
                    let mut h = h;
                    h.entry(e.action_id)
                        .and_modify(|l: &mut Vec<&HistoryRecord>| l.push(e))
                        .or_insert(vec![e]);
                    h
                });

        for (action_id, entries) in entries {
            info!(
                "{} {}",
                DateTime::from_timestamp(action_id, 0).unwrap_or_else(Utc::now),
                bs58::encode(action_id.to_be_bytes()).into_string(),
            );
            for entry in entries.iter().take(5) {
                info!("  {} {}", entry.action_type, entry.path,)
            }
            if entries.len() > 5 {
                info!("  and {} more...", entries.len() - 5);
            }
        }

        Ok(())
    }

    /// Show details of a specific history entry
    pub async fn show(&self, action_id: &str) -> Result<()> {
        let entries = self.history_manager.get_history_entry(action_id).await?;
        if entries.is_empty() {
            info!("No such entry");
        }
        let mut entries = entries.iter();
        let entry = entries.next().expect("entry");
        info!("{} {}", entry.action_timestamp(), entry.action_id_base58(),);
        for entry in entries {
            info!("  {} {}", entry.action_type, entry.path,)
        }

        Ok(())
    }
}
