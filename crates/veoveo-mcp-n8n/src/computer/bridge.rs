//! The Computer's bridge for n8n's Veoveo Tool node, on loopback at `/n8n/bridge/calls`,
//! with the runner's request shape. It admits a call with the token of this start, which
//! only n8n's environment holds, and calls the tool through the Hub as the Computer's
//! owner with the Computer's own credential: the Hub decides each call as it decides the
//! owner's. Beside it, `/n8n/bridge/tools` lists the tools the Hub lists to the owner for
//! the node's tool list, admitting the same token as its bearer.
use std::time::Duration;

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use secrecy::{ExposeSecret, SecretString};
use serde_json::json;
use veoveo_hub_sdk::{client::HubClient, hub::HubToolName};

use super::model::bears;
use crate::bridge::{BridgeAnswer, BridgeCall, BridgeTools, ListedTool};

/// The most a bridged call may take, as the runner's bridge allows.
const CALL_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone)]
pub struct ComputerBridge {
    pub token: SecretString,
    pub hub: HubClient,
}

/// The tools the Hub lists to the owner, by Hub name, for the Veoveo Tool node's list:
/// those a call through the bridge can name.
pub async fn tools(State(bridge): State<ComputerBridge>, headers: HeaderMap) -> Response {
    if !bears(&headers, &bridge.token) {
        tracing::warn!(
            class = "n8n.bridge_tools",
            admitted = false,
            "refused a tool list without the Computer's token"
        );
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "the token is not this Computer's n8n's"})),
        )
            .into_response();
    }
    let listed = match bridge.hub.tools().await {
        Ok(listed) => listed,
        Err(error) => {
            tracing::warn!(class = "n8n.bridge_tools", %error, "the Hub could not list the tools");
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": "the Hub could not list your tools"})),
            )
                .into_response();
        }
    };
    let mut tools: Vec<ListedTool> = listed
        .items
        .iter()
        .filter(|tool| {
            HubToolName::parse(&tool.name)
                .ok()
                .is_some_and(|name| name.parts().is_ok())
        })
        .map(|tool| ListedTool {
            name: tool.name.to_string(),
            description: tool.description.as_ref().map(ToString::to_string),
        })
        .collect();
    tools.sort_by(|a, b| a.name.cmp(&b.name));
    Json(BridgeTools { tools }).into_response()
}

/// A call of the Veoveo Tool node.
pub async fn call(State(bridge): State<ComputerBridge>, body: axum::body::Bytes) -> Response {
    let Ok(call) = serde_json::from_slice::<BridgeCall>(&body) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "expected {runToken, tool, arguments}"})),
        )
            .into_response();
    };
    if call.run_token.as_bytes() != bridge.token.expose_secret().as_bytes() {
        tracing::warn!(
            class = "n8n.bridge_call",
            tool = call.tool,
            admitted = false,
            "refused a bridged call without the Computer's token"
        );
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "the token is not this Computer's n8n's"})),
        )
            .into_response();
    }
    // A tool through the Hub is `{server}__{tool}`.
    let Some(name) = HubToolName::parse(&call.tool)
        .ok()
        .filter(|name| name.parts().is_ok())
    else {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": format!("{} is not a tool's name through the Hub", call.tool)})),
        )
            .into_response();
    };
    let answer = match bridge
        .hub
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
    tracing::info!(
        class = "n8n.bridge_call",
        tool = call.tool,
        admitted = true,
        is_error = answer.is_error,
        "a bridged call as the Computer's owner"
    );
    Json(answer).into_response()
}
