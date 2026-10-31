//! Runs as durable MCP Tasks. `run` creates a Task for its caller; the worker dispatches
//! one operation to n8n: it calls the workflow's Veoveo Trigger on loopback with the
//! operation ID, a fresh run token, the callback URL and the input, and records the
//! execution the trigger answers with. The Task then parks. The Veoveo Result node's
//! callback on the server's loopback listener, or a status check through n8n's public API,
//! settles the operation, and the Task completes, fails or ends as cancelled.
mod job;
mod outcome;
mod service;

use std::time::Duration;

use anyhow::{Context, Result, bail};
use secrecy::ExposeSecret;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use url::Url;
use veoveo_hub_sdk::{
    durable_tasks::{CallbackUrls, OperationDispatch, TaskContext, TaskWorker},
    hosting::ServerCalls,
};
use veoveo_task::{
    CheckSchedule, OperationKey, OperationSpec, ProviderName, StepDispatch, Submission,
    TaskFailure, TaskRuntime, TaskTransition, WaitOutcome,
};
use veoveo_types::TaskTypeName;

pub use outcome::{N8nOperations, RunOutcome};
pub use service::N8nTasks;

use crate::{
    contract::{ExecutionId, N8nResource, RunRequest, RunResult},
    n8n::{N8nClient, VEOVEO_TRIGGER, generate_secret},
    store::{AppliedWorkflow, RunStore, WorkflowStore},
};

/// The Task type of a run.
pub fn run_task_type() -> TaskTypeName {
    TaskTypeName::from_static("run")
}

/// How long a run's execution may take before the server stops it.
pub const RUN_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);
/// The status checks of a run's execution: the first two seconds after the dispatch, then
/// each interval 1.2 times the one before, at most 60 within the run's deadline. The
/// Veoveo Result's callback settles a run sooner.
const CHECK_FIRST: Duration = Duration::from_secs(2);
const CHECK_INTERVAL: Duration = Duration::from_secs(2);
const CHECK_BACKOFF: f64 = 1.2;
const CHECK_LIMIT: u32 = 60;
/// The provider of every run's operation.
const PROVIDER: &str = "n8n";
/// The key of the one operation of a run.
const OPERATION: &str = "execution";

/// What the worker needs to run a workflow.
#[derive(Clone)]
pub struct Runner {
    pub runtime: TaskRuntime,
    pub workflows: WorkflowStore,
    pub runs: RunStore,
    pub n8n: N8nClient,
    /// The stored identity of the branch's one tenant.
    pub tenant: veoveo_store::TenantId,
    /// The n8n instance whose data the server keeps the key of, which names each job.
    pub instance: uuid::Uuid,
    /// The server's loopback listener, where the Veoveo nodes reach the server.
    pub loopback: Url,
}

impl Runner {
    /// Where the Veoveo Result node posts a run's output.
    pub fn callbacks(&self) -> CallbackUrls {
        CallbackUrls::new(self.loopback.clone(), crate::server())
            .expect("the loopback listener is an HTTP URL")
    }

    /// Where the Veoveo Tool node calls other servers through the bridge.
    pub fn bridge_url(&self) -> Url {
        crate::loopback::bridge_url(&self.loopback)
    }

    /// Claim and run the server's Tasks until `stop` is cancelled. A parked Task holds no
    /// worker; a restarted worker runs it again from its recorded dispatch.
    pub async fn work(self, calls: Option<ServerCalls>, stop: CancellationToken) -> Result<()> {
        let mut worker = TaskWorker::new(self.runtime.clone()).callbacks(self.callbacks());
        if let Some(calls) = calls {
            worker = worker.calls(calls);
        }
        worker
            .run(
                move |ctx| {
                    let runner = self.clone();
                    async move { runner.run(ctx).await }
                },
                stop,
            )
            .await?;
        Ok(())
    }

