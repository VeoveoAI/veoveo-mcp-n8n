//! The tools: `run` as an MCP Task, `retry_execution`, `stop_execution` and
//! `editor_changes`.
use rmcp::{ErrorData as McpError, RoleServer, model::ClientCapabilities, service::RequestContext};
use veoveo_hub_sdk::hosting::{RoutesBuilder, hub_identity};

use super::{N8nMcp, n8n_error, policy, store_error};
use crate::{
    contract::{
        EditorChange, EditorChanges, EditorChangesRequest, ExecutionId, ExecutionView,
        N8nPermission, N8nResource, RetryExecutionRequest, RunRequest, RunResult,
        StopExecutionRequest,
    },
    n8n::{N8nError, N8nExecution},
    workflows::file_members,
};

pub(super) fn register(routes: RoutesBuilder<N8nMcp>) -> RoutesBuilder<N8nMcp> {
    routes
        .task::<RunRequest, _, _>(
            "Run a workflow that starts with the Veoveo Trigger, by its file name, with a JSON object as its input. Run as an MCP Task; its result is the execution's output and address.",
            policy(N8nPermission::WorkflowsRun),
            |domain, request, context| async move { domain.run(request, context).await.map_err(Into::into) },
        )
        .tool::<RetryExecutionRequest, _, _>(
            "Retry a failed n8n execution; n8n runs it again from its failed node as a new execution.",
            policy(N8nPermission::ExecutionsWrite),
            |domain, request, context| async move { domain.retry_execution(request, context).await.map_err(Into::into) },
        )
        .tool::<StopExecutionRequest, _, _>(
            "Stop a running or waiting n8n execution.",
            policy(N8nPermission::ExecutionsWrite),
            |domain, request, context| async move { domain.stop_execution(request, context).await.map_err(Into::into) },
        )
        .tool::<EditorChangesRequest, _, _>(
            "List each workflow someone changed in n8n's editor since the branch applied it, with n8n's JSON of it to write to its file and push.",
            policy(N8nPermission::WorkflowsRead),
            |domain, request, context| async move { domain.editor_changes(request, context).await.map_err(Into::into) },
        )
}

/// An execution as the server serves it, its workflow named by its file.
pub(super) async fn view(
    domain: &N8nMcp,
    tenant: veoveo_store::TenantId,
    execution: N8nExecution,
    detailed: bool,
) -> Result<ExecutionView, McpError> {
    let workflow = match execution
        .workflow_id
        .as_deref()
        .and_then(|id| crate::n8n::N8nWorkflowId::parse(id).ok())
    {
        Some(id) => domain
            .workflows
            .by_n8n_id(tenant, &id)
            .await
            .map_err(store_error)?
            .map(|applied| applied.workflow),
        None => None,
    };
    let (output, error) = if detailed {
        (execution.output(), execution.error())
    } else {
        (None, None)
    };
    Ok(ExecutionView {
        execution_uri: N8nResource::Execution(execution.id.clone()),
        execution: execution.id,
        workflow,
        status: execution.status,
        mode: execution.mode,
        started_at: execution.started_at,
        stopped_at: execution.stopped_at,
        retry_of: execution.retry_of,
        output,
        error,
    })
}

/// n8n's refusal of an execution request, as the caller's error.
fn refused(error: N8nError) -> McpError {
    match error {
        N8nError::Refused { status, .. } if status == reqwest::StatusCode::NOT_FOUND => {
            McpError::invalid_params("no such execution", None)
        }
        N8nError::Refused {
            status, message, ..
        } if status.is_client_error() => McpError::invalid_request(message, None),
        other => n8n_error(other),
    }
}

impl N8nMcp {
    async fn run(
        &self,
        _request: RunRequest,
        _context: RequestContext<RoleServer>,
    ) -> Result<RunResult, McpError> {
        Err(McpError::missing_required_client_capability(
            ClientCapabilities::builder().enable_tasks().build(),
        ))
    }

    async fn execution_view(
        &self,
        tenant: veoveo_store::TenantId,
        execution: &ExecutionId,
    ) -> Result<ExecutionView, McpError> {
        let found = self
            .n8n
            .execution(execution, false)
            .await
            .map_err(n8n_error)?
            .ok_or_else(|| McpError::invalid_params("no such execution", None))?;
        view(self, tenant, found, false).await
    }

    async fn retry_execution(
        &self,
        request: RetryExecutionRequest,
        context: RequestContext<RoleServer>,
    ) -> Result<ExecutionView, McpError> {
        let identity = hub_identity(&context)?;
        let tenant = self.tenant(&identity)?;
        let retried = self
            .n8n
            .retry_execution(&request.execution)
            .await
            .map_err(refused)?;
        tracing::info!(
            execution = %request.execution, %retried,
            principal = identity.actor.id.as_str(), "retried an execution"
        );
        self.execution_view(tenant, &retried).await
    }

    async fn stop_execution(
        &self,
        request: StopExecutionRequest,
        context: RequestContext<RoleServer>,
    ) -> Result<ExecutionView, McpError> {
        let identity = hub_identity(&context)?;
        let tenant = self.tenant(&identity)?;
        self.n8n
            .stop_execution(&request.execution)
            .await
            .map_err(refused)?;
        tracing::info!(
            execution = %request.execution,
            principal = identity.actor.id.as_str(), "stopped an execution"
        );
        self.execution_view(tenant, &request.execution).await
    }

    async fn editor_changes(
        &self,
        _request: EditorChangesRequest,
        context: RequestContext<RoleServer>,
    ) -> Result<EditorChanges, McpError> {
        let identity = hub_identity(&context)?;
        let tenant = self.tenant(&identity)?;
        let mut workflows = Vec::new();
        for applied in self.workflows.recorded(tenant).await.map_err(store_error)? {
            let Some(current) = self
                .n8n
                .workflow(&applied.n8n_id)
                .await
                .map_err(n8n_error)?
            else {
                continue;
            };
            if current.version_id == applied.version {
                continue;
            }
            let Some(document) = self
                .n8n
                .workflow_document(&applied.n8n_id)
                .await
                .map_err(n8n_error)?
            else {
                continue;
            };
            workflows.push(EditorChange {
                file: format!("workflows/{}", applied.workflow.file_name()),
                workflow: applied.workflow,
                applied_version: applied.version.to_string(),
                current_version: current.version_id.to_string(),
                json: file_members(&document),
            });
        }
        Ok(EditorChanges { workflows })
    }
}
