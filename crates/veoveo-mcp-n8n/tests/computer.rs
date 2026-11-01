//! `veoveo-n8n-computer`'s routes and team checkouts against a fake n8n, a fake Hub MCP
//! endpoint and a fake Lore command line: `open` clones a team's repository at its runner's
//! tracked branch once and applies its workflows as tagged drafts, then redirects to the
//! workflow; a change saved in n8n and a new workflow under the team's tag are written back
//! to the team's files in the server's file form; a team whose runner names no tracked
//! branch, or a repository no team serves, opens nothing; `teams` lists the teams; the
//! editor reaches n8n without the mount as its owner, and its answers set no cookie; the
//! bridge admits only the Computer's token and calls the Hub with the Computer's own
//! credential. Every value is synthetic fixture data; nothing here runs n8n or Lore.
use std::{
    collections::BTreeMap,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use axum::{
    Json, Router,
    body::Body,
    extract::{Path as UrlPath, Query, State},
    http::{HeaderMap, Method, Request, StatusCode, Uri, header},
    response::{IntoResponse, Response},
    routing::{any, post, put},
};
use rmcp::{
    ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ErrorData,
        ListResourcesResult, PaginatedRequestParams, ReadResourceRequestParams,
        ReadResourceResponse, ReadResourceResult, Resource, ResourceContents, ServerCapabilities,
        ServerConfig,
    },
    service::{RequestContext, RoleServer},
    transport::streamable_http_server::StreamableHttpService,
};
use secrecy::SecretString;
use serde_json::{Value, json};
use tower::ServiceExt;
use url::Url;
use veoveo_hub_sdk::client::{HubClient, StaticBearer};
use veoveo_mcp_n8n::{
    computer::{Checkouts, ComputerBridge, Editor, EditorSession, router},
    n8n::N8nClient,
};

const API_KEY: &str = "computer-api-key";
const PLACEHOLDER: &str = "computer-hub-reference-placeholder";
const MARKETING: &str = "0123456789abcdef0123456789abcdef";
const SALES: &str = "fedcba9876543210fedcba9876543210";
const TRACKED: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const PATH: &str = "/computer/c/019a5b30-0000-7000-8000-000000000001/apps/n8n/";
const SESSION: &str = "eyJhbGciOiJIUzI1NiJ9.eyJpZCI6Im93bmVyIiwiZXhwIjoyMDAwMDAwMDAwfQ.c2ln";
/// The session n8n renews in an editor answer, which expires later.
const RENEWED: &str = "eyJhbGciOiJIUzI1NiJ9.eyJpZCI6Im93bmVyIiwiZXhwIjoyMDAwMDAwMDAxfQ.c2ln";
const TOKEN: &str = "computer-bridge-token";

// ---------------------------------------------------------------- fake n8n --

#[derive(Default)]
struct N8nState {
    /// Each workflow's whole document, by ID.
    workflows: BTreeMap<String, Value>,
    tags: Vec<Value>,
    /// What reached the editor's fallback: method, path and query, cookie and origin.
    editor: Vec<(Method, String, String, String)>,
    publishes: usize,
    next: u64,
}

type Shared = Arc<Mutex<N8nState>>;

fn authorized(headers: &HeaderMap) -> bool {
    headers
        .get("x-n8n-api-key")
        .and_then(|key| key.to_str().ok())
        == Some(API_KEY)
}

fn summary(document: &Value) -> Value {
    json!({
        "id": document["id"], "name": document["name"], "versionId": document["versionId"],
        "activeVersionId": document["activeVersionId"], "isArchived": document["isArchived"]
    })
}

async fn workflow(
    State(state): State<Shared>,
    UrlPath(id): UrlPath<String>,
    method: Method,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    assert!(authorized(&headers));
    let mut state = state.lock().unwrap();
    if method == Method::PUT {
        let body: Value = serde_json::from_slice(&body).unwrap();
        state.next += 1;
        let version = uuid::Uuid::from_u128(u128::from(state.next)).to_string();
        let Some(document) = state.workflows.get_mut(&id) else {
            return StatusCode::NOT_FOUND.into_response();
        };
        for (member, value) in body.as_object().unwrap() {
            document[member] = value.clone();
        }
        document["versionId"] = json!(version);
        return Json(summary(document)).into_response();
    }
    match state.workflows.get(&id) {
        Some(document) => Json(document.clone()).into_response(),
        None => (StatusCode::NOT_FOUND, Json(json!({"message": "Not Found"}))).into_response(),
    }
}

