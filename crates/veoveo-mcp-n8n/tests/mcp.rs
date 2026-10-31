//! The MCP surface and the server's HTTP surfaces through the SDK's in-process Hub
//! against a migrated database: discovery, permission refusals, the refusal of another
//! tenant, the workflows, executions and editor resources, the tools a caller without
//! Task support reaches, the hooks' deliveries to n8n's webhooks and the editor's proxy on
//! the app origin. The test user is `user-1` of `tenant-a`. n8n's answers come from the
//! fake n8n in `support`, which proves the server's side, not n8n. Requires the
//! PostgreSQL 18 server programs on `PATH`.
mod support;

use std::{sync::Arc, time::Duration};

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use secrecy::SecretString;
use serde_json::{Value, json};
use support::{API_KEY, FakeN8n, N8N_ID, OWNER_SESSION, RENEWED_SESSION, VERSION};
use tower::ServiceExt;
use url::Url;
use veoveo_fixtures::store::TestDb;
use veoveo_hub_sdk::{
    apps::AppHtml,
    durable_tasks::DurableTasks,
    hosting::{
        Hosted, HostedServer,
        testing::{self, TestHub},
    },
    registration::AppOrigin,
};
use veoveo_mcp_n8n::{
    contract::{
        FileDigest,
        N8nPermission::{self, Editor, ExecutionsWrite, WorkflowsRead, WorkflowsRun},
        WorkflowName,
    },
    editor::{self, EditorSession},
    hooks,
    mcp::{N8nMcp, Served},
    migrator,
    n8n::{N8nClient, N8nWorkflowId},
    runs::N8nTasks,
    server,
    store::{AppliedWorkflow, WorkflowStore, stored_tenant},
};
use veoveo_store::PlatformStore;
use veoveo_task::{Instance, TaskRuntime};

const APP_HOST: &str = "n8n.apps.veoveo.test";

fn served(tenant: &str) -> Served {
    Served {
        tenant: veoveo_types::TenantId::parse(tenant).unwrap(),
        editor_url: Url::parse(&format!("https://{APP_HOST}/")).unwrap(),
        hooks_url: Url::parse(
            "https://hub.veoveo.test/instances/019a5b30-0000-7000-8000-000000000001/n8n/hooks/",
        )
        .unwrap(),
    }
}

