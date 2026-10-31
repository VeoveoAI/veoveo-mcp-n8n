//! n8n's executions and workflows as runs need them: the Veoveo Trigger on n8n's webhook
//! listener, the public API's executions, and a workflow's whole document.
use base64::Engine;
use reqwest::{Method, header};
use serde::{Deserialize, Deserializer, de};
use serde_json::{Value, json};

use super::{
    N8nClient, N8nError, N8nWorkflowId,
    client::{parse, refuse_failure},
};
use crate::contract::{ExecutionId, ExecutionStatus};

/// The node types of the Veoveo nodes, which n8n loads from `N8N_CUSTOM_EXTENSIONS` under
/// its custom package name.
pub const VEOVEO_TRIGGER: &str = "CUSTOM.veoveoTrigger";
pub const VEOVEO_RESULT: &str = "CUSTOM.veoveoResult";
pub const VEOVEO_TOOL: &str = "CUSTOM.veoveoTool";
/// The header that marks the server's own call of a Veoveo Trigger. The hooks never
/// forward a header of the server's, so only the server starts a run.
pub const DISPATCH_HEADER: &str = "x-veoveo-dispatch";

/// An execution as n8n's public API returns it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct N8nExecution {
    #[serde(deserialize_with = "execution_id")]
    pub id: ExecutionId,
    #[serde(default)]
    pub workflow_id: Option<String>,
    #[serde(deserialize_with = "status")]
    pub status: ExecutionStatus,
    pub mode: String,
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub stopped_at: Option<String>,
    #[serde(default, deserialize_with = "optional_execution_id")]
    pub retry_of: Option<ExecutionId>,
    /// The run data, when the read asked for it.
    #[serde(default)]
    pub data: Option<Value>,
    #[serde(default)]
    pub workflow_data: Option<Value>,
}

