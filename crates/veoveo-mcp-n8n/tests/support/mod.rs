//! A fake n8n on loopback for the tests that do not run a real one: its public API holds
//! one published workflow and the executions the fake's Veoveo Trigger starts, its
//! webhook listener echoes deliveries, and its editor routes echo what a request carried.
//! It proves the server's side of each exchange, not n8n.
#![allow(dead_code)]

pub mod mesh;

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path, Query, State},
    http::{HeaderMap, Method, StatusCode, Uri, header},
    response::{IntoResponse, Response},
    routing::{any, get, post},
};
use serde_json::{Value, json};
use url::Url;

pub const API_KEY: &str = "test-api-key";
pub const N8N_ID: &str = "Au7bKCOK5h1uXPLz";
pub const VERSION: &str = "0190f3a2-7b1c-4d2e-8f3a-4b5c6d7e8f90";
/// The owner session the fake's login issues: a JWT whose `exp` is in 2033.
pub const OWNER_SESSION: &str =
    "eyJhbGciOiJIUzI1NiJ9.eyJpZCI6Im93bmVyIiwiZXhwIjoyMDAwMDAwMDAwfQ.c2ln";
/// The session n8n renews in an editor answer, which expires later.
pub const RENEWED_SESSION: &str =
    "eyJhbGciOiJIUzI1NiJ9.eyJpZCI6Im93bmVyIiwiZXhwIjoyMDAwMDAwMDAxfQ.c2ln";

/// One execution the fake holds.
#[derive(Clone, Debug)]
pub struct FakeExecution {
    pub status: String,
    pub dispatch: Value,
    pub output: Vec<Value>,
}

#[derive(Default)]
pub struct FakeState {
    pub executions: BTreeMap<u64, FakeExecution>,
    pub dispatches: Vec<(HeaderMap, Value)>,
    pub deliveries: Vec<(Method, String, HeaderMap, Bytes)>,
    pub editor: Vec<(Method, String, HeaderMap)>,
    pub logins: usize,
    pub stops: Vec<u64>,
}

#[derive(Clone)]
pub struct FakeN8n {
    pub base: Url,
    pub state: Arc<Mutex<FakeState>>,
}

impl FakeN8n {
    pub async fn start() -> Self {
        let state = Arc::new(Mutex::new(FakeState::default()));
        let router = Router::new()
            .route("/api/v1/workflows/{id}", get(workflow))
            .route("/api/v1/executions", get(executions))
            .route("/api/v1/executions/{id}", get(execution))
            .route("/api/v1/executions/{id}/stop", post(stop))
            .route("/webhook/{*path}", any(webhook))
            .route("/rest/login", post(login))
            .fallback(editor)
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self {
            base: Url::parse(&format!("http://{address}/")).unwrap(),
            state,
        }
    }

    /// Settle execution `id` with `status` and `output`.
    pub fn settle(&self, id: u64, status: &str, output: Vec<Value>) {
        let mut state = self.state.lock().unwrap();
        let execution = state.executions.get_mut(&id).expect("a started execution");
        execution.status = status.to_owned();
        execution.output = output;
    }

    pub fn state(&self) -> std::sync::MutexGuard<'_, FakeState> {
        self.state.lock().unwrap()
    }
}

fn authorized(headers: &HeaderMap) -> bool {
    headers
        .get("x-n8n-api-key")
        .and_then(|key| key.to_str().ok())
        == Some(API_KEY)
}

async fn workflow(Path(id): Path<String>, headers: HeaderMap) -> (StatusCode, Json<Value>) {
    if !authorized(&headers) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"message": "unauthorized"})),
        );
    }
    if id != N8N_ID {
        return (StatusCode::NOT_FOUND, Json(json!({"message": "Not Found"})));
    }
    (
        StatusCode::OK,
        Json(json!({
            "id": N8N_ID, "name": "Hello Veoveo", "versionId": VERSION,
            "activeVersionId": VERSION, "isArchived": false, "nodes": [], "connections": {}
        })),
    )
}

fn execution_json(id: u64, execution: &FakeExecution, with_data: bool) -> Value {
    let mut value = json!({
        "id": id.to_string(), "workflowId": N8N_ID, "status": execution.status,
        "mode": "webhook", "startedAt": "2026-10-31T10:00:00.000Z", "stoppedAt": null,
        "retryOf": null
    });
    if with_data {
        value["data"] = json!({"resultData": {
            "lastNodeExecuted": "Done",
            "runData": {
                "Veoveo Trigger": [{"data": {"main": [[{"json": {"veoveo": {"operationId": execution.dispatch["operationId"]}}}]]}}],
                "Done": [{"data": {"main": [execution.output.iter().map(|json| json!({"json": json})).collect::<Vec<_>>()]}}]
            },
            "error": if execution.status == "error" { json!({"message": "the step failed"}) } else { Value::Null }
        }});
    }
    value
}

