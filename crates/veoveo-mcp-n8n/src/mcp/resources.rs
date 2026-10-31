//! Tenant admission is current on every exact read and collection page. Whether a
//! workflow is active comes from n8n at the moment of the read.
use super::{N8nMcp, n8n_error, policy, store_error};
use crate::{
    contract::{
        EditorView, ExecutionNumber, ExecutionView, Executions, N8nPermission, N8nResource,
        ReadEditor, ReadExecution, ReadExecutions, ReadWorkflow, ReadWorkflows, WorkflowSummary,
        WorkflowView, Workflows,
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
            ResourceBuilder::new("The branch's workflows")
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
            ReadEditor,
            ResourceBuilder::new("n8n's editor")
                .reader(read_editor)
                .policy(policy(N8nPermission::Editor))
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

async fn read_editor(
    domain: N8nMcp,
    _: ReadEditor,
    context: RequestContext<RoleServer>,
) -> Result<EditorView, McpError> {
    let identity = hub_identity(&context)?;
    domain.tenant(&identity)?;
    Ok(EditorView {
        editor_url: domain.served.editor_url.to_string(),
        hooks_url: domain.served.hooks_url.to_string(),
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
            // A workflow the branch does not apply has no executions here.
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
        items.push(super::tools::view(&domain, tenant, execution, false).await?);
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
    let execution = domain
        .n8n
        .execution(&read.execution, true)
        .await
        .map_err(n8n_error)?
        .ok_or_else(|| McpError::invalid_params("no such execution", None))?;
    super::tools::view(&domain, tenant, execution, true).await
}
