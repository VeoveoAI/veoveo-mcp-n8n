//! The MCP surface through the SDK's in-process Hub against a migrated database:
//! discovery, the surfaces, permission refusals and the refusal of another tenant. The
//! test user is `user-1` of `tenant-a`. n8n's answers come from a fake n8n on loopback
//! that holds one published workflow; this proves the server's reads, not n8n.
//! Requires the PostgreSQL 18 server programs on `PATH`.
use std::time::Duration;

use axum::{
    Json, Router,
    body::Body,
    extract::Path,
    http::{HeaderMap, StatusCode},
    routing::get,
};
use secrecy::SecretString;
use serde_json::{Value, json};
use url::Url;
use veoveo_fixtures::store::TestDb;
use veoveo_hub_sdk::{
    hosting::{
        Hosted,
        testing::{self, TestHub},
    },
    registration::AppOrigin,
};
use veoveo_mcp_n8n::{
    contract::{
        FileDigest,
        N8nPermission::{self, Editor, WorkflowsRead},
        WorkflowName,
    },
    mcp::{self, N8nMcp, Served},
    migrator,
    n8n::{N8nClient, N8nWorkflowId},
    server,
    store::{AppliedWorkflow, WorkflowStore, stored_tenant},
};

const API_KEY: &str = "test-api-key";
const APP_HOST: &str = "n8n.apps.veoveo.test";
const N8N_ID: &str = "Au7bKCOK5h1uXPLz";
const VERSION: &str = "0190f3a2-7b1c-4d2e-8f3a-4b5c6d7e8f90";

/// A fake n8n whose public API holds the published workflow `N8N_ID`.
async fn fake_n8n() -> Url {
    async fn workflow(Path(id): Path<String>, headers: HeaderMap) -> (StatusCode, Json<Value>) {
        if headers
            .get("x-n8n-api-key")
            .and_then(|key| key.to_str().ok())
            != Some(API_KEY)
        {
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
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route("/api/v1/workflows/{id}", get(workflow)),
        )
        .await
        .unwrap();
    });
    Url::parse(&format!("http://{address}/")).unwrap()
}

fn served(tenant: &str) -> Served {
    Served {
        tenant: veoveo_types::TenantId::parse(tenant).unwrap(),
        editor_url: Url::parse("https://n8n.apps.veoveo.test/").unwrap(),
        hooks_url: Url::parse(
            "https://hub.veoveo.test/instances/019a5b30-0000-7000-8000-000000000001/n8n/hooks/",
        )
        .unwrap(),
    }
}

