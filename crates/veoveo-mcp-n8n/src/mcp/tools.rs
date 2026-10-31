//! The tools: `run` as an MCP Task, `retry_execution` and `stop_execution`. Retrying or
//! stopping an execution needs the data rule's access to it beside
//! `n8n.executions.write`.
use rmcp::{ErrorData as McpError, RoleServer, model::ClientCapabilities, service::RequestContext};
use veoveo_hub_sdk::hosting::{RoutesBuilder, hub_identity};

use super::{N8nMcp, n8n_error, policy, store_error};
use crate::{
    contract::{
        ExecutionAccess, ExecutionId, ExecutionView, N8nPermission, N8nResource,
        RetryExecutionRequest, RunRequest, RunResult, StopExecutionRequest,
    },
    n8n::{N8nError, N8nExecution},
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
}

/// An execution as the server serves it, its workflow named by its file. A listing names
/// no `access` and carries no detail; a read of the one execution carries its failure,
/// and its data only with [`ExecutionAccess::Data`].
pub(super) async fn view(
    domain: &N8nMcp,
    tenant: veoveo_store::TenantId,
    execution: N8nExecution,
    access: Option<ExecutionAccess>,
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
    let (output, run_data, failed_node, error) = match access {
        Some(ExecutionAccess::Data) => (
            execution.output(),
            execution.run_data(),
            execution.failed_node(),
            execution.error(),
        ),
        Some(ExecutionAccess::Summary) => (None, None, execution.failed_node(), execution.error()),
        None => (None, None, None, None),
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
        access,
        output,
        run_data,
        failed_node,
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

    /// `execution` as its caller reads it now.
    async fn execution_view(
        &self,
        tenant: veoveo_store::TenantId,
        identity: &veoveo_hub_sdk::HubInternalIdentity,
        execution: &ExecutionId,
    ) -> Result<ExecutionView, McpError> {
        let access = self.access(tenant, identity, execution).await?;
        // The failure a summary names is in the execution's data, which the view keeps
        // from a reader without access.
        let found = self
            .n8n
            .execution(execution, true)
            .await
            .map_err(n8n_error)?
            .ok_or_else(|| McpError::invalid_params("no such execution", None))?;
        view(self, tenant, found, Some(access)).await
    }

    /// Refuse to act on an execution whose data the caller may not read: a run's belongs
    /// to its Task's caller, any other to the repository's writers.
    async fn admit_action(
        &self,
        tenant: veoveo_store::TenantId,
        identity: &veoveo_hub_sdk::HubInternalIdentity,
        execution: &ExecutionId,
    ) -> Result<(), McpError> {
        if self.access(tenant, identity, execution).await? == ExecutionAccess::Data {
            return Ok(());
        }
        Err(McpError::invalid_request(
            "only the caller of the run that started this execution, or a writer of the repository for one no run started, may act on it",
            None,
        ))
    }

    async fn retry_execution(
        &self,
        request: RetryExecutionRequest,
        context: RequestContext<RoleServer>,
    ) -> Result<ExecutionView, McpError> {
        let identity = hub_identity(&context)?;
        let tenant = self.tenant(&identity)?;
        self.admit_action(tenant, &identity, &request.execution)
            .await?;
        let dispatched = self
            .runs
            .dispatched(tenant, self.instance, &request.execution)
            .await
            .map_err(store_error)?;
        let retried = self
            .n8n
            .retry_execution(&request.execution)
            .await
            .map_err(refused)?;
        // A retry of a run's execution stays the run's caller's, who alone asked for it.
        if let Some(dispatched) = dispatched {
            self.runs
                .record_dispatched(
                    tenant,
                    self.instance,
                    &retried,
                    dispatched.task_id,
                    &dispatched.caller,
                )
                .await
                .map_err(store_error)?;
        }
        tracing::info!(
            execution = %request.execution, %retried,
            principal = identity.actor.id.as_str(), "retried an execution"
        );
        self.execution_view(tenant, &identity, &retried).await
    }

    async fn stop_execution(
        &self,
        request: StopExecutionRequest,
        context: RequestContext<RoleServer>,
    ) -> Result<ExecutionView, McpError> {
        let identity = hub_identity(&context)?;
        let tenant = self.tenant(&identity)?;
        self.admit_action(tenant, &identity, &request.execution)
            .await?;
        self.n8n
            .stop_execution(&request.execution)
            .await
            .map_err(refused)?;
        tracing::info!(
            execution = %request.execution,
            principal = identity.actor.id.as_str(), "stopped an execution"
        );
        self.execution_view(tenant, &identity, &request.execution)
            .await
    }
}
