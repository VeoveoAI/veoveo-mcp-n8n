//! Tenant admission is current on every exact read and collection page. Whether a
//! workflow is active comes from n8n at the moment of the read, and what of an execution
//! its reader reads from the data rule at the moment of the read.
use super::{N8nMcp, n8n_error, policy, store_error};
use crate::{
    contract::{
        AppliedFile, ExecutionNumber, ExecutionView, Executions, N8nPermission, N8nResource,
        ReadExecution, ReadExecutions, ReadStatus, ReadWorkflow, ReadWorkflows, StatusView,
        WorkflowSummary, WorkflowView, Workflows,
    },
    store::AppliedWorkflow,
    workflows::webhooks,
};
use rmcp::{ErrorData as McpError, RoleServer, service::RequestContext};
use veoveo_hub_sdk::{
    definition::ReadCollection,
    hosting::{ReadCache, ResourceBuilder, RoutesBuilder, hub_identity},
};
use veoveo_types::{LiveKeyset, Page, Pager};

pub(super) fn register(routes: RoutesBuilder<N8nMcp>) -> RoutesBuilder<N8nMcp> {
    routes
        .template::<ReadWorkflow, _, _>(
            ResourceBuilder::new("Workflow")
                .reader(read_workflow)
                .policy(policy(N8nPermission::WorkflowsRead))
                .cache(ReadCache::NoStore),
        )
        .collection_resource::<ReadWorkflows, _, _>(
            Some(ReadWorkflows),
            ResourceBuilder::new("The team's workflows")
                .reader(read_workflows)
                .policy(policy(N8nPermission::WorkflowsRead))
                .cache(ReadCache::NoStore),
        )
        .template::<ReadExecution, _, _>(
            ResourceBuilder::new("Execution")
                .reader(read_execution)
                .policy(policy(N8nPermission::WorkflowsRead))
                .cache(ReadCache::NoStore),
        )
        .collection_resource::<ReadExecutions, _, _>(
            Some(ReadExecutions {
                workflow: None,
                status: None,
            }),
            ResourceBuilder::new("n8n's executions")
                .reader(read_executions)
                .policy(policy(N8nPermission::WorkflowsRead))
                .cache(ReadCache::NoStore),
        )
        .singleton(
            ReadStatus,
            ResourceBuilder::new("What this runner runs")
                .reader(read_status)
                .policy(policy(N8nPermission::WorkflowsRead))
                .cache(ReadCache::NoStore),
        )
}

/// Whether n8n has `applied` published now; a workflow n8n no longer holds is not.
async fn active(domain: &N8nMcp, applied: &AppliedWorkflow) -> Result<bool, McpError> {
    Ok(domain
        .n8n
        .workflow(&applied.n8n_id)
        .await
        .map_err(n8n_error)?
        .is_some_and(|workflow| workflow.is_active() && !workflow.is_archived))
}

async fn read_workflows(
    domain: N8nMcp,
    read: ReadCollection<ReadWorkflows>,
    context: RequestContext<RoleServer>,
) -> Result<Page<Workflows>, McpError> {
    let identity = hub_identity(&context)?;
    let tenant = domain.tenant(&identity)?;
    let (pager, rows) = domain
        .workflows
        .page(tenant, read.selection())
        .await
        .map_err(store_error)?;
    let mut items = Vec::with_capacity(rows.len());
    for applied in rows {
        let active = active(&domain, &applied).await?;
        items.push(WorkflowSummary {
            workflow_uri: N8nResource::Workflow(applied.workflow.clone()),
            workflow: applied.workflow,
            runnable: applied.runnable,
            active,
            file_sha256: applied.digest,
        });
    }
    pager
        .finish(items)
        .map_err(|error| store_error(error.into()))
}

