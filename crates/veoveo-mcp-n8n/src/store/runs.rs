//! The runs the server dispatched to n8n, one for each `run` Task while it runs.
use sha2::{Digest, Sha256};
use veoveo_store::{PlatformStore, TenantId};

use super::StoreError;
use crate::contract::{ExecutionId, WorkflowName};

/// One run as the store holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub task_id: uuid::Uuid,
    pub workflow: WorkflowName,
    pub operation_id: uuid::Uuid,
    /// n8n's execution, once its trigger answered.
    pub execution: Option<ExecutionId>,
}

struct RunRow {
    task_id: uuid::Uuid,
    workflow: String,
    operation_id: uuid::Uuid,
    execution_id: Option<String>,
}

impl TryFrom<RunRow> for Run {
    type Error = StoreError;
    fn try_from(row: RunRow) -> Result<Self, Self::Error> {
        Ok(Self {
            task_id: row.task_id,
            workflow: WorkflowName::parse(row.workflow)
                .map_err(|_| StoreError::Corrupt("workflow name"))?,
            operation_id: row.operation_id,
            execution: row
                .execution_id
                .map(ExecutionId::parse)
                .transpose()
                .map_err(|_| StoreError::Corrupt("execution ID"))?,
        })
    }
}

/// The SHA-256 of a run token, the only form the store keeps.
pub fn token_digest(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

/// The server's runs in its own schema.
#[derive(Clone)]
pub struct RunStore {
    store: PlatformStore,
}

impl RunStore {
    pub fn new(store: PlatformStore) -> Self {
        Self { store }
    }

    /// Record that `task_id`'s run of `workflow` dispatches `operation_id` with `token`.
    pub async fn record(
        &self,
        tenant: TenantId,
        task_id: uuid::Uuid,
        workflow: &WorkflowName,
        operation_id: uuid::Uuid,
        token: &str,
    ) -> Result<(), StoreError> {
        let digest = token_digest(token);
        sqlx::query_file!(
            "src/store/runs/record_run.sql",
            tenant as TenantId,
            task_id,
            workflow.as_str(),
            operation_id,
            digest.as_slice(),
        )
        .execute(self.store.pool())
        .await?;
        Ok(())
    }

    /// Record the execution n8n started for `task_id`'s run.
    pub async fn record_execution(
        &self,
        tenant: TenantId,
        task_id: uuid::Uuid,
        execution: &ExecutionId,
    ) -> Result<(), StoreError> {
        sqlx::query_file!(
            "src/store/runs/record_execution.sql",
            tenant as TenantId,
            task_id,
            execution.as_str(),
        )
        .execute(self.store.pool())
        .await?;
        Ok(())
    }

    /// `task_id`'s run, when it has one.
    pub async fn run(
        &self,
        tenant: TenantId,
        task_id: uuid::Uuid,
    ) -> Result<Option<Run>, StoreError> {
        sqlx::query_file_as!(
            RunRow,
            "src/store/runs/run.sql",
            tenant as TenantId,
            task_id
        )
        .fetch_optional(self.store.pool())
        .await?
        .map(Run::try_from)
        .transpose()
    }

    /// The run whose token is `token`, when one has it.
    pub async fn by_token(&self, tenant: TenantId, token: &str) -> Result<Option<Run>, StoreError> {
        let digest = token_digest(token);
        sqlx::query_file_as!(
            RunRow,
            "src/store/runs/run_by_token.sql",
            tenant as TenantId,
            digest.as_slice(),
        )
        .fetch_optional(self.store.pool())
        .await?
        .map(Run::try_from)
        .transpose()
    }

    /// Forget `task_id`'s run once its Task ended.
    pub async fn forget(&self, tenant: TenantId, task_id: uuid::Uuid) -> Result<(), StoreError> {
        sqlx::query_file!("src/store/runs/forget_run.sql", tenant as TenantId, task_id)
            .execute(self.store.pool())
            .await?;
        Ok(())
    }
}