async fn workflows(
    State(state): State<Shared>,
    method: Method,
    headers: HeaderMap,
    Query(query): Query<BTreeMap<String, String>>,
    body: axum::body::Bytes,
) -> Response {
    assert!(authorized(&headers));
    let mut state = state.lock().unwrap();
    if method == Method::POST {
        let mut document: Value = serde_json::from_slice(&body).unwrap();
        state.next += 1;
        let id = format!("wf{:014}", state.next);
        document["id"] = json!(id);
        document["versionId"] = json!(uuid::Uuid::from_u128(u128::from(state.next)).to_string());
        document["activeVersionId"] = Value::Null;
        document["isArchived"] = json!(false);
        document["tags"] = json!([]);
        state.workflows.insert(id, document.clone());
        return Json(summary(&document)).into_response();
    }
    let tag = query.get("tags").cloned().unwrap_or_default();
    let data: Vec<Value> = state
        .workflows
        .values()
        .filter(|document| {
            document["tags"]
                .as_array()
                .unwrap()
                .iter()
                .any(|carried| carried["name"] == tag.as_str())
        })
        .map(summary)
        .collect();
    Json(json!({"data": data, "nextCursor": null})).into_response()
}

async fn publish(State(state): State<Shared>) -> StatusCode {
    state.lock().unwrap().publishes += 1;
    StatusCode::INTERNAL_SERVER_ERROR
}

async fn tags(
    State(state): State<Shared>,
    method: Method,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    assert!(authorized(&headers));
    let mut state = state.lock().unwrap();
    if method == Method::POST {
        let body: Value = serde_json::from_slice(&body).unwrap();
        let tag = json!({"id": format!("tag{}", state.tags.len() + 1), "name": body["name"]});
        state.tags.push(tag.clone());
        return Json(tag).into_response();
    }
    Json(json!({"data": state.tags, "nextCursor": null})).into_response()
}

async fn workflow_tags(
    State(state): State<Shared>,
    UrlPath(id): UrlPath<String>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    assert!(authorized(&headers));
    let ids: Vec<Value> = serde_json::from_slice(&body).unwrap();
    let mut state = state.lock().unwrap();
    let tags: Vec<Value> = state
        .tags
        .iter()
        .filter(|tag| ids.iter().any(|named| named["id"] == tag["id"]))
        .cloned()
        .collect();
    state.workflows.get_mut(&id).unwrap()["tags"] = json!(tags);
    Json(json!(tags)).into_response()
}

async fn login() -> Response {
    (
        [(
            header::SET_COOKIE,
            format!("n8n-auth={SESSION}; Path=/; HttpOnly"),
        )],
        Json(json!({"data": {}})),
    )
        .into_response()
}

async fn editor(
    State(state): State<Shared>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let text = |name: header::HeaderName| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned()
    };
    state.lock().unwrap().editor.push((
        method,
        uri.to_string(),
        text(header::COOKIE),
        text(header::ORIGIN),
    ));
    (
        [
            (header::SET_COOKIE, format!("n8n-auth={RENEWED}; HttpOnly")),
            (header::X_FRAME_OPTIONS, "SAMEORIGIN".to_owned()),
        ],
        "<html>editor</html>",
    )
        .into_response()
}

async fn fake_n8n() -> (Url, Shared) {
    let state = Shared::default();
    let router = Router::new()
        .route("/api/v1/workflows", any(workflows))
        .route("/api/v1/workflows/{id}", any(workflow))
        .route("/api/v1/workflows/{id}/publish", post(publish))
        .route("/api/v1/workflows/{id}/tags", put(workflow_tags))
        .route("/api/v1/tags", any(tags))
        .route("/rest/login", post(login))
        .fallback(editor)
        .with_state(state.clone());
    (serve(router).await, state)
}

async fn serve(router: Router) -> Url {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    base
}

// ----------------------------------------------------------------- fake Hub --

/// The Hub's MCP endpoint as the Computer's owner sees it: two runners of the n8n
/// repository, `marketing`, which names its tracked branch, and `sales`, which does not,
/// beside another server's resource; and one tool.
#[derive(Clone, Default)]
struct FakeHub {
    calls: Arc<Mutex<Vec<CallToolRequestParams>>>,
}