    /// One claimed run Task.
    async fn run(&self, ctx: TaskContext) -> Result<TaskTransition> {
        let snapshot = ctx.task();
        if snapshot.task_type != run_task_type() {
            return Ok(failed("unsupported_task", "the Task is not an n8n run"));
        }
        let Ok(request) = serde_json::from_value::<RunRequest>(snapshot.request.clone()) else {
            return Ok(failed("invalid_run", "the Task's request is not a run"));
        };
        let task_id = ctx.task_id().as_uuid();
        let Some(applied) = self
            .workflows
            .workflow(self.tenant, &request.workflow)
            .await?
        else {
            return Ok(failed(
                "workflow_not_applied",
                format!("the branch applies no workflow {}", request.workflow),
            ));
        };
        let Some(path) = trigger_path(&applied) else {
            return Ok(failed(
                "workflow_not_runnable",
                format!("{} has no Veoveo Trigger", request.workflow),
            ));
        };
        let spec = OperationSpec::checks(
            OperationKey::parse(OPERATION)?,
            ProviderName::parse(PROVIDER)?,
            CheckSchedule::new(CHECK_FIRST, CHECK_INTERVAL, CHECK_BACKOFF, CHECK_LIMIT)?,
            RUN_TIMEOUT,
        );
        let operation = ctx
            .operation(&spec)
            .dispatch(|dispatch| self.dispatch(task_id, &request, &path, dispatch))
            .await?;
        let execution = operation.external_id.as_ref().and_then(job::execution);
        let outcome = ctx
            .wait::<RunOutcome, _>(&spec.key, &N8nOperations::new(self.n8n.clone()))
            .await?;
        let transition = match outcome {
            WaitOutcome::Settled(RunOutcome::Succeeded { execution, output }) => {
                let result = RunResult {
                    workflow: request.workflow.clone(),
                    execution_uri: N8nResource::Execution(execution.clone()),
                    execution,
                    output,
                };
                veoveo_hub_sdk::durable_tasks::mcp_task_completion(
                    "The workflow's execution succeeded",
                    veoveo_hub_sdk::hosting::task_result::<RunRequest>(&result)
                        .map_err(|error| anyhow::anyhow!(error.message.to_string()))?,
                )?
            }
            WaitOutcome::Settled(RunOutcome::Failed {
                execution,
                status,
                message,
            }) => failed(
                "execution_failed",
                format!("n8n execution {execution} ended {status}: {message}"),
            ),
            // A Task ends as cancelled only after its caller asked; an execution someone
            // stopped in n8n fails the run instead.
            WaitOutcome::Settled(RunOutcome::Stopped { execution }) => failed(
                "execution_stopped",
                format!("n8n execution {execution} was stopped in n8n"),
            ),
            WaitOutcome::Expired => {
                if let Some(execution) = &execution {
                    let stopped = self.n8n.stop_execution(execution).await;
                    tracing::warn!(%execution, ?stopped, "the run's execution passed its deadline");
                }
                failed(
                    "run_deadline_passed",
                    format!("the execution did not end within {RUN_TIMEOUT:?}"),
                )
            }
            WaitOutcome::Cancelled { receipt } => {
                tracing::info!(%receipt, "the run's cancellation stopped its execution");
                TaskTransition::Cancelled
            }
        };
        self.runs.forget(self.tenant, task_id).await?;
        tracing::info!(
            task = %ctx.task_id(), workflow = %request.workflow,
            execution = ?execution.as_ref().map(ExecutionId::as_str),
            outcome = ?outcome_name(&transition), "the run ended"
        );
        Ok(transition)
    }

    /// Start the run's execution once: record the run with a fresh token, call the
    /// workflow's Veoveo Trigger and record the execution it answers with. An attempt
    /// after one that recorded no submission looks for that attempt's execution first.
    async fn dispatch(
        &self,
        task_id: uuid::Uuid,
        request: &RunRequest,
        path: &str,
        dispatch: OperationDispatch,
    ) -> Result<Submission> {
        let operation_id = dispatch.operation_id.to_string();
        if let StepDispatch::Unsettled { .. } = dispatch.dispatch
            && let Some(execution) = self.reconcile(task_id, &operation_id).await?
        {
            tracing::info!(%execution, "found the execution an earlier dispatch started");
            return Ok(Submission::job(job::job(self.instance, &execution)));
        }
        let token = generate_secret()?;
        self.runs
            .record(
                self.tenant,
                task_id,
                &request.workflow,
                uuid::Uuid::parse_str(&operation_id)?,
                token.expose_secret(),
            )
            .await?;
        let body = json!({
            "operationId": operation_id,
            "runToken": token.expose_secret(),
            "callbackUrl": dispatch.callback_url.as_ref().map(Url::as_str),
            "bridgeUrl": self.bridge_url().as_str(),
            "input": request.input,
        });
        let execution = self
            .n8n
            .dispatch(path, &body)
            .await
            .context("calling the workflow's Veoveo Trigger")?;
        self.runs
            .record_execution(self.tenant, task_id, &execution)
            .await?;
        tracing::info!(workflow = %request.workflow, %execution, "dispatched the run");
        Ok(Submission::job(job::job(self.instance, &execution)))
    }

