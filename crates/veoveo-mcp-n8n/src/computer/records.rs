//! What a person's own Computer applied of one team, kept in a file of its own beside n8n's
//! state, as the server keeps its in its table.
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::{
    contract::{FileDigest, WorkflowName},
    n8n::N8nWorkflowId,
    store::AppliedWorkflow,
    workflows::AppliedRecords,
};

/// One workflow the Computer applied.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Record {
    workflow: WorkflowName,
    n8n_id: N8nWorkflowId,
    digest: FileDigest,
    version: uuid::Uuid,
    runnable: bool,
}

/// The records of one team, in `{root}/{team}.json`.
pub struct TeamRecords {
    path: PathBuf,
}

impl TeamRecords {
    pub fn new(root: &Path, team: &str) -> Self {
        Self {
            path: root.join(format!("{team}.json")),
        }
    }

    /// Whether the Computer applied the team at all.
    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    fn read(&self) -> Result<Vec<Record>> {
        match std::fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .with_context(|| format!("reading {}", self.path.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(error) => Err(error).with_context(|| format!("reading {}", self.path.display())),
        }
    }

    fn write(&self, records: &[Record]) -> Result<()> {
        let parent = self.path.parent().context("the records have a folder")?;
        std::fs::create_dir_all(parent)?;
        let staged = self.path.with_extension("json.new");
        std::fs::write(&staged, serde_json::to_vec_pretty(records)?)?;
        std::fs::rename(&staged, &self.path)?;
        Ok(())
    }
}

impl AppliedRecords for TeamRecords {
    async fn recorded(&self) -> Result<Vec<AppliedWorkflow>> {
        Ok(self
            .read()?
            .into_iter()
            .map(|record| AppliedWorkflow {
                workflow: record.workflow,
                n8n_id: record.n8n_id,
                digest: record.digest,
                version: record.version,
                runnable: record.runnable,
                applied: serde_json::Value::Null,
            })
            .collect())
    }

    async fn record(&self, applied: &AppliedWorkflow) -> Result<()> {
        let mut records = self.read()?;
        records.retain(|record| record.workflow != applied.workflow);
        records.push(Record {
            workflow: applied.workflow.clone(),
            n8n_id: applied.n8n_id.clone(),
            digest: applied.digest,
            version: applied.version,
            runnable: applied.runnable,
        });
        records.sort_by(|a, b| a.workflow.cmp(&b.workflow));
        self.write(&records)
    }

    async fn forget(&self, workflow: &WorkflowName) -> Result<()> {
        let mut records = self.read()?;
        records.retain(|record| &record.workflow != workflow);
        self.write(&records)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn records_round_trip_through_their_file() {
        let directory = tempfile::tempdir().unwrap();
        let records = TeamRecords::new(directory.path(), "marketing");
        assert!(!records.exists());
        assert!(records.recorded().await.unwrap().is_empty());
        let applied = AppliedWorkflow {
            workflow: WorkflowName::parse("hello-run").unwrap(),
            n8n_id: N8nWorkflowId::parse("Au7bKCOK5h1uXPLz").unwrap(),
            digest: FileDigest::from_bytes([3; 32]),
            version: uuid::Uuid::from_u128(7),
            runnable: true,
            applied: serde_json::Value::Null,
        };
        records.record(&applied).await.unwrap();
        records.record(&applied).await.unwrap();
        assert_eq!(
            records.recorded().await.unwrap(),
            std::slice::from_ref(&applied)
        );
        records.forget(&applied.workflow).await.unwrap();
        assert!(records.exists());
        assert!(records.recorded().await.unwrap().is_empty());
    }
}
