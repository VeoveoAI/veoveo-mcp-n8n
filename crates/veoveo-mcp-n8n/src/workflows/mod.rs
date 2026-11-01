//! Workflows from Lore: at each start the server reads the team's `workflows/*.json`,
//! creates or updates each workflow in n8n by its file name and publishes it, archives
//! every workflow it applied whose file is gone, and records each file's SHA-256 and the
//! n8n version it produced. A workflow n8n refuses fails the start. A person's own
//! Computer applies a team's files the same way as drafts, recording them in a file of
//! its own.
mod files;
mod plan;

use std::{collections::BTreeMap, future::Future};

use anyhow::{Context, Result};
use veoveo_store::TenantId;

pub use files::{WorkflowFile, file_members, read_directory, webhooks};
pub use plan::{Activation, Decision, Recorded, plan, plan_for};

use crate::{
    contract::WorkflowName,
    n8n::{N8nClient, N8nWorkflow},
    store::{AppliedWorkflow, WorkflowStore},
};

/// Where a program records what it applied to n8n.
pub trait AppliedRecords: Sync {
    fn recorded(&self) -> impl Future<Output = Result<Vec<AppliedWorkflow>>> + Send;
    fn record(&self, applied: &AppliedWorkflow) -> impl Future<Output = Result<()>> + Send;
    fn forget(&self, workflow: &WorkflowName) -> impl Future<Output = Result<()>> + Send;
}

/// The server's records of one tenant, in its own table.
pub struct TenantRecords<'a> {
    pub store: &'a WorkflowStore,
    pub tenant: TenantId,
}

impl AppliedRecords for TenantRecords<'_> {
    async fn recorded(&self) -> Result<Vec<AppliedWorkflow>> {
        Ok(self.store.recorded(self.tenant).await?)
    }
    async fn record(&self, applied: &AppliedWorkflow) -> Result<()> {
        Ok(self.store.record(self.tenant, applied).await?)
    }
    async fn forget(&self, workflow: &WorkflowName) -> Result<()> {
        Ok(self.store.forget(self.tenant, workflow).await?)
    }
}

/// Apply `files` to n8n for `tenant`, publishing them, and record what was applied.
/// Returns each decision in the order it was carried out.
pub async fn apply(
    client: &N8nClient,
    store: &WorkflowStore,
    tenant: TenantId,
    files: &[WorkflowFile],
) -> Result<Vec<Decision>> {
    apply_to(
        client,
        &TenantRecords { store, tenant },
        files,
        Activation::Publish,
    )
    .await
}

/// Apply `files` to n8n with `activation` and record what was applied in `records`.
pub async fn apply_to(
    client: &N8nClient,
    records: &impl AppliedRecords,
    files: &[WorkflowFile],
    activation: Activation,
) -> Result<Vec<Decision>> {
    let recorded: BTreeMap<_, _> = records
        .recorded()
        .await
        .context("reading the applied workflows")?
        .into_iter()
        .map(|applied| {
            (
                applied.workflow,
                Recorded {
                    n8n_id: applied.n8n_id,
                    digest: applied.digest,
                    version: applied.version,
                },
            )
        })
        .collect();
    let mut current = BTreeMap::new();
    for (workflow, recorded) in &recorded {
        let found = client
            .workflow(&recorded.n8n_id)
            .await
            .with_context(|| format!("reading workflow {workflow} from n8n"))?;
        current.insert(workflow.clone(), found);
    }
    let digests: Vec<_> = files
        .iter()
        .map(|file| (file.workflow.clone(), file.digest))
        .collect();
    let decisions = plan_for(&digests, &recorded, &current, activation);
    let by_name: BTreeMap<_, _> = files.iter().map(|file| (&file.workflow, file)).collect();
    for decision in &decisions {
        let workflow = decision.workflow();
        carry_out(
            client,
            records,
            activation,
            decision,
            by_name.get(workflow).copied(),
        )
        .await
        .with_context(|| format!("applying {}", workflow.file_name()))?;
    }
    Ok(decisions)
}

async fn carry_out(
    client: &N8nClient,
    records: &impl AppliedRecords,
    activation: Activation,
    decision: &Decision,
    file: Option<&WorkflowFile>,
) -> Result<()> {
    let file = || file.context("the decision names a file of the team");
    match decision {
        Decision::Create { workflow } => {
            let file = file()?;
            let created = client.create_workflow(&file.body).await?;
            record(records, file, &created).await?;
            if activation == Activation::Draft {
                tracing::info!(workflow = %workflow, n8n_id = %created.id, digest = %file.digest, "created the draft");
                return Ok(());
            }
            let published = client.publish(&created.id).await?;
            record(records, file, &published).await?;
            tracing::info!(
                workflow = %workflow, n8n_id = %published.id, version = %published.version_id,
                digest = %file.digest, "created and published the workflow"
            );
        }
        Decision::Update {
            workflow,
            n8n_id,
            replaced,
            unarchive,
        } => {
            let file = file()?;
            if *unarchive {
                client.unarchive(n8n_id).await?;
                tracing::warn!(workflow = %workflow, n8n_id = %n8n_id, "restored the workflow someone archived in n8n");
            }
            let updated = client.update_workflow(n8n_id, &file.body).await?;
            record(records, file, &updated).await?;
            if activation == Activation::Draft {
                tracing::info!(workflow = %workflow, n8n_id = %n8n_id, digest = %file.digest, "updated the draft");
                return Ok(());
            }
            let published = client.publish(n8n_id).await?;
            record(records, file, &published).await?;
            if let Some(replaced) = replaced {
                tracing::warn!(
                    workflow = %workflow, n8n_id = %n8n_id, %replaced,
                    "the team's file replaced a version saved in n8n; n8n's history keeps it"
                );
            }
            tracing::info!(
                workflow = %workflow, n8n_id = %n8n_id, version = %published.version_id,
                digest = %file.digest, "updated and published the workflow"
            );
        }
        Decision::Publish { workflow, n8n_id } => {
            let file = file()?;
            let published = client.publish(n8n_id).await?;
            record(records, file, &published).await?;
            tracing::info!(workflow = %workflow, n8n_id = %n8n_id, "published the unchanged workflow");
        }
        Decision::Keep { workflow, n8n_id } => {
            tracing::info!(workflow = %workflow, n8n_id = %n8n_id, "the workflow is unchanged and published");
        }
        Decision::Archive { workflow, n8n_id } => {
            client.archive(n8n_id).await?;
            records.forget(workflow).await?;
            tracing::info!(workflow = %workflow, n8n_id = %n8n_id, "archived the workflow whose file left the team's files");
        }
        Decision::Forget { workflow } => {
            records.forget(workflow).await?;
            tracing::info!(workflow = %workflow, "forgot the workflow whose file left the team's files and n8n");
        }
    }
    Ok(())
}

async fn record(
    records: &impl AppliedRecords,
    file: &WorkflowFile,
    workflow: &N8nWorkflow,
) -> Result<()> {
    records
        .record(&AppliedWorkflow {
            workflow: file.workflow.clone(),
            n8n_id: workflow.id.clone(),
            digest: file.digest,
            version: workflow.version_id,
            runnable: file.runnable,
            applied: file.body.clone(),
        })
        .await
        .context("recording the applied workflow")?;
    Ok(())
}