    /// The execution an earlier dispatch of `task_id` started: the one it recorded, or
    /// one n8n holds whose Veoveo Trigger emitted `operation_id`.
    async fn reconcile(
        &self,
        task_id: uuid::Uuid,
        operation_id: &str,
    ) -> Result<Option<ExecutionId>> {
        let Some(run) = self.runs.run(self.tenant, task_id).await? else {
            return Ok(None);
        };
        if let Some(execution) = run.execution {
            return Ok(Some(execution));
        }
        let applied = self
            .workflows
            .workflow(self.tenant, &run.workflow)
            .await?
            .context("the run's workflow is no longer applied")?;
        for status in [Some(crate::contract::ExecutionStatus::Running), None] {
            let page = self
                .n8n
                .executions(Some(&applied.n8n_id), status, 100, None, true)
                .await?;
            if let Some(found) = page
                .executions
                .iter()
                .find(|execution| execution.operation() == Some(operation_id))
            {
                self.runs
                    .record_execution(self.tenant, task_id, &found.id)
                    .await?;
                return Ok(Some(found.id.clone()));
            }
        }
        Ok(None)
    }
}

/// The path of `applied`'s Veoveo Trigger under n8n's production webhooks: n8n serves a
/// trigger with a webhook ID at `{webhookId}/run`.
pub fn trigger_path(applied: &AppliedWorkflow) -> Option<String> {
    applied
        .applied
        .get("nodes")?
        .as_array()?
        .iter()
        .filter(|node| node.get("type").and_then(Value::as_str) == Some(VEOVEO_TRIGGER))
        .filter(|node| node.get("disabled").and_then(Value::as_bool) != Some(true))
        .find_map(|node| node.get("webhookId").and_then(Value::as_str))
        .map(|webhook| format!("{webhook}/run"))
}

fn failed(code: &str, message: impl std::fmt::Display) -> TaskTransition {
    TaskTransition::Failed(TaskFailure::new(code, message.to_string()))
}

fn outcome_name(transition: &TaskTransition) -> &'static str {
    match transition {
        TaskTransition::Succeeded { .. } => "succeeded",
        TaskTransition::Failed(_) => "failed",
        TaskTransition::Cancelled => "cancelled",
        TaskTransition::Running { .. } | TaskTransition::Waiting { .. } => "running",
    }
}

/// Whether `input` is what `run` admits: a JSON object.
pub fn admit_input(input: &Value) -> Result<()> {
    if !input.is_object() {
        bail!("a run's input is a JSON object");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{contract::FileDigest, n8n::N8nWorkflowId};

    fn applied(nodes: Value) -> AppliedWorkflow {
        AppliedWorkflow {
            workflow: crate::contract::WorkflowName::parse("hello").unwrap(),
            n8n_id: N8nWorkflowId::parse("Wf1").unwrap(),
            digest: FileDigest::from_bytes([0; 32]),
            version: uuid::Uuid::nil(),
            runnable: true,
            applied: json!({"nodes": nodes}),
        }
    }

    #[test]
    fn the_trigger_is_served_under_its_webhook_id() {
        let workflow = applied(json!([
            {"name": "Off", "type": VEOVEO_TRIGGER, "webhookId": "off", "disabled": true},
            {"name": "Veoveo Trigger", "type": VEOVEO_TRIGGER, "webhookId": "4f9a"},
        ]));
        assert_eq!(trigger_path(&workflow).as_deref(), Some("4f9a/run"));
        assert!(trigger_path(&applied(json!([{"type": VEOVEO_TRIGGER}]))).is_none());
        assert!(
            trigger_path(&applied(
                json!([{"type": "n8n-nodes-base.webhook", "webhookId": "x"}])
            ))
            .is_none()
        );
    }

    #[test]
    fn a_run_takes_an_object() {
        assert!(admit_input(&json!({"a": 1})).is_ok());
        assert!(admit_input(&json!([1])).is_err());
    }
}