fn status(repository: &str, tracked: Option<&str>) -> Value {
    let mut status = json!({
        "n8nVersion": "2.46.0", "serverVersion": "0.1.0", "startedAt": "2026-10-31T10:00:00Z",
        "workflows": [], "hooksUrl": "https://veoveo.example/instances/x/n8n/hooks/",
        "repository": repository, "writer": true,
        "editUrl": format!("https://veoveo.example/computer/apps/n8n/open?repository={repository}")
    });
    if let Some(tracked) = tracked {
        status["trackedBranch"] = json!(tracked);
    }
    status
}

impl ServerHandler for FakeHub {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
    }
    async fn list_resources(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        let resource = |uri: &str| Resource::new(uri, "status");
        Ok(ListResourcesResult::with_all_items(vec![
            resource("n8n://marketing/status"),
            resource("n8n://marketing/workflows"),
            resource("n8n://sales/status"),
            resource("time://clock/current"),
        ]))
    }
    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        let body = match request.uri.as_str() {
            "n8n://marketing/status" => status(MARKETING, Some(TRACKED)),
            "n8n://sales/status" => status(SALES, None),
            other => return Err(ErrorData::invalid_params(format!("no {other}"), None)),
        };
        Ok(ReadResourceResult::new(vec![
            ResourceContents::text(body.to_string(), request.uri)
                .with_mime_type("application/json"),
        ])
        .into())
    }
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        self.calls.lock().unwrap().push(request.clone());
        let mut result = CallToolResult::success(vec![ContentBlock::text("resolved")]);
        result.structured_content =
            Some(json!({"tool": request.name, "arguments": request.arguments}));
        Ok(result.into())
    }
}

async fn fake_hub() -> (Url, FakeHub) {
    let hub = FakeHub::default();
    let served = hub.clone();
    let service = StreamableHttpService::new(
        move || Ok(served.clone()),
        veoveo_hub_sdk::stateless_session_manager(),
        veoveo_hub_sdk::canonical_streamable_http_server_config(),
    );
    let router =
        Router::new()
            .nest_service("/mcp/computer", service)
            .layer(axum::middleware::from_fn(
                |request: axum::extract::Request, next: axum::middleware::Next| async move {
                    // The proxy replaces this placeholder with the owner's token.
                    assert_eq!(
                        request.headers().get(header::AUTHORIZATION).unwrap(),
                        format!("Bearer {PLACEHOLDER}").as_str()
                    );
                    next.run(request).await
                },
            ));
    let base = serve(router).await;
    (base.join("mcp/computer").unwrap(), hub)
}

// ---------------------------------------------------------------- fake Lore --