async fn read_workflow(
    domain: N8nMcp,
    read: ReadWorkflow,
    context: RequestContext<RoleServer>,
) -> Result<WorkflowView, McpError> {
    let identity = hub_identity(&context)?;
    let tenant = domain.tenant(&identity)?;
    let applied = domain
        .workflows
        .workflow(tenant, &read.workflow)
        .await
        .map_err(store_error)?
        .ok_or_else(|| McpError::invalid_params("no such workflow", None))?;
    let active = active(&domain, &applied).await?;
    Ok(WorkflowView {
        workflow_uri: N8nResource::Workflow(applied.workflow.clone()),
        webhooks: webhooks(&applied.applied, &domain.served.hooks_url),
        workflow: applied.workflow,
        runnable: applied.runnable,
        active,
        file_sha256: applied.digest,
        applied: applied.applied,
    })
}

async fn read_status(
    domain: N8nMcp,
    _: ReadStatus,
    context: RequestContext<RoleServer>,
) -> Result<StatusView, McpError> {
    let identity = hub_identity(&context)?;
    let tenant = domain.tenant(&identity)?;
    let workflows = domain
        .workflows
        .recorded(tenant)
        .await
        .map_err(store_error)?
        .into_iter()
        .map(|applied| AppliedFile {
            workflow: applied.workflow,
            file_sha256: applied.digest,
        })
        .collect();
    let repository = identity
        .repository_grant()
        .map(|grant| grant.repository.to_string());
    let edit_url = repository.as_ref().map(|repository| {
        let mut url = domain.served.issuer.clone();
        url.set_path("/computer/apps/n8n/open");
        url.query_pairs_mut().append_pair("repository", repository);
        url.to_string()
    });
    Ok(StatusView {
        n8n_version: domain.served.n8n_version.clone(),
        server_version: env!("CARGO_PKG_VERSION").to_owned(),
        started_at: domain.served.started_at.clone(),
        workflows,
        hooks_url: domain.served.hooks_url.to_string(),
        writer: N8nMcp::writer(&identity),
        tracked_branch: repository
            .as_ref()
            .and(domain.served.tracked_branch.clone()),
        repository,
        edit_url,
    })
}

async fn read_executions(
    domain: N8nMcp,
    read: ReadCollection<ReadExecutions>,
    context: RequestContext<RoleServer>,
) -> Result<Page<Executions>, McpError> {
    let identity = hub_identity(&context)?;
    let tenant = domain.tenant(&identity)?;
    let request = read.selection();
    let filters = request.filters().clone();
    let pager = Pager::<Executions>::new(
        tenant,
        filters.clone(),
        request.limit(),
        LiveKeyset,
        request.cursor(),
    )
    .map_err(|error| store_error(error.into()))?;
    let workflow = match &filters.workflow {
        Some(name) => match domain
            .workflows
            .workflow(tenant, name)
            .await
            .map_err(store_error)?
        {
            Some(applied) => Some(applied.n8n_id),
            // A workflow the team does not apply has no executions here.
            None => {
                return pager
                    .finish(Vec::new())
                    .map_err(|error| store_error(error.into()));
            }
        },
        None => None,
    };
    let limit = u32::try_from(pager.fetch_limit()).unwrap_or(u32::MAX);
    let page = domain
        .n8n
        .executions(
            workflow.as_ref(),
            filters.status,
            limit,
            pager.after().map(|after| after.0),
            false,
        )
        .await
        .map_err(n8n_error)?;
    let mut items = Vec::with_capacity(page.executions.len());
    for execution in page.executions {
        if ExecutionNumber::of(&execution.id).is_none() {
            continue;
        }
        items.push(super::tools::view(&domain, tenant, execution, None).await?);
    }
    pager
        .finish(items)
        .map_err(|error| store_error(error.into()))
}

async fn read_execution(
    domain: N8nMcp,
    read: ReadExecution,
    context: RequestContext<RoleServer>,
) -> Result<ExecutionView, McpError> {
    let identity = hub_identity(&context)?;
    let tenant = domain.tenant(&identity)?;
    let access = domain.access(tenant, &identity, &read.execution).await?;
    let execution = domain
        .n8n
        .execution(&read.execution, true)
        .await
        .map_err(n8n_error)?
        .ok_or_else(|| McpError::invalid_params("no such execution", None))?;
    super::tools::view(&domain, tenant, execution, Some(access)).await
}
