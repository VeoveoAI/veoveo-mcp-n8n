//! The server's loopback listener, which only processes in the sandbox reach: the Veoveo
//! Result node posts a run's output to its operation's callback URL here, and the Veoveo
//! Tool node lists the bridge's tools and calls other servers through the bridge here.
use std::{net::SocketAddr, sync::Arc};

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use url::Url;
use veoveo_task::{
    CallbackToken, Delivery, EventTarget, OperationEvent, OperationEventId, TaskRuntime,
};

use crate::{
    bridge::{Bridge, BridgeCall, BridgeRefusal, BridgeTools},
    contract::ExecutionId,
    runs::RunOutcome,
};

/// The loopback listener's port.
pub const PORT: u16 = 8812;
/// The largest body a Veoveo node posts: a run's output or a tool's arguments.
const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;

/// The base URL of the loopback listener.
pub fn base() -> Url {
    Url::parse(&format!("http://127.0.0.1:{PORT}/")).expect("the loopback URL")
}

/// Where the Veoveo Tool node calls the bridge under `base`.
pub fn bridge_url(base: &Url) -> Url {
    base.join(&format!("{}/bridge/calls", crate::server().as_str()))
        .expect("the bridge path")
}

#[derive(Clone)]
struct Loopback {
    runtime: TaskRuntime,
    bridge: Arc<Bridge>,
}

/// The loopback routes: `/n8n/operations/{operation_id}/{token}`, `/n8n/bridge/calls` and
/// `/n8n/bridge/tools`.
pub fn router(runtime: TaskRuntime, bridge: Bridge) -> Router {
    let mount = format!("/{}", crate::server().as_str());
    Router::new()
        .route(
            &format!("{mount}/operations/{{operation_id}}/{{token}}"),
            post(result),
        )
        .route(&format!("{mount}/bridge/calls"), post(bridged))
        .route(&format!("{mount}/bridge/tools"), get(listed))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(Loopback {
            runtime,
            bridge: Arc::new(bridge),
        })
}

/// Serve `router` on the loopback listener until `shutdown` completes.
pub async fn serve(
    router: Router,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], PORT))).await?;
    tracing::info!(port = PORT, "serving the Veoveo nodes on loopback");
    serve_on(listener, router, shutdown).await
}

/// Serve `router` on `listener` until `shutdown` completes.
pub async fn serve_on(
    listener: tokio::net::TcpListener,
    router: Router,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown)
        .await?;
    Ok(())
}

/// What the Veoveo Result node posts.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResultPost {
    execution_id: ExecutionId,
    #[serde(default)]
    output: Vec<Value>,
}

/// A run's output from its Veoveo Result node: the final event of its operation.
async fn result(
    State(loopback): State<Loopback>,
    Path((operation_id, token)): Path<(String, String)>,
    body: axum::body::Bytes,
) -> Response {
    let (Ok(operation_id), Some(token)) = (operation_id.parse(), CallbackToken::parse(&token))
    else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Ok(post) = serde_json::from_slice::<ResultPost>(&body) else {
        return (StatusCode::BAD_REQUEST, "expected {executionId, output}").into_response();
    };
    // The event is the operation's, which no other run shares; n8n's execution number
    // alone would repeat once n8n's data starts again.
    let Ok(event_id) = OperationEventId::parse(format!("result-{operation_id}")) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let payload = match serde_json::to_value(RunOutcome::Succeeded {
        execution: post.execution_id,
        output: post.output,
    }) {
        Ok(payload) => payload,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let event = OperationEvent {
        target: EventTarget::Callback {
            operation_id,
            token,
        },
        event_id,
        payload,
        is_final: true,
    };
    match loopback.runtime.deliver(&event).await {
        Ok(Delivery::Unknown) => StatusCode::NOT_FOUND.into_response(),
        Ok(delivery) => {
            tracing::info!(?delivery, "a run's Veoveo Result arrived");
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => {
            tracing::error!(%error, "storing a run's result failed");
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
    }
}

/// The bridge table's tools, for the Veoveo Tool node's list. The table is the server's
/// own declaration, so any caller on loopback may read it.
async fn listed() -> Json<BridgeTools> {
    Json(BridgeTools::table())
}

/// A call of the Veoveo Tool node through the bridge.
async fn bridged(State(loopback): State<Loopback>, body: axum::body::Bytes) -> Response {
    let Ok(call) = serde_json::from_slice::<BridgeCall>(&body) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "expected {runToken, tool, arguments}"})),
        )
            .into_response();
    };
    let tool = call.tool.clone();
    match loopback.bridge.call(call).await {
        Ok(Ok(answer)) => {
            tracing::info!(
                class = "n8n.bridge_call", %tool, admitted = true, is_error = answer.is_error,
                "a bridged call"
            );
            Json(answer).into_response()
        }
        Ok(Err(refusal)) => {
            tracing::warn!(
                class = "n8n.bridge_call", %tool, admitted = false, reason = %refusal,
                "refused a bridged call"
            );
            let status = match refusal {
                BridgeRefusal::NoCalls => StatusCode::SERVICE_UNAVAILABLE,
                _ => StatusCode::FORBIDDEN,
            };
            (status, Json(json!({"error": refusal.to_string()}))).into_response()
        }
        Err(error) => {
            tracing::error!(class = "n8n.bridge_call", %tool, %error, "a bridged call failed");
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": "the call failed"})),
            )
                .into_response()
        }
    }
}
