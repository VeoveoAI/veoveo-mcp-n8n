//! Runs and executions: the arguments of `run`, `retry_execution` and `stop_execution`,
//! what they return, and an n8n execution as the server serves it.
use std::{borrow::Cow, fmt, str::FromStr};

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};

use super::{ExecutionId, N8nResource, WorkflowName};

/// The arguments of `run`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[veoveo_hub_sdk::tool(provider = super::N8nProvider, output = RunResult, name = "run", task = true)]
pub struct RunRequest {
    /// The workflow to run, by its file name under `workflows/`. It starts with the
    /// Veoveo Trigger.
    pub workflow: WorkflowName,
    /// The workflow's input, the JSON object its Veoveo Trigger emits as `input`.
    #[serde(default = "empty_object")]
    #[schemars(schema_with = "object_schema")]
    pub input: serde_json::Value,
}

fn empty_object() -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::new())
}

fn object_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({"type": "object", "description": "The workflow's input."})
}

/// What a finished run returns: the execution and its output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[derive(veoveo_hub_sdk::SingleOutput)]
pub struct RunResult {
    pub workflow: WorkflowName,
    pub execution: ExecutionId,
    /// `n8n://execution/{execution_id}`, where the execution stays readable.
    pub execution_uri: N8nResource,
    /// The JSON of the items the workflow produced: the Veoveo Result node's input, or
    /// the last node's output when the workflow has no Veoveo Result.
    pub output: Vec<serde_json::Value>,
}

/// The arguments of `retry_execution`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[veoveo_hub_sdk::tool(provider = super::N8nProvider, output = ExecutionView, name = "retry_execution")]
pub struct RetryExecutionRequest {
    /// The failed execution to retry; n8n starts a new one from its failed node.
    pub execution: ExecutionId,
}

/// The arguments of `stop_execution`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[veoveo_hub_sdk::tool(provider = super::N8nProvider, output = ExecutionView, name = "stop_execution")]
pub struct StopExecutionRequest {
    /// The running or waiting execution to stop.
    pub execution: ExecutionId,
}

/// Where an n8n execution stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ExecutionStatus {
    New,
    Running,
    Waiting,
    Success,
    Error,
    Crashed,
    Canceled,
    Unknown,
}

impl ExecutionStatus {
    pub const ALL: [Self; 8] = [
        Self::New,
        Self::Running,
        Self::Waiting,
        Self::Success,
        Self::Error,
        Self::Crashed,
        Self::Canceled,
        Self::Unknown,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Running => "running",
            Self::Waiting => "waiting",
            Self::Success => "success",
            Self::Error => "error",
            Self::Crashed => "crashed",
            Self::Canceled => "canceled",
            Self::Unknown => "unknown",
        }
    }

    /// Whether the execution ended: succeeded, failed or stopped.
    pub fn is_final(self) -> bool {
        matches!(
            self,
            Self::Success | Self::Error | Self::Crashed | Self::Canceled
        )
    }
}

impl fmt::Display for ExecutionStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ExecutionStatus {
    type Err = UnknownExecutionStatus;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|status| status.as_str() == value)
            .ok_or(UnknownExecutionStatus)
    }
}

/// A value that is not one of n8n's execution statuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error(
    "an execution status is one of n8n's: new, running, waiting, success, error, crashed, canceled, unknown"
)]
pub struct UnknownExecutionStatus;

/// One n8n execution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[derive(veoveo_hub_sdk::SingleOutput)]
pub struct ExecutionView {
    pub execution: ExecutionId,
    /// `n8n://execution/{execution_id}`.
    pub execution_uri: N8nResource,
    /// The workflow, when the team's file names it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<WorkflowName>,
    pub status: ExecutionStatus,
    /// How it started, such as `webhook`, `trigger` or `retry`, as n8n names it.
    pub mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped_at: Option<String>,
    /// The execution this one retries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_of: Option<ExecutionId>,
    /// Whether the caller reads the execution's data, in a read of the one execution: a
    /// run's to its Task's caller, a webhook's or a schedule's to a writer of the
    /// repository, and to everyone else its status, timing and failure alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<ExecutionAccess>,
    /// The output's items, once it succeeded, with the execution's data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<Vec<serde_json::Value>>,
    /// Each node's inputs and outputs as n8n ran them, with the execution's data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_data: Option<serde_json::Value>,
    /// The node that failed, once it failed, in a read of the one execution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed_node: Option<String>,
    /// n8n's error message, once it failed, in a read of the one execution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// What of an execution its reader reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ExecutionAccess {
    /// Its data: node inputs and outputs and its result.
    Data,
    /// Its status, timing, the failing node's name and the error's message alone.
    Summary,
}

/// The filters of the executions listing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionFilters {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<WorkflowName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<ExecutionStatus>,
}

/// An execution number as a listing key: n8n numbers executions in creation order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ExecutionNumber(pub u64);

impl JsonSchema for ExecutionNumber {
    fn schema_name() -> Cow<'static, str> {
        "ExecutionNumber".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({"type": "integer", "minimum": 0})
    }
}

impl ExecutionNumber {
    pub fn of(execution: &ExecutionId) -> Option<Self> {
        execution.as_str().parse().ok().map(Self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_spell_as_n8n_does() {
        for status in ExecutionStatus::ALL {
            assert_eq!(status.as_str().parse::<ExecutionStatus>(), Ok(status));
            assert_eq!(
                serde_json::to_value(status).unwrap(),
                serde_json::json!(status.as_str())
            );
        }
        assert!("succeeded".parse::<ExecutionStatus>().is_err());
        assert!(ExecutionStatus::Canceled.is_final());
        assert!(!ExecutionStatus::Waiting.is_final());
    }

    #[test]
    fn a_run_takes_a_workflow_and_an_object() {
        let request: RunRequest =
            serde_json::from_value(serde_json::json!({"workflow": "hello-veoveo"})).unwrap();
        assert_eq!(request.input, serde_json::json!({}));
        assert!(
            serde_json::from_value::<RunRequest>(serde_json::json!({"workflow": "Hello"})).is_err()
        );
    }
}
