//! Workflows from Lore: at each start the server reads the branch's `workflows/*.json`,
//! creates or updates each workflow in n8n by its file name and publishes it, archives
//! every workflow it applied whose file is gone, and records each file's SHA-256 and the
//! n8n version it produced. A workflow n8n refuses fails the start.
mod files;
mod plan;

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use veoveo_store::TenantId;

pub use files::{WorkflowFile, file_members, read_directory, webhooks};
pub use plan::{Decision, Recorded, plan};

use crate::{
    n8n::{N8nClient, N8nWorkflow},
    store::{AppliedWorkflow, WorkflowStore},
};

/// Apply `files` to n8n for `tenant` and record what was applied. Returns each decision
/// in the order it was carried out.
pub async fn apply(
    client: &N8nClient,
    store: &WorkflowStore,
    tenant: TenantId,
    files: &[WorkflowFile],
) -> Result<Vec<Decision>> {
    let recorded: BTreeMap<_, _> = store
        .recorded(tenant)
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
    let decisions = plan(&digests, &recorded, &current);
    let by_name: BTreeMap<_, _> = files.iter().map(|file| (&file.workflow, file)).collect();
    for decision in &decisions {
        let workflow = decision.workflow();
        carry_out(
            client,
            store,
            tenant,
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
    store: &WorkflowStore,
    tenant: TenantId,
    decision: &Decision,
    file: Option<&WorkflowFile>,
) -> Result<()> {
    let file = || file.context("the decision names a file of the branch");
    match decision {
        Decision::Create { workflow } => {
            let file = file()?;
            let created = client.create_workflow(&file.body).await?;
            record(store, tenant, file, &created).await?;
            let published = client.publish(&created.id).await?;
            record(store, tenant, file, &published).await?;
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
            record(store, tenant, file, &updated).await?;
            let published = client.publish(n8n_id).await?;
            record(store, tenant, file, &published).await?;
            if let Some(replaced) = replaced {
                tracing::warn!(
                    workflow = %workflow, n8n_id = %n8n_id, %replaced,
                    "the branch's file replaced a version saved in n8n; n8n's history keeps it"
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
            record(store, tenant, file, &published).await?;
            tracing::info!(workflow = %workflow, n8n_id = %n8n_id, "published the unchanged workflow");
        }
        Decision::Keep { workflow, n8n_id } => {
            tracing::info!(workflow = %workflow, n8n_id = %n8n_id, "the workflow is unchanged and published");
        }
        Decision::Archive { workflow, n8n_id } => {
            client.archive(n8n_id).await?;
            store.forget(tenant, workflow).await?;
            tracing::info!(workflow = %workflow, n8n_id = %n8n_id, "archived the workflow whose file left the branch");
        }
        Decision::Forget { workflow } => {
            store.forget(tenant, workflow).await?;
            tracing::info!(workflow = %workflow, "forgot the workflow whose file left the branch and n8n");
        }
    }
    Ok(())
}

async fn record(
    store: &WorkflowStore,
    tenant: TenantId,
    file: &WorkflowFile,
    workflow: &N8nWorkflow,
) -> Result<()> {
    store
        .record(
            tenant,
            &AppliedWorkflow {
                workflow: file.workflow.clone(),
                n8n_id: workflow.id.clone(),
                digest: file.digest,
                version: workflow.version_id,
                runnable: file.runnable,
                applied: file.body.clone(),
            },
        )
        .await
        .context("recording the applied workflow")?;
    Ok(())
}
