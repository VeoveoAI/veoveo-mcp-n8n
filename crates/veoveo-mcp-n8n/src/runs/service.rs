//! The MCP Tasks of `run`: a Task-augmented call creates the caller's Task, and the caller
//! reads, cancels and listens to it through the SDK's durable Task boundary.
use rmcp::{
    ErrorData as McpError, RoleServer,
    model::{
        CallToolRequestParams, CreateTaskResult, GetTaskParams, GetTaskResult, UpdateTaskParams,
    },
    service::RequestContext,
};
use veoveo_hub_sdk::{
    HubInternalIdentity, PrincipalKind,
    calls::ToolCall,
    durable_tasks::{DurableTaskService, DurableTaskSubscription},
};
use veoveo_task::{CreateTask, RecoveryClass, TaskOwner, TaskRuntime, TaskSurface};
use veoveo_types::TaskId;

use super::{admit_input, run_task_type, trigger_path};
use crate::{
    contract::RunRequest,
    store::{WorkflowStore, stored_tenant},
};

/// How long a run's Task stays readable after it ends.
const TASK_TTL_MS: u64 = 7 * 24 * 60 * 60 * 1_000;
/// How often a caller polls a run's Task.
const TASK_POLL_INTERVAL_MS: u64 = 2_000;

/// The Task service of the branch's one tenant.
#[derive(Clone)]
pub struct N8nTasks {
    runtime: TaskRuntime,
    workflows: WorkflowStore,
    tenant: veoveo_types::TenantId,
}

impl N8nTasks {
    pub fn new(
        runtime: TaskRuntime,
        workflows: WorkflowStore,
        tenant: veoveo_types::TenantId,
    ) -> Self {
        Self {
            runtime,
            workflows,
            tenant,
        }
    }
}

/// A caller the Hub forwarded, of the branch's tenant.
#[derive(Clone)]
pub struct Caller {
    identity: HubInternalIdentity,
    owner: TaskOwner,
}

impl DurableTaskService for N8nTasks {
    type Caller = Caller;

    fn authenticate(&self, context: &RequestContext<RoleServer>) -> Result<Caller, McpError> {
        let identity = veoveo_hub_sdk::hosting::hub_identity(context)?;
        if identity.authority.tenant != self.tenant {
            return Err(McpError::invalid_request(
                "this n8n instance serves another tenant",
                None,
            ));
        }
        let owner = task_owner(&identity)?;
        Ok(Caller { identity, owner })
    }

    async fn start_tool_task(
        &self,
        caller: &Caller,
        request: CallToolRequestParams,
    ) -> Result<Option<CreateTaskResult>, McpError> {
        if request.name.as_ref() != RunRequest::TOOL {
            return Ok(None);
        }
        let arguments = serde_json::Value::Object(request.arguments.clone().unwrap_or_default());
        let run: RunRequest = serde_json::from_value(arguments)
            .map_err(|error| McpError::invalid_params(error.to_string(), None))?;
        admit_input(&run.input)
            .map_err(|error| McpError::invalid_params(error.to_string(), None))?;
        let tenant = stored_tenant(&caller.identity.authority.tenant).map_err(|_| {
            McpError::invalid_request("the forwarded identity names no stored tenant", None)
        })?;
        let applied = self
            .workflows
            .workflow(tenant, &run.workflow)
            .await
            .map_err(|error| McpError::internal_error(error.to_string(), None))?
            .ok_or_else(|| McpError::invalid_params("no such workflow", None))?;
        if trigger_path(&applied).is_none() {
            return Err(McpError::invalid_params(
                "the workflow does not start with the Veoveo Trigger, so it cannot run",
                None,
            ));
        }
        let retention_pins = veoveo_hub_sdk::durable_tasks::retention_pins(request.meta.as_ref())?;
        let created = self
            .runtime
            .create(CreateTask {
                task_id: TaskId::new(),
                owner: caller.owner.clone(),
                server: self.runtime.server().clone(),
                task_type: run_task_type(),
                request: serde_json::to_value(&run)
                    .map_err(|error| McpError::internal_error(error.to_string(), None))?,
                recovery_class: RecoveryClass::Resume,
                idempotency_key: None,
                ttl_ms: Some(TASK_TTL_MS),
                poll_interval_ms: Some(TASK_POLL_INTERVAL_MS),
                retention_pins,
            })
            .await
            .map_err(|error| McpError::internal_error(error.to_string(), None))?;
        tracing::info!(
            task = %created.snapshot.task_id, workflow = %run.workflow,
            principal = caller.identity.actor.id.as_str(), "created a run"
        );
        Ok(Some(CreateTaskResult::new(
            veoveo_hub_sdk::durable_tasks::task_seed(&created.snapshot),
        )))
    }

    async fn get_task(
        &self,
        caller: &Caller,
        request: GetTaskParams,
    ) -> Result<GetTaskResult, McpError> {
        veoveo_hub_sdk::durable_tasks::get_durable_task(
            &self.runtime,
            &self.runtime.for_owner(&caller.owner),
            request,
        )
        .await
    }

    async fn update_task(
        &self,
        caller: &Caller,
        request: UpdateTaskParams,
    ) -> Result<(), McpError> {
        veoveo_hub_sdk::durable_tasks::update_durable_task(
            &self.runtime.for_owner(&caller.owner),
            request,
        )
        .await
    }

    async fn cancel_task(&self, caller: &Caller, task_id: String) -> Result<(), McpError> {
        veoveo_hub_sdk::durable_tasks::cancel_durable_task(
            &self.runtime.for_owner(&caller.owner),
            task_id,
        )
        .await
    }

    async fn subscribe_tasks(
        &self,
        caller: &Caller,
        task_ids: Vec<String>,
    ) -> Result<DurableTaskSubscription, McpError> {
        veoveo_hub_sdk::durable_tasks::subscribe_durable_tasks(
            &self.runtime,
            &self.runtime.for_owner(&caller.owner),
            task_ids,
        )
        .await
    }
}

/// The caller as a Task owner. A Task's surface is the Hub profile the call arrived on,
/// so the caller reads it back only through that profile.
fn task_owner(identity: &HubInternalIdentity) -> Result<TaskOwner, McpError> {
    let surface = TaskSurface::parse(identity.profile.as_str())
        .map_err(|_| McpError::invalid_request("the Hub profile is not a Task surface", None))?;
    Ok(TaskOwner {
        principal_key: identity.actor.id.clone(),
        principal_kind: match identity.actor.kind {
            PrincipalKind::User => veoveo_task::PrincipalKind::User,
            PrincipalKind::Service => veoveo_task::PrincipalKind::Service,
        },
        issuer: identity.actor.issuer.clone(),
        subject: identity.actor.subject.clone(),
        surface,
        tenant_key: identity.actor.tenant.clone(),
        data_labels: identity.actor.data_labels.clone(),
        authority: identity.authority.clone(),
    })
}
