//! Calls from n8n's Veoveo Tool node into other servers. The bridge admits a call only for
//! a tool in its typed table and only with the token of a running run, and calls through
//! `ServerCalls::for_task` for that run's Task: the Hub exchanges the Task for a token
//! that acts for the run's caller with the permissions the server declared in its `uses`
//! and the caller holds at that moment.
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use veoveo_hub_sdk::{hosting::ServerCalls, hub::HubToolName};
use veoveo_task::TaskRuntime;
use veoveo_types::{PermissionName, TaskId};

use crate::store::RunStore;

/// The most a bridged call may take; a longer one belongs in a Task of the provider.
const CALL_TIMEOUT: Duration = Duration::from_secs(120);

/// The variable of n8n's environment that names the bridge, `{base}/n8n/bridge/calls`;
/// the bridge lists its tools at `{base}/n8n/bridge/tools`.
pub const BRIDGE_URL: &str = "VEOVEO_BRIDGE_URL";
/// The variable of n8n's environment that holds a person's Computer's token for its
/// bridge; a runner sets none, since each run carries its own.
pub const BRIDGE_TOKEN: &str = "VEOVEO_BRIDGE_TOKEN";

/// The tools n8n may call, each with the provider permission it is called with, which the
/// server declares in its `uses`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BridgeTool {
    /// Time's `resolve_time`, which reads the installation's time authority.
    TimeResolveTime,
}

impl BridgeTool {
    pub const ALL: [Self; 1] = [Self::TimeResolveTime];

    /// The tool's name through the Hub, `{server}__{tool}`.
    pub fn hub_name(self) -> &'static str {
        match self {
            Self::TimeResolveTime => "time__resolve_time",
        }
    }

    /// The provider permission the call carries.
    pub fn permission(self) -> PermissionName {
        let name = match self {
            Self::TimeResolveTime => "time.authorities.read",
        };
        PermissionName::parse(name).expect("a bridge table permission")
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|tool| tool.hub_name() == name)
    }
}

/// What the Veoveo Tool node posts.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BridgeCall {
    pub run_token: String,
    pub tool: String,
    #[serde(default)]
    pub arguments: Map<String, Value>,
}

/// What the Veoveo Tool node receives: the tool's structured result, or its error.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeAnswer {
    pub is_error: bool,
    pub result: Value,
}

/// The tools a bridge offers the Veoveo Tool node's list, `GET /n8n/bridge/tools`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BridgeTools {
    pub tools: Vec<ListedTool>,
}

/// One tool of the list: its Hub name, which the node stores, and what it does.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ListedTool {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl BridgeTools {
    /// The bridge table's tools, which a runner's bridge offers.
    pub fn table() -> Self {
        Self {
            tools: BridgeTool::ALL
                .into_iter()
                .map(|tool| ListedTool {
                    name: tool.hub_name().to_owned(),
                    description: None,
                })
                .collect(),
        }
    }
}

/// Why the bridge refused a call before it reached the Hub.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BridgeRefusal {
    #[error("{0} is not a tool n8n may call")]
    UnknownTool(String),
    #[error("the token is not a running run's")]
    NotARun,
    #[error("the run's Task has ended")]
    RunEnded,
    #[error("this server makes no calls to other servers")]
    NoCalls,
}

/// The bridge of the runner's one tenant.
#[derive(Clone)]
pub struct Bridge {
    runtime: TaskRuntime,
    runs: RunStore,
    tenant: veoveo_store::TenantId,
    calls: Option<ServerCalls>,
}

impl Bridge {
    pub fn new(
        runtime: TaskRuntime,
        runs: RunStore,
        tenant: veoveo_store::TenantId,
        calls: Option<ServerCalls>,
    ) -> Self {
        Self {
            runtime,
            runs,
            tenant,
            calls,
        }
    }

    /// Admit `call` and make it for its run's caller. A refusal names its reason; an
    /// answer carries the provider's result, an error result included.
    pub async fn call(
        &self,
        call: BridgeCall,
    ) -> Result<Result<BridgeAnswer, BridgeRefusal>, anyhow::Error> {
        let Some(tool) = BridgeTool::parse(&call.tool) else {
            return Ok(Err(BridgeRefusal::UnknownTool(call.tool)));
        };
        let Some(run) = self.runs.by_token(self.tenant, &call.run_token).await? else {
            return Ok(Err(BridgeRefusal::NotARun));
        };
        let task_id = TaskId::from_uuid(run.task_id);
        let snapshot = match self.runtime.get(task_id).await? {
            Some(snapshot) if !snapshot.is_terminal() => snapshot,
            _ => return Ok(Err(BridgeRefusal::RunEnded)),
        };
        let Some(calls) = &self.calls else {
            return Ok(Err(BridgeRefusal::NoCalls));
        };
        let client = calls.for_task(&snapshot)?;
        let name = HubToolName::parse(tool.hub_name())?;
        let answer = match client
            .call_tool(name, call.arguments)
            .timeout(CALL_TIMEOUT)
            .await
        {
            Ok(result) => BridgeAnswer {
                is_error: result.is_error == Some(true),
                result: result
                    .structured_content
                    .clone()
                    .unwrap_or_else(|| json!({"content": result.content})),
            },
            Err(error) => BridgeAnswer {
                is_error: true,
                result: json!({"error": error.to_string()}),
            },
        };
        Ok(Ok(answer))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_names_hub_tools_and_their_permissions() {
        for tool in BridgeTool::ALL {
            let name = HubToolName::parse(tool.hub_name()).unwrap();
            let (server, _) = name.parts().unwrap();
            assert!(tool.permission().is_owned_by(&server), "{tool:?}");
            assert_eq!(BridgeTool::parse(tool.hub_name()), Some(tool));
        }
        assert_eq!(BridgeTool::parse("time__create_event"), None);
        let call: BridgeCall = serde_json::from_value(json!({
            "runToken": "t", "tool": "time__resolve_time", "arguments": {"expression": "now"}
        }))
        .unwrap();
        assert_eq!(call.arguments["expression"], "now");
        assert_eq!(
            serde_json::to_value(BridgeTools::table()).unwrap(),
            json!({"tools": [{"name": "time__resolve_time"}]})
        );
    }
}
