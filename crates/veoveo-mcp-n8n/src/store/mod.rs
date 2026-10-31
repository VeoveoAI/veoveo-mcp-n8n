//! The server's own tables in the `n8n` schema: what it applied from the branch's
//! `workflows/` for its tenant. Each statement is a `.sql` file under `workflows/`, run
//! through sqlx's checked `query_file*!` macros. n8n's own `n8n_*` tables share the
//! schema; no statement here names them.
use crate::{
    contract::{FileDigest, WorkflowName, Workflows},
    n8n::N8nWorkflowId,
};
use veoveo_hub_sdk::definition::CollectionRequest;
use veoveo_store::{PlatformStore, TenantId};
use veoveo_types::{LiveKeyset, Pager, PagingError};

/// The most workflows one branch holds.
pub const MAX_WORKFLOWS: usize = 500;

/// One workflow as the server applied it.
#[derive(Debug, Clone, PartialEq)]
pub struct AppliedWorkflow {
    pub workflow: WorkflowName,
    pub n8n_id: N8nWorkflowId,
    pub digest: FileDigest,
    /// The n8n version the server's apply produced.
    pub version: uuid::Uuid,
    pub runnable: bool,
    /// The JSON the server sent to n8n.
    pub applied: serde_json::Value,
}

/// A row as the database returns it, before its values are admitted again.
struct AppliedRow {
    workflow: String,
    n8n_workflow_id: String,
    file_sha256: Vec<u8>,
    n8n_version_id: uuid::Uuid,
    runnable: bool,
    applied: serde_json::Value,
}

impl TryFrom<AppliedRow> for AppliedWorkflow {
    type Error = StoreError;
    fn try_from(row: AppliedRow) -> Result<Self, Self::Error> {
        Ok(Self {
            workflow: WorkflowName::parse(row.workflow)
                .map_err(|_| StoreError::Corrupt("workflow name"))?,
            n8n_id: N8nWorkflowId::parse(row.n8n_workflow_id)
                .map_err(|_| StoreError::Corrupt("n8n workflow ID"))?,
            digest: FileDigest::from_bytes(
                row.file_sha256
                    .try_into()
                    .map_err(|_| StoreError::Corrupt("file digest"))?,
            ),
            version: row.n8n_version_id,
            runnable: row.runnable,
            applied: row.applied,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("the cursor belongs to another listing or tenant")]
    Paging(#[from] PagingError),
    #[error("a stored {0} fails admission")]
    Corrupt(&'static str),
    #[error("the tenant has more than {MAX_WORKFLOWS} applied workflows")]
    TooMany,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// The stored identity of a tenant, as the identity schema derives it.
pub fn stored_tenant(
    tenant: &veoveo_types::TenantId,
) -> Result<TenantId, veoveo_store::StoreError> {
    veoveo_store::deterministic_tenant_id(tenant.as_str())
}

/// The server's applied workflows in its own schema.
#[derive(Clone)]
pub struct WorkflowStore {
    store: PlatformStore,
}

impl WorkflowStore {
    pub fn new(store: PlatformStore) -> Self {
        Self { store }
    }

    /// Every workflow the server applied for `tenant`, in name order.
    pub async fn recorded(&self, tenant: TenantId) -> Result<Vec<AppliedWorkflow>, StoreError> {
        let bound = i64::try_from(MAX_WORKFLOWS + 1).expect("a small bound");
        let rows = sqlx::query_file_as!(
            AppliedRow,
            "src/store/workflows/recorded_workflows.sql",
            tenant as TenantId,
            bound,
        )
        .fetch_all(self.store.pool())
        .await?;
        if rows.len() > MAX_WORKFLOWS {
            return Err(StoreError::TooMany);
        }
        rows.into_iter().map(AppliedWorkflow::try_from).collect()
    }

    /// Record `applied` for `tenant`, replacing what the workflow's last apply recorded.
    pub async fn record(
        &self,
        tenant: TenantId,
        applied: &AppliedWorkflow,
    ) -> Result<(), StoreError> {
        sqlx::query_file!(
            "src/store/workflows/record_workflow.sql",
            tenant as TenantId,
            applied.workflow.as_str(),
            applied.n8n_id.as_str(),
            applied.digest.as_bytes().as_slice(),
            applied.version,
            applied.runnable,
            applied.applied,
        )
        .execute(self.store.pool())
        .await?;
        Ok(())
    }

    /// Forget `workflow`, whose file left the branch.
    pub async fn forget(
        &self,
        tenant: TenantId,
        workflow: &WorkflowName,
    ) -> Result<(), StoreError> {
        sqlx::query_file!(
            "src/store/workflows/forget_workflow.sql",
            tenant as TenantId,
            workflow.as_str(),
        )
        .execute(self.store.pool())
        .await?;
        Ok(())
    }

    /// `tenant`'s applied `workflow`, or `None` when the branch has no such workflow.
    pub async fn workflow(
        &self,
        tenant: TenantId,
        workflow: &WorkflowName,
    ) -> Result<Option<AppliedWorkflow>, StoreError> {
        sqlx::query_file_as!(
            AppliedRow,
            "src/store/workflows/workflow.sql",
            tenant as TenantId,
            workflow.as_str(),
        )
        .fetch_optional(self.store.pool())
        .await?
        .map(AppliedWorkflow::try_from)
        .transpose()
    }

    /// One page of `tenant`'s applied workflows in name order, after the request's cursor,
    /// with the pager that finishes the page once each row became an item. A cursor
    /// another tenant's listing issued is refused.
    pub async fn page(
        &self,
        tenant: TenantId,
        request: &CollectionRequest<Workflows>,
    ) -> Result<(Pager<Workflows>, Vec<AppliedWorkflow>), StoreError> {
        let pager = Pager::<Workflows>::new(
            tenant,
            *request.filters(),
            request.limit(),
            LiveKeyset,
            request.cursor(),
        )?;
        let rows = match pager.after() {
            None => {
                sqlx::query_file_as!(
                    AppliedRow,
                    "src/store/workflows/workflow_page_first.sql",
                    tenant as TenantId,
                    pager.fetch_limit()
                )
                .fetch_all(self.store.pool())
                .await?
            }
            Some(after) => {
                sqlx::query_file_as!(
                    AppliedRow,
                    "src/store/workflows/workflow_page_next.sql",
                    tenant as TenantId,
                    after.as_str(),
                    pager.fetch_limit()
                )
                .fetch_all(self.store.pool())
                .await?
            }
        };
        let rows = rows
            .into_iter()
            .map(AppliedWorkflow::try_from)
            .collect::<Result<Vec<_>, _>>()?;
        Ok((pager, rows))
    }
}