impl N8nExecution {
    /// The JSON of the items the execution produced: the Veoveo Result node's, or the last
    /// node's when no Veoveo Result ran.
    pub fn output(&self) -> Option<Vec<Value>> {
        let result = self.data.as_ref()?.get("resultData")?;
        let runs = result.get("runData")?.as_object()?;
        let result_node = self
            .workflow_data
            .as_ref()
            .and_then(|workflow| workflow.get("nodes"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find(|node| node.get("type").and_then(Value::as_str) == Some(VEOVEO_RESULT))
            .and_then(|node| node.get("name"))
            .and_then(Value::as_str)
            .filter(|name| runs.contains_key(*name));
        let node =
            result_node.or_else(|| result.get("lastNodeExecuted").and_then(Value::as_str))?;
        let items = runs
            .get(node)?
            .as_array()?
            .last()?
            .get("data")?
            .get("main")?
            .get(0)?
            .as_array()?;
        Some(
            items
                .iter()
                .map(|item| item.get("json").cloned().unwrap_or(Value::Null))
                .collect(),
        )
    }

    /// n8n's error message, when the execution failed.
    pub fn error(&self) -> Option<String> {
        let error = self.failure()?;
        error
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| Some(error.to_string()))
    }

    /// Each node's runs, with their inputs and outputs, as n8n records them.
    pub fn run_data(&self) -> Option<Value> {
        self.data
            .as_ref()?
            .get("resultData")?
            .get("runData")
            .cloned()
    }

    /// The name of the node that failed, once the execution failed: the node n8n's error
    /// names, or the last one it ran.
    pub fn failed_node(&self) -> Option<String> {
        let result = self.data.as_ref()?.get("resultData")?;
        self.failure()?
            .get("node")
            .and_then(|node| node.get("name"))
            .or_else(|| result.get("lastNodeExecuted"))
            .and_then(Value::as_str)
            .map(str::to_owned)
    }

    /// The error n8n recorded, once the execution failed; a null one is none.
    fn failure(&self) -> Option<&Value> {
        self.data
            .as_ref()?
            .get("resultData")?
            .get("error")
            .filter(|error| !error.is_null())
    }

    /// The operation the Veoveo Trigger started this execution for, from its output.
    pub fn operation(&self) -> Option<&str> {
        let runs = self
            .data
            .as_ref()?
            .get("resultData")?
            .get("runData")?
            .as_object()?;
        runs.values()
            .filter_map(Value::as_array)
            .filter_map(|runs| runs.first())
            .filter_map(|run| run.get("data")?.get("main")?.get(0)?.get(0))
            .find_map(|item| {
                item.get("json")?
                    .get("veoveo")?
                    .get("operationId")?
                    .as_str()
            })
    }
}

fn execution_id<'de, D: Deserializer<'de>>(deserializer: D) -> Result<ExecutionId, D::Error> {
    let value = Value::deserialize(deserializer)?;
    id_from(&value).ok_or_else(|| de::Error::custom("an execution ID is a decimal number"))
}

fn optional_execution_id<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<ExecutionId>, D::Error> {
    let value = Value::deserialize(deserializer)?;
    if value.is_null() {
        return Ok(None);
    }
    id_from(&value)
        .map(Some)
        .ok_or_else(|| de::Error::custom("an execution ID is a decimal number"))
}

fn id_from(value: &Value) -> Option<ExecutionId> {
    match value {
        Value::String(text) => ExecutionId::parse(text.clone()).ok(),
        Value::Number(number) => ExecutionId::parse(number.as_u64()?.to_string()).ok(),
        _ => None,
    }
}

fn status<'de, D: Deserializer<'de>>(deserializer: D) -> Result<ExecutionStatus, D::Error> {
    let value = String::deserialize(deserializer)?;
    value.parse().map_err(de::Error::custom)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExecutionPage {
    data: Vec<N8nExecution>,
    next_cursor: Option<String>,
}

/// A page of n8n's executions, newest first, and whether n8n holds more after it.
pub struct ExecutionsPage {
    pub executions: Vec<N8nExecution>,
    pub more: bool,
}

impl N8nClient {
    /// Call the Veoveo Trigger at `path` under n8n's production webhooks with `body`. The
    /// trigger answers at once with the execution it started.
    pub async fn dispatch(&self, path: &str, body: &Value) -> Result<ExecutionId, N8nError> {
        let target = format!("webhook/{path}");
        let response = self
            .http
            .post(self.url(&target))
            .header(header::CONTENT_TYPE, "application/json")
            .header(DISPATCH_HEADER, "run")
            .body(body.to_string())
            .send()
            .await?;
        let response = refuse_failure(Method::POST, &target, response).await?;
        let text = response.text().await?;
        let id = text.trim().trim_matches('"');
        ExecutionId::parse(id).map_err(|_| N8nError::Unexpected {
            path: target,
            reason: format!(
                "the Veoveo Trigger answered {:?}, not an execution ID",
                id.chars().take(64).collect::<String>()
            ),
        })
    }

    /// Execution `id`, with its run data when `with_data`; `None` when n8n has none.
    pub async fn execution(
        &self,
        id: &ExecutionId,
        with_data: bool,
    ) -> Result<Option<N8nExecution>, N8nError> {
        let path = format!("api/v1/executions/{id}?includeData={with_data}");
        match self.call(Method::GET, &path, None).await {
            Ok(execution) => Ok(Some(execution)),
            Err(error) if error.is_not_found() => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// A page of at most `limit` executions, newest first, of `workflow` when given, with
    /// `status` when given, and after execution `after` when given.
    pub async fn executions(
        &self,
        workflow: Option<&N8nWorkflowId>,
        status: Option<ExecutionStatus>,
        limit: u32,
        after: Option<u64>,
        with_data: bool,
    ) -> Result<ExecutionsPage, N8nError> {
        let mut path = format!("api/v1/executions?limit={limit}&includeData={with_data}");
        if let Some(workflow) = workflow {
            path.push_str(&format!("&workflowId={workflow}"));
        }
        if let Some(status) = status {
            path.push_str(&format!("&status={status}"));
        }
        if let Some(after) = after {
            // n8n's cursor names the last execution of the page before and the limit.
            let cursor = base64::engine::general_purpose::STANDARD
                .encode(json!({"lastId": after.to_string(), "limit": limit}).to_string());
            path.push_str("&cursor=");
            path.push_str(
                &url::form_urlencoded::byte_serialize(cursor.as_bytes()).collect::<String>(),
            );
        }
        let page: ExecutionPage = self.call(Method::GET, &path, None).await?;
        Ok(ExecutionsPage {
            more: page.next_cursor.is_some(),
            executions: page.data,
        })
    }

    /// Stop execution `id`; n8n answers with where it stopped.
    pub async fn stop_execution(&self, id: &ExecutionId) -> Result<Value, N8nError> {
        self.call(
            Method::POST,
            &format!("api/v1/executions/{id}/stop"),
            Some(&json!({})),
        )
        .await
    }

    /// Retry execution `id`, which n8n runs as a new execution.
    pub async fn retry_execution(&self, id: &ExecutionId) -> Result<ExecutionId, N8nError> {
        #[derive(Deserialize)]
        struct Retried {
            #[serde(deserialize_with = "execution_id")]
            id: ExecutionId,
        }
        let retried: Retried = self
            .call(
                Method::POST,
                &format!("api/v1/executions/{id}/retry"),
                Some(&json!({"loadWorkflow": false})),
            )
            .await?;
        Ok(retried.id)
    }

    /// Workflow `id`'s whole document as n8n holds it now.
    pub async fn workflow_document(&self, id: &N8nWorkflowId) -> Result<Option<Value>, N8nError> {
        let path = format!("api/v1/workflows/{id}?excludePinnedData=false");
        let response = self.api(Method::GET, &path, None).await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        parse(&path, refuse_failure(Method::GET, &path, response).await?)
            .await
            .map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn execution(value: Value) -> N8nExecution {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn an_execution_reads_its_output_error_and_operation() {
        let succeeded = execution(json!({
            "id": "7", "workflowId": "Wf1", "status": "success", "mode": "webhook",
            "startedAt": "2026-10-31T10:00:00.000Z", "stoppedAt": "2026-10-31T10:00:01.000Z",
            "retryOf": null,
            "workflowData": {"nodes": [
                {"name": "Start", "type": VEOVEO_TRIGGER},
                {"name": "Done", "type": VEOVEO_RESULT},
                {"name": "After", "type": "n8n-nodes-base.noOp"}
            ]},
            "data": {"resultData": {
                "lastNodeExecuted": "After",
                "runData": {
                    "Start": [{"data": {"main": [[{"json": {"veoveo": {"operationId": "op-1"}, "input": {}}}]]}}],
                    "Done": [{"data": {"main": [[{"json": {"greeting": "hi"}}, {"json": {"n": 2}}]]}}],
                    "After": [{"data": {"main": [[{"json": {"ignored": true}}]]}}]
                }
            }}
        }));
        assert_eq!(succeeded.id.as_str(), "7");
        assert_eq!(succeeded.status, ExecutionStatus::Success);
        assert_eq!(
            succeeded.output(),
            Some(vec![json!({"greeting": "hi"}), json!({"n": 2})])
        );
        assert_eq!(succeeded.operation(), Some("op-1"));
        assert_eq!(succeeded.error(), None);

        let failed = execution(json!({
            "id": 8, "status": "error", "mode": "retry", "retryOf": "7",
            "data": {"resultData": {"error": {"message": "the API refused"}, "runData": {}}}
        }));
        assert_eq!(failed.id.as_str(), "8");
        assert_eq!(failed.retry_of.as_ref().map(ExecutionId::as_str), Some("7"));
        assert_eq!(failed.error().as_deref(), Some("the API refused"));
        assert!(
            serde_json::from_value::<N8nExecution>(
                json!({"id": "x", "status": "success", "mode": "m"})
            )
            .is_err()
        );
    }
}