/// A Lore command line that records its arguments and, for a clone, makes the checkout
/// with the team template's workflow in it.
fn fake_lore(directory: &Path) -> (PathBuf, PathBuf) {
    let program = directory.join("lore");
    let log = directory.join("lore.log");
    let template = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../templates/team/workflows/hello-run.json")
        .canonicalize()
        .unwrap();
    std::fs::write(
        &program,
        format!(
            "#!/bin/sh\necho \"$@\" >> '{}'\n[ \"$1\" = clone ] || exit 2\nmkdir -p \"$3/.lore\" \"$3/workflows\"\ncp '{}' \"$3/workflows/hello-run.json\"\n",
            log.display(),
            template.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    (program, log)
}

// ------------------------------------------------------------------- tests --

struct Fixture {
    routes: Router,
    checkouts: Arc<Checkouts>,
    n8n: Shared,
    hub: FakeHub,
    home: PathBuf,
    lore_log: PathBuf,
    _directory: tempfile::TempDir,
}

async fn fixture() -> Fixture {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let directory = tempfile::tempdir().unwrap();
    let home = directory.path().join("home");
    let (lore, lore_log) = fake_lore(directory.path());
    let (n8n_base, n8n) = fake_n8n().await;
    let (hub_url, hub) = fake_hub().await;
    let client = N8nClient::new(n8n_base.clone())
        .unwrap()
        .with_api_key(SecretString::from(API_KEY));
    let checkouts = Arc::new(Checkouts::new(
        home.join("n8n"),
        home.join(".veoveo/n8n/teams"),
        lore,
        "grpcs://lore.example:41337".to_owned(),
        client,
    ));
    let hub_client = HubClient::new(
        hub_url,
        reqwest::Client::builder().no_proxy().build().unwrap(),
        StaticBearer::new(PLACEHOLDER),
    );
    let routes = router(
        Editor {
            http: reqwest::Client::builder().no_proxy().build().unwrap(),
            n8n: n8n_base.clone(),
            session: Arc::new(EditorSession::new(
                N8nClient::new(n8n_base).unwrap(),
                SecretString::from("owner-password"),
            )),
            hub: hub_client.clone(),
            checkouts: checkouts.clone(),
            path: PATH.to_owned(),
        },
        ComputerBridge {
            token: SecretString::from(TOKEN),
            hub: hub_client,
        },
    );
    Fixture {
        routes,
        checkouts,
        n8n,
        hub,
        home,
        lore_log,
        _directory: directory,
    }
}

impl Fixture {
    async fn send(&self, request: Request<Body>) -> (StatusCode, HeaderMap, String) {
        let response = self.routes.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let body = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        (status, headers, String::from_utf8_lossy(&body).into_owned())
    }

    async fn get(&self, path: &str) -> (StatusCode, HeaderMap, String) {
        self.send(Request::get(path).body(Body::empty()).unwrap())
            .await
    }

    fn file(&self, name: &str) -> Value {
        serde_json::from_slice(
            &std::fs::read(self.home.join("n8n/marketing/workflows").join(name)).unwrap(),
        )
        .unwrap()
    }

    fn workflow_named(&self, name: &str) -> Value {
        self.n8n
            .lock()
            .unwrap()
            .workflows
            .values()
            .find(|document| document["name"] == name)
            .cloned()
            .unwrap_or_else(|| panic!("no workflow {name}"))
    }
}

#[tokio::test]
async fn open_clones_a_team_once_applies_drafts_and_writes_back_what_is_saved() {
    let fixture = fixture().await;
    let open = format!("/apps/n8n/open?repository={MARKETING}&workflow=hello-run");
    let (status, headers, body) = fixture.get(&open).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    let applied = fixture.workflow_named("Hello Run");
    let id = applied["id"].as_str().unwrap().to_owned();
    assert_eq!(headers[header::LOCATION], format!("{PATH}workflow/{id}"));
    // Cloned at the tracked branch, as the owner; applied as a draft under the team's tag.
    assert_eq!(
        std::fs::read_to_string(&fixture.lore_log).unwrap(),
        format!(
            "clone grpcs://lore.example:41337/{MARKETING} {} --branch {TRACKED} --non-interactive\n",
            fixture.home.join("n8n/marketing").display()
        )
    );
    assert_eq!(
        applied["tags"],
        json!([{"id": "tag1", "name": "veoveo:marketing"}])
    );
    assert_eq!(applied["activeVersionId"], Value::Null);
    assert_eq!(fixture.n8n.lock().unwrap().publishes, 0);

    // Opening again clones nothing and keeps the unchanged draft.
    let version = applied["versionId"].clone();
    let (status, _, _) = fixture.get(&open).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(
        std::fs::read_to_string(&fixture.lore_log)
            .unwrap()
            .lines()
            .count(),
        1
    );
    assert_eq!(fixture.workflow_named("Hello Run")["versionId"], version);

    // A change saved in the editor is written back in the file form.
    {
        let mut state = fixture.n8n.lock().unwrap();
        let document = state.workflows.get_mut(&id).unwrap();
        document["nodes"][1]["name"] = json!("Edited Greeting");
        document["versionId"] = json!(uuid::Uuid::from_u128(999).to_string());
        state.next += 1;
        let created = json!({
            "id": "wfnew00000000001", "name": "Invoice Intake (v2)", "nodes": [], "connections": {},
            "settings": {}, "versionId": uuid::Uuid::from_u128(1000).to_string(),
            "activeVersionId": null, "isArchived": false,
            "tags": [{"id": "tag1", "name": "veoveo:marketing"}]
        });
        state
            .workflows
            .insert("wfnew00000000001".to_owned(), created);
    }
    assert_eq!(fixture.checkouts.write_back_all().await.unwrap(), 2);
    let written = fixture.file("hello-run.json");
    assert_eq!(written["nodes"][1]["name"], "Edited Greeting");
    for set_by_n8n in ["id", "versionId", "activeVersionId", "isArchived", "tags"] {
        assert!(written.get(set_by_n8n).is_none(), "{set_by_n8n}: {written}");
    }
    assert_eq!(
        fixture.file("invoice-intake-v2.json")["name"],
        "Invoice Intake (v2)"
    );
    // Nothing changed since: nothing is written, and an apply keeps the drafts.
    assert_eq!(fixture.checkouts.write_back_all().await.unwrap(), 0);
    let applied = fixture.checkouts.apply("marketing").await.unwrap();
    assert_eq!(applied.len(), 2);
    assert_eq!(
        fixture.workflow_named("Hello Run")["versionId"],
        uuid::Uuid::from_u128(999).to_string()
    );

    // Without a workflow, open shows the team's workflows.
    let (status, headers, _) = fixture
        .get(&format!("/apps/n8n/open?repository={MARKETING}"))
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(headers[header::LOCATION], format!("{PATH}home/workflows"));
}

#[tokio::test]
async fn open_refuses_what_it_cannot_check_out_and_teams_lists_them() {
    let fixture = fixture().await;
    let (status, _, body) = fixture
        .get(&format!("/apps/n8n/open?repository={SALES}"))
        .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert!(body.contains("names no tracked branch"), "{body}");
    let (status, _, body) = fixture
        .get("/apps/n8n/open?repository=99999999999999999999999999999999")
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let (status, _, body) = fixture.get("/apps/n8n/open").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let (status, _, body) = fixture
        .get(&format!(
            "/apps/n8n/open?repository={MARKETING}&workflow=missing"
        ))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert!(
        !fixture.lore_log.exists()
            || std::fs::read_to_string(&fixture.lore_log)
                .unwrap()
                .lines()
                .count()
                == 1
    );

    let (status, _, body) = fixture.get("/apps/n8n/teams").await;
    assert_eq!(status, StatusCode::OK);
    for expected in [
        "marketing",
        MARKETING,
        "sales",
        SALES,
        &format!("{PATH}open?repository={SALES}"),
    ] {
        assert!(body.contains(expected), "{expected}: {body}");
    }
}

#[tokio::test]
async fn the_editor_acts_as_the_owner_and_the_bridge_as_the_computer() {
    let fixture = fixture().await;
    let request = Request::post("/apps/n8n/rest/workflows?filter=1")
        .header(header::COOKIE, "theme=dark; n8n-auth=forged")
        .header(header::AUTHORIZATION, "Bearer someone")
        .body(Body::from("{}"))
        .unwrap();
    let (status, headers, body) = fixture.send(request).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(headers.get(header::SET_COOKIE).is_none());
    assert_eq!(headers[header::X_FRAME_OPTIONS], "SAMEORIGIN");
    {
        let state = fixture.n8n.lock().unwrap();
        let (method, uri, cookie, origin) = state.editor.last().unwrap();
        assert_eq!(method, Method::POST);
        assert_eq!(uri, "/rest/workflows?filter=1");
        assert_eq!(cookie, &format!("theme=dark; n8n-auth={SESSION}"));
        assert!(origin.starts_with("http://127.0.0.1:"), "{origin}");
    }
    // n8n renewed the session in its answer, which the next request carries.
    fixture.get("/apps/n8n/").await;
    assert_eq!(
        fixture.n8n.lock().unwrap().editor.last().unwrap().2,
        format!("n8n-auth={RENEWED}")
    );
    // Nothing else of the port answers.
    assert_eq!(fixture.get("/rest/login").await.0, StatusCode::NOT_FOUND);

    let bridge = |token: &str, tool: &str| {
        Request::post("/n8n/bridge/calls")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({"runToken": token, "tool": tool, "arguments": {"expression": "now"}})
                    .to_string(),
            ))
            .unwrap()
    };
    let (status, _, body) = fixture.send(bridge("forged", "time__resolve_time")).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, _, body) = fixture.send(bridge(TOKEN, "resolve_time")).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(fixture.hub.calls.lock().unwrap().is_empty());
    let (status, _, body) = fixture.send(bridge(TOKEN, "time__resolve_time")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let answer: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(answer["isError"], false);
    assert_eq!(answer["result"]["tool"], "time__resolve_time");
    assert_eq!(answer["result"]["arguments"], json!({"expression": "now"}));
    assert_eq!(fixture.hub.calls.lock().unwrap().len(), 1);
}
