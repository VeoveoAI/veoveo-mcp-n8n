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

/// The run's Task that dispatched an execution, and the Task's caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dispatched {
    pub task_id: uuid::Uuid,
    /// The caller's principal, as the Task's owner names it.
    pub caller: String,
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

    /// Record that `task_id`, whose caller is `caller`, dispatched `execution` of n8n
    /// `instance`: only that caller reads its data.
    pub async fn record_dispatched(
        &self,
        tenant: TenantId,
        instance: uuid::Uuid,
        execution: &ExecutionId,
        task_id: uuid::Uuid,
        caller: &str,
    ) -> Result<(), StoreError> {
        sqlx::query_file!(
            "src/store/runs/record_dispatched.sql",
            tenant as TenantId,
            instance,
            execution.as_str(),
            task_id,
            caller,
        )
        .execute(self.store.pool())
        .await?;
        Ok(())
    }

    /// The run's Task that dispatched `execution` of n8n `instance`, and its caller, if
    /// one did.
    pub async fn dispatched(
        &self,
        tenant: TenantId,
        instance: uuid::Uuid,
        execution: &ExecutionId,
    ) -> Result<Option<Dispatched>, StoreError> {
        Ok(sqlx::query_file!(
            "src/store/runs/dispatched.sql",
            tenant as TenantId,
            instance,
            execution.as_str(),
        )
        .fetch_optional(self.store.pool())
        .await?
        .map(|row| Dispatched {
            task_id: row.task_id,
            caller: row.caller,
        }))
    }

    /// Forget the dispatched executions n8n no longer keeps, after its default pruning of
    /// 336 hours, and those of an n8n instance other than `instance`.
    pub async fn prune_dispatched(
        &self,
        tenant: TenantId,
        instance: uuid::Uuid,
    ) -> Result<u64, StoreError> {
        Ok(sqlx::query_file!(
            "src/store/runs/prune_dispatched.sql",
            tenant as TenantId,
            instance,
        )
        .execute(self.store.pool())
        .await?
        .rows_affected())
    }

    /// Forget `task_id`'s run once its Task ended.
    pub async fn forget(&self, tenant: TenantId, task_id: uuid::Uuid) -> Result<(), StoreError> {
        sqlx::query_file!("src/store/runs/forget_run.sql", tenant as TenantId, task_id)
            .execute(self.store.pool())
            .await?;
        Ok(())
    }
}