fn app() -> AppHtml {
    AppHtml::load(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/editor.html").as_ref()).unwrap()
}

/// The server on `db` serving `tenant` with `n8n`, its hooks, its editor and its Tasks,
/// and the pool it holds, which the test closes.
async fn hosted(db: &TestDb, n8n: &FakeN8n, tenant: &str) -> (HostedServer, PlatformStore) {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let store = db.server_store(&server()).await;
    let pool = store.clone();
    let workflows = WorkflowStore::new(store.clone());
    let instance = Arc::new(Instance::start(&store, &server()).await.unwrap());
    let runtime = TaskRuntime::new(store, server(), testing::deployment_id(), instance.clone());
    let tasks = N8nTasks::new(
        runtime,
        workflows.clone(),
        veoveo_types::TenantId::parse(tenant).unwrap(),
    );
    let client = N8nClient::new(n8n.base.clone())
        .unwrap()
        .with_api_key(SecretString::from(API_KEY));
    let session = Arc::new(EditorSession::new(
        N8nClient::new(n8n.base.clone()).unwrap(),
        SecretString::from("owner-password"),
    ));
    let served = served(tenant);
    let app = app();
    let hosted = testing::for_domain::<N8nMcp>()
        .handler(move || {
            Hosted::new(N8nMcp::new(
                workflows.clone(),
                client.clone(),
                served.clone(),
                app.clone(),
            ))
            .with_tasks(DurableTasks::tasks_only(tasks.clone()))
        })
        .public_routes(hooks::router(n8n.base.clone()))
        .authenticated_routes(editor::router(
            n8n.base.clone(),
            session,
            Some(APP_HOST.to_owned()),
        ))
        .app_origin(Some(
            AppOrigin::parse(&format!("https://{APP_HOST}")).unwrap(),
        ))
        .build();
    let _ = instance;
    (hosted, pool)
}

/// A Hub that grants `granted` in front of the server, and the pool to close.
async fn hub(
    db: &TestDb,
    n8n: &FakeN8n,
    tenant: &str,
    granted: &[N8nPermission],
) -> (TestHub, PlatformStore) {
    let (hosted, pool) = hosted(db, n8n, tenant).await;
    (
        TestHub::new(hosted).granting(granted.iter().map(|permission| permission.name())),
        pool,
    )
}

fn error_message(response: &Value) -> &str {
    response["error"]["message"].as_str().unwrap_or_default()
}

fn read_json(response: &Value) -> Value {
    let text = response["result"]["contents"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("no contents: {response}"));
    serde_json::from_str(text).unwrap()
}

async fn read(hub: &TestHub, uri: &str) -> Value {
    hub.rpc("resources/read", json!({"uri": uri})).await
}

#[tokio::test]
async fn workflows_executions_and_the_editor_are_read_with_their_permissions_for_one_tenant() {
    tokio::time::timeout(Duration::from_secs(60), async {
        let db = TestDb::new().await;
        db.migrate_server(&server(), migrator()).await;
        let n8n = FakeN8n::start().await;
        let (full, full_pool) =
            hub(&db, &n8n, "tenant-a", &[WorkflowsRead, WorkflowsRun, ExecutionsWrite, Editor]).await;

        // Discovery: the fixed resources, the templates, the editor's App and the tools.
        let listed = full.rpc("resources/list", json!({})).await;
        let uris: Vec<&str> = listed["result"]["resources"]
            .as_array()
            .unwrap_or_else(|| panic!("{listed}"))
            .iter()
            .filter_map(|resource| resource["uri"].as_str())
            .collect();
        for uri in [
            "n8n://workflows",
            "n8n://executions",
            "n8n://editor",
            "n8n://docs",
            "n8n://contract",
            "ui://n8n/editor",
        ] {
            assert!(uris.contains(&uri), "{uri}: {uris:?}");
        }
        let templates = full.rpc("resources/templates/list", json!({})).await.to_string();
        for template in [
            "n8n://workflow/{workflow}",
            "n8n://execution/{execution_id}",
            "n8n://executions{?workflow,status,limit,cursor}",
        ] {
            assert!(templates.contains(template), "{template}: {templates}");
        }
        let tools = full.rpc("tools/list", json!({})).await;
        let mut names: Vec<&str> = tools["result"]["tools"]
            .as_array()
            .unwrap_or_else(|| panic!("{tools}"))
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect();
        names.sort_unstable();
        assert_eq!(names, ["editor_changes", "retry_execution", "run", "stop_execution"]);

        // The editor's App is the view that frames the app origin.
        let app = full.rpc("resources/read", json!({"uri": "ui://n8n/editor"})).await;
        let html = app["result"]["contents"][0]["text"].as_str().unwrap_or_default();
        assert!(html.contains("n8n://editor"), "{app}");

        // An empty branch lists no workflow.
        let listing = read_json(&read(&full, "n8n://workflows").await);
        assert_eq!(listing["items"], json!([]));

        // One workflow the server applied reads with n8n's published state and its
        // webhook's public URL.
        let workflows = WorkflowStore::new(db.server_store(&server()).await);
        let tenant = stored_tenant(&veoveo_types::TenantId::parse("tenant-a").unwrap()).unwrap();
        let applied = json!({
            "name": "Hello Veoveo",
            "nodes": [{"name": "Hook", "type": "n8n-nodes-base.webhook",
                       "parameters": {"httpMethod": "POST", "path": "hello-veoveo"}}],
            "connections": {},
            "settings": {}
        });
        workflows
            .record(
                tenant,
                &AppliedWorkflow {
                    workflow: WorkflowName::parse("hello-veoveo").unwrap(),
                    n8n_id: N8nWorkflowId::parse(N8N_ID).unwrap(),
                    digest: FileDigest::from_bytes([7; 32]),
                    version: uuid::Uuid::parse_str(VERSION).unwrap(),
                    runnable: false,
                    applied: applied.clone(),
                },
            )
            .await
            .unwrap();
        let listing = read_json(&read(&full, "n8n://workflows").await);
        assert_eq!(
            listing["items"],
            json!([{
                "workflow": "hello-veoveo",
                "workflowUri": "n8n://workflow/hello-veoveo",
                "runnable": false,
                "active": true,
                "fileSha256": "07".repeat(32)
            }])
        );
        let workflow = read_json(&read(&full, "n8n://workflow/hello-veoveo").await);
        assert_eq!(workflow["applied"], applied);
        assert_eq!(
            workflow["webhooks"][0]["url"],
            "https://hub.veoveo.test/instances/019a5b30-0000-7000-8000-000000000001/n8n/hooks/webhook/hello-veoveo"
        );
        let missing = read(&full, "n8n://workflow/not-applied").await;
        assert!(error_message(&missing).contains("no such workflow"), "{missing}");

        // n8n's executions read with the workflow's file name, newest first.
        {
            let mut state = n8n.state();
            for (id, status) in [(1, "success"), (2, "error")] {
                state.executions.insert(
                    id,
                    support::FakeExecution {
                        status: status.to_owned(),
                        dispatch: json!({"operationId": "op"}),
                        output: vec![json!({"greeting": "hi"})],
                    },
                );
            }
        }
        let executions = read_json(&read(&full, "n8n://executions").await);
        let ids: Vec<&str> = executions["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["execution"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["2", "1"], "{executions}");
        assert_eq!(executions["items"][0]["workflow"], "hello-veoveo");
        let failed = read_json(&read(&full, "n8n://executions?workflow=hello-veoveo&status=error").await);
        assert_eq!(failed["items"].as_array().unwrap().len(), 1, "{failed}");
        let none = read_json(&read(&full, "n8n://executions?workflow=not-applied").await);
        assert_eq!(none["items"], json!([]));
        let one = read_json(&read(&full, "n8n://execution/1").await);
        assert_eq!(one["status"], "success");
        assert_eq!(one["output"], json!([{"greeting": "hi"}]));
        let stopped = full
            .rpc("tools/call", json!({"name": "stop_execution", "arguments": {"execution": "1"}}))
            .await;
        assert!(stopped.to_string().contains("already finished"), "{stopped}");

        // `run` is a Task: a caller without Task support is told so.
        let run = full
            .rpc("tools/call", json!({"name": "run", "arguments": {"workflow": "hello-veoveo"}}))
            .await;
        assert!(run["error"].is_object() || run["result"]["isError"] == true, "{run}");

        let editor = read_json(&read(&full, "n8n://editor").await);
        assert_eq!(editor["editorUrl"], format!("https://{APP_HOST}/"));

        // Each surface needs its own permission.
        let (reader, reader_pool) = hub(&db, &n8n, "tenant-a", &[WorkflowsRead]).await;
        let denied = read(&reader, "n8n://editor").await;
        assert!(error_message(&denied).contains("n8n.editor"), "{denied}");
        let denied = reader.rpc("resources/read", json!({"uri": "ui://n8n/editor"})).await;
        assert!(error_message(&denied).contains("n8n.editor"), "{denied}");
        let denied = reader
            .rpc("tools/call", json!({"name": "stop_execution", "arguments": {"execution": "2"}}))
            .await;
        assert!(error_message(&denied).contains("n8n.executions.write"), "{denied}");
        let denied = reader
            .rpc("tools/call", json!({"name": "run", "arguments": {"workflow": "hello-veoveo"}}))
            .await;
        assert!(error_message(&denied).contains("n8n.workflows.run"), "{denied}");
        let (none, none_pool) = hub(&db, &n8n, "tenant-a", &[Editor]).await;
        let denied = read(&none, "n8n://workflows").await;
        assert!(error_message(&denied).contains("n8n.workflows.read"), "{denied}");

        // A branch that serves another tenant refuses this caller.
        let (other, other_pool) =
            hub(&db, &n8n, "reference", &[WorkflowsRead, ExecutionsWrite, Editor]).await;
        for uri in ["n8n://workflows", "n8n://workflow/hello-veoveo", "n8n://editor", "n8n://executions", "n8n://execution/1"] {
            let refused = read(&other, uri).await;
            assert!(
                error_message(&refused).contains("serves another tenant"),
                "{uri}: {refused}"
            );
        }
        let refused = other
            .rpc("tools/call", json!({"name": "editor_changes", "arguments": {}}))
            .await;
        assert!(refused.to_string().contains("serves another tenant"), "{refused}");
        for pool in [full_pool, reader_pool, none_pool, other_pool] {
            pool.close().await;
        }
    })
    .await
    .expect("the MCP test exceeded 60 seconds");
}

fn on_app_origin(
    path: &str,
    method: &str,
    bearer: Option<String>,
    cookie: Option<&str>,
) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("host", APP_HOST);
    if let Some(bearer) = bearer {
        request = request.header("authorization", format!("Bearer {bearer}"));
    }
    if let Some(cookie) = cookie {
        request = request.header("cookie", cookie);
    }
    request.body(Body::empty()).unwrap()
}

#[tokio::test]
async fn the_hooks_reach_n8n_s_webhooks_and_the_editor_acts_as_n8n_s_owner() {
    let db = TestDb::new().await;
    db.migrate_server(&server(), migrator()).await;
    let n8n = FakeN8n::start().await;
    let (hub, hub_pool) = hub(&db, &n8n, "tenant-a", &[Editor]).await;
    // The editor's answers are read whole, headers included, from the server's router.
    let (hosted, router_pool) = hosted(&db, &n8n, "tenant-a").await;
    let router = hosted.into_router();
    let send = |request: Request<Body>| {
        let router = router.clone();
        async move { router.oneshot(request).await.unwrap() }
    };

    // A delivery reaches n8n's webhook without the server's headers or cookies, and its
    // answer sets no cookie.
    let (status, body) = hub
        .send(
            hub.request("/hooks/webhook/hello-veoveo?source=github")
                .method("POST")
                .header("x-veoveo-dispatch", "run")
                .header("x-hub-signature-256", "sha256=abc")
                .header("cookie", "session=1")
                .body(Body::from("{\"ping\":true}"))
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("hello-veoveo"), "{body}");
    {
        let state = n8n.state();
        let (method, path, headers, delivered) = state.deliveries.last().unwrap();
        assert_eq!((method.as_str(), path.as_str()), ("POST", "hello-veoveo"));
        assert_eq!(delivered.as_ref(), b"{\"ping\":true}");
        assert!(!headers.contains_key("x-veoveo-dispatch"));
        assert!(!headers.contains_key("cookie"));
        assert_eq!(headers["x-hub-signature-256"], "sha256=abc");
    }
    // A delivery never reaches n8n's REST or public API, nor a Veoveo Trigger.
    for path in [
        "/hooks/rest/login",
        "/hooks/api/v1/workflows",
        "/hooks/healthz",
    ] {
        let (status, _) = hub
            .send(
                hub.request(path)
                    .method("POST")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
    }
    let (status, _) = hub
        .send(
            hub.request("/hooks/webhook/4f9a/run")
                .method("POST")
                .header("x-veoveo-dispatch", "run")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "a delivery started a run");
    assert!(n8n.state().dispatches.is_empty());

    // The editor: the Hub's token and n8n.editor admit a person, whose request reaches n8n
    // with the owner's session in place of any the browser sent and without the Hub's
    // token; the answer carries neither n8n's cookie nor its frame refusal.
    let response = send(on_app_origin("/", "GET", None, None)).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let response = send(on_app_origin(
        "/",
        "GET",
        Some(hub.token_granting([])),
        None,
    ))
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let response = send(on_app_origin(
        "/rest/workflows?filter=1",
        "PATCH",
        Some(hub.token()),
        Some("theme=dark; n8n-auth=forged"),
    ))
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().get("set-cookie").is_none());
    assert!(response.headers().get("x-frame-options").is_none());
    let body = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    let echoed: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(echoed["uri"], "/rest/workflows?filter=1");
    assert_eq!(
        echoed["cookie"],
        format!("theme=dark; n8n-auth={OWNER_SESSION}")
    );
    assert_eq!(echoed["authorization"], false);
    // The owner signed in once; n8n's renewed session serves the next request.
    let response = send(on_app_origin("/", "GET", Some(hub.token()), None)).await;
    assert_eq!(response.status(), StatusCode::OK);
    {
        let state = n8n.state();
        assert_eq!(state.logins, 1);
        let (_, _, headers) = state.editor.last().unwrap();
        assert_eq!(
            headers["cookie"],
            format!("n8n-auth={RENEWED_SESSION}").as_str()
        );
    }
    // The editor is the app origin's alone: under the mount it answers nothing.
    let (status, _) = hub
        .send(
            hub.request("/rest/login")
                .header("authorization", format!("Bearer {}", hub.token()))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    hub_pool.close().await;
    router_pool.close().await;
}
