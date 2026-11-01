//! The Computer's bridge for n8n's Veoveo Tool node, on loopback at `/n8n/bridge/calls`,
//! with the runner's request shape. It admits a call with the token of this start, which
//! only n8n's environment holds, and calls the tool through the Hub as the Computer's
//! owner with the Computer's own credential: the Hub decides each call as it decides the
//! owner's.
use std::time::Duration;

use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use secrecy::{ExposeSecret, SecretString};
use serde_json::json;
use veoveo_hub_sdk::{client::HubClient, hub::HubToolName};

use crate::bridge::{BridgeAnswer, BridgeCall};

/// The most a bridged call may take, as the runner's bridge allows.
const CALL_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone)]
pub struct ComputerBridge {
    pub token: SecretString,
    pub hub: HubClient,
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