async fn executions(
    State(state): State<Arc<Mutex<FakeState>>>,
    headers: HeaderMap,
    Query(query): Query<BTreeMap<String, String>>,
) -> (StatusCode, Json<Value>) {
    if !authorized(&headers) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"message": "unauthorized"})),
        );
    }
    let state = state.lock().unwrap();
    let limit: usize = query
        .get("limit")
        .and_then(|limit| limit.parse().ok())
        .unwrap_or(100);
    let with_data = query
        .get("includeData")
        .is_some_and(|value| value == "true");
    let status = query.get("status");
    let data: Vec<Value> = state
        .executions
        .iter()
        .rev()
        .filter(|(_, execution)| status.is_none_or(|status| &execution.status == status))
        .take(limit)
        .map(|(id, execution)| execution_json(*id, execution, with_data))
        .collect();
    (
        StatusCode::OK,
        Json(json!({"data": data, "nextCursor": null})),
    )
}

async fn execution(
    State(state): State<Arc<Mutex<FakeState>>>,
    Path(id): Path<u64>,
    headers: HeaderMap,
    Query(query): Query<BTreeMap<String, String>>,
) -> (StatusCode, Json<Value>) {
    if !authorized(&headers) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"message": "unauthorized"})),
        );
    }
    let state = state.lock().unwrap();
    match state.executions.get(&id) {
        Some(execution) => (
            StatusCode::OK,
            Json(execution_json(
                id,
                execution,
                query
                    .get("includeData")
                    .is_some_and(|value| value == "true"),
            )),
        ),
        None => (StatusCode::NOT_FOUND, Json(json!({"message": "Not Found"}))),
    }
}

async fn stop(
    State(state): State<Arc<Mutex<FakeState>>>,
    Path(id): Path<u64>,
    headers: HeaderMap,
) -> (StatusCode, Json<Value>) {
    if !authorized(&headers) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"message": "unauthorized"})),
        );
    }
    let mut state = state.lock().unwrap();
    state.stops.push(id);
    match state.executions.get_mut(&id) {
        Some(execution)
            if !["success", "error", "canceled"].contains(&execution.status.as_str()) =>
        {
            execution.status = "canceled".to_owned();
            (
                StatusCode::OK,
                Json(json!({"status": "canceled", "finished": false})),
            )
        }
        Some(_) => (
            StatusCode::CONFLICT,
            Json(json!({"message": "already finished"})),
        ),
        None => (StatusCode::NOT_FOUND, Json(json!({"message": "Not Found"}))),
    }
}

async fn webhook(
    State(state): State<Arc<Mutex<FakeState>>>,
    method: Method,
    Path(path): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let mut state = state.lock().unwrap();
    if path.ends_with("/run") {
        // The fake's Veoveo Trigger.
        if headers
            .get("x-veoveo-dispatch")
            .and_then(|value| value.to_str().ok())
            != Some("run")
        {
            return StatusCode::NOT_FOUND.into_response();
        }
        let dispatch: Value = serde_json::from_slice(&body).unwrap();
        state.dispatches.push((headers, dispatch.clone()));
        let id = state
            .executions
            .keys()
            .next_back()
            .map_or(1, |last| last + 1);
        state.executions.insert(
            id,
            FakeExecution {
                status: "running".to_owned(),
                dispatch,
                output: Vec::new(),
            },
        );
        return id.to_string().into_response();
    }
    state
        .deliveries
        .push((method, path.clone(), headers, body.clone()));
    (
        StatusCode::OK,
        [(header::SET_COOKIE, "tracking=1")],
        Json(json!({"delivered": path, "bytes": body.len()})),
    )
        .into_response()
}

async fn login(State(state): State<Arc<Mutex<FakeState>>>, body: Bytes) -> Response {
    let login: Value = serde_json::from_slice(&body).unwrap_or_default();
    if login["emailOrLdapLoginId"] != "owner@n8n.veoveo.invalid" {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    state.lock().unwrap().logins += 1;
    (
        StatusCode::OK,
        [(
            header::SET_COOKIE,
            format!(
                "n8n-auth={OWNER_SESSION}; Max-Age=604800; Path=/; HttpOnly; Secure; SameSite=Lax"
            ),
        )],
        Json(json!({"data": {"email": "owner@n8n.veoveo.invalid"}})),
    )
        .into_response()
}

async fn editor(
    State(state): State<Arc<Mutex<FakeState>>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let cookie = headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let authorization = headers.contains_key(header::AUTHORIZATION);
    state
        .lock()
        .unwrap()
        .editor
        .push((method.clone(), uri.to_string(), headers));
    (
        StatusCode::OK,
        [
            (header::X_FRAME_OPTIONS, "SAMEORIGIN".to_owned()),
            (header::SET_COOKIE, format!("n8n-auth={RENEWED_SESSION}; HttpOnly")),
        ],
        Json(json!({"method": method.as_str(), "uri": uri.to_string(), "cookie": cookie, "authorization": authorization})),
    )
        .into_response()
}