/// A Hub that grants `granted` in front of a server on `db` serving `tenant`.
async fn hub(db: &TestDb, n8n: &Url, tenant: &str, granted: &[N8nPermission]) -> TestHub {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let workflows = WorkflowStore::new(db.server_store(&server()).await);
    let client = N8nClient::new(n8n.clone())
        .unwrap()
        .with_api_key(SecretString::from(API_KEY));
    let served = served(tenant);
    TestHub::new(
        testing::for_domain::<N8nMcp>()
            .handler(move || {
                Hosted::new(N8nMcp::new(
                    workflows.clone(),
                    client.clone(),
                    served.clone(),
                ))
            })
            .public_routes(mcp::hooks())
            .authenticated_routes(mcp::editor())
            .build(),
    )
    .granting(granted.iter().map(|permission| permission.name()))
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
async fn workflows_and_the_editor_are_read_with_their_permissions_for_one_tenant() {
    tokio::time::timeout(Duration::from_secs(60), async {
        let db = TestDb::new().await;
        db.migrate_server(&server(), migrator()).await;
        let n8n = fake_n8n().await;
        let full = hub(&db, &n8n, "tenant-a", &[WorkflowsRead, Editor]).await;

        // Discovery: the fixed resources and the workflow template; no tool yet.
        let listed = full.rpc("resources/list", json!({})).await;
        let uris: Vec<&str> = listed["result"]["resources"]
            .as_array()
            .unwrap_or_else(|| panic!("{listed}"))
            .iter()
            .filter_map(|resource| resource["uri"].as_str())
            .collect();
        for uri in ["n8n://workflows", "n8n://editor", "n8n://docs", "n8n://contract"] {
            assert!(uris.contains(&uri), "{uri}: {uris:?}");
        }
        let templates = full.rpc("resources/templates/list", json!({})).await;
        assert!(
            templates.to_string().contains("n8n://workflow/{workflow}"),
            "{templates}"
        );
        let tools = full.rpc("tools/list", json!({})).await;
        assert!(
            tools["result"]["tools"]
                .as_array()
                .is_none_or(|tools| tools.is_empty()),
            "{tools}"
        );

        // An empty branch lists no workflow.
        let listing = read_json(&read(&full, "n8n://workflows").await);
        assert_eq!(listing["items"], json!([]));
        assert_eq!(listing["nextCursor"], Value::Null);

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
        assert_eq!(workflow["active"], true);
        assert_eq!(
            workflow["webhooks"],
            json!([{
                "node": "Hook",
                "method": "POST",
                "url": "https://hub.veoveo.test/instances/019a5b30-0000-7000-8000-000000000001/n8n/hooks/webhook/hello-veoveo"
            }])
        );
        let missing = read(&full, "n8n://workflow/not-applied").await;
        assert!(error_message(&missing).contains("no such workflow"), "{missing}");

        let editor = read_json(&read(&full, "n8n://editor").await);
        assert_eq!(editor["editorUrl"], "https://n8n.apps.veoveo.test/");
        assert!(
            editor["hooksUrl"]
                .as_str()
                .unwrap()
                .ends_with("/n8n/hooks/"),
            "{editor}"
        );

        // The editor needs its own permission, and the workflows theirs.
        let reader = hub(&db, &n8n, "tenant-a", &[WorkflowsRead]).await;
        let denied = read(&reader, "n8n://editor").await;
        assert!(error_message(&denied).contains("n8n.editor"), "{denied}");
        let none = hub(&db, &n8n, "tenant-a", &[Editor]).await;
        let denied = read(&none, "n8n://workflows").await;
        assert!(
            error_message(&denied).contains("n8n.workflows.read"),
            "{denied}"
        );

        // A branch that serves another tenant refuses this caller.
        let other = hub(&db, &n8n, "reference", &[WorkflowsRead, Editor]).await;
        for uri in ["n8n://workflows", "n8n://workflow/hello-veoveo", "n8n://editor"] {
            let refused = read(&other, uri).await;
            assert!(
                error_message(&refused).contains("serves another tenant"),
                "{uri}: {refused}"
            );
        }
    })
    .await
    .expect("the MCP test exceeded 60 seconds");
}

#[tokio::test]
async fn the_hooks_are_public_and_the_app_origin_admits_editors() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let db = TestDb::new().await;
    db.migrate_server(&server(), migrator()).await;
    let n8n = fake_n8n().await;
    let workflows = WorkflowStore::new(db.server_store(&server()).await);
    let client = N8nClient::new(n8n)
        .unwrap()
        .with_api_key(SecretString::from(API_KEY));
    let served = served("tenant-a");
    let hub = TestHub::new(
        testing::for_domain::<N8nMcp>()
            .handler(move || {
                Hosted::new(N8nMcp::new(
                    workflows.clone(),
                    client.clone(),
                    served.clone(),
                ))
            })
            .public_routes(mcp::hooks())
            .authenticated_routes(mcp::editor())
            .app_origin(Some(
                AppOrigin::parse(&format!("https://{APP_HOST}")).unwrap(),
            ))
            .build(),
    )
    .granting([Editor.name()]);
    // The hooks need no Hub token; deliveries are not forwarded to n8n yet.
    let (status, body) = hub
        .send(
            hub.request("/hooks/webhook/hello-veoveo")
                .method("POST")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    // The app origin's root needs the Hub's token and n8n.editor; the editor itself is
    // not forwarded yet.
    let editor = |bearer: Option<String>| {
        let mut request = axum::http::Request::builder()
            .uri("/")
            .header("host", APP_HOST);
        if let Some(bearer) = bearer {
            request = request.header("authorization", format!("Bearer {bearer}"));
        }
        request.body(Body::empty()).unwrap()
    };
    let (status, _) = hub.send(editor(None)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = hub.send(editor(Some(hub.token_granting([])))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = hub.send(editor(Some(hub.token()))).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    // The app origin reaches neither MCP nor the hooks.
    let (status, _) = hub
        .send({
            let mut request = editor(Some(hub.token()));
            *request.uri_mut() = "/hooks/webhook/hello-veoveo".parse().unwrap();
            request
        })
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
