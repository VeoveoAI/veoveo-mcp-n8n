//! The MCP surface and the hooks through the SDK's in-process Hub against a migrated
//! database: discovery, the overview App and the tool linked to it, permission refusals,
//! the refusal of another tenant, the workflows, status and executions resources, the
//! data rule of an execution for its run's caller, a writer of the linked repository and
//! a reader, the tools a caller without Task support reaches, and the hooks' deliveries
//! to n8n's webhooks. Callers are `user-1`, the test user, and `user-2` of `tenant-a`,
//! each with a forwarded grant on the runner's linked repository or none. n8n's answers
//! come from the fake n8n in `support`, which proves the server's side, not n8n. Requires
//! the PostgreSQL 18 server programs on `PATH`.
mod support;

use std::{sync::Arc, time::Duration};

use axum::{body::Body, http::StatusCode};
use secrecy::SecretString;
use serde_json::{Value, json};
use support::{API_KEY, FakeExecution, FakeN8n, N8N_ID, VERSION};
use url::Url;
use veoveo_fixtures::store::TestDb;
use veoveo_hub_sdk::{
    ForwardedRequest, HubInternalTokenIssuer, HubProfileId, OAuthClientId, Principal, TokenIssuer,
    TokenSubject,
    apps::AppHtml,
    durable_tasks::DurableTasks,
    hosting::{
        Hosted, HostedServer,
        testing::{self, TestHub},
    },
    registration::{RepositoryAction, RepositoryGrant, RepositoryId, ResourceIndicator},
};
use veoveo_mcp_n8n::{
    contract::{
        ExecutionId, FileDigest,
        N8nPermission::{self, ExecutionsWrite, WorkflowsRead, WorkflowsRun},
        WorkflowName,
    },
    hooks,
    mcp::{N8nMcp, Served},
    migrator,
    n8n::{N8nClient, N8nWorkflowId},
    runs::N8nTasks,
    server,
    store::{AppliedWorkflow, RunStore, WorkflowStore, stored_tenant},
};
use veoveo_store::PlatformStore;
use veoveo_task::{Instance, TaskRuntime};
use veoveo_types::PrincipalId;

/// The team repository the runner serves, by its Lore ID.
const REPOSITORY: &str = "0123456789abcdef0123456789abcdef";
/// The n8n instance whose executions the server dispatched.
const N8N_INSTANCE: uuid::Uuid = uuid::Uuid::from_u128(0x0190_f3a2_7b1c_7d2e_8f3a_4b5c_6d7e_8f91);

fn served(tenant: &str) -> Served {
    Served {
        tenant: veoveo_types::TenantId::parse(tenant).unwrap(),
        hooks_url: Url::parse(
            "https://hub.veoveo.test/instances/019a5b30-0000-7000-8000-000000000001/n8n/hooks/",
        )
        .unwrap(),
        issuer: Url::parse(testing::TEST_ISSUER).unwrap(),
        n8n_version: "2.46.0".to_owned(),
        started_at: "2026-10-31T10:00:00Z".to_owned(),
    }
}

fn app() -> AppHtml {
    AppHtml::load(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/overview.html").as_ref()).unwrap()
}

/// The server on `db` serving `tenant` with `n8n`, its hooks and its Tasks, and the pool
/// it holds, which the test closes.
async fn hosted(db: &TestDb, n8n: &FakeN8n, tenant: &str) -> (HostedServer, PlatformStore) {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let store = db.server_store(&server()).await;
    let pool = store.clone();
    let workflows = WorkflowStore::new(store.clone());
    let runs = RunStore::new(store.clone());
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
    let served = served(tenant);
    let app = app();
    let hosted = testing::for_domain::<N8nMcp>()
        .handler(move || {
            Hosted::new(N8nMcp::new(
                workflows.clone(),
                runs.clone(),
                N8N_INSTANCE,
                client.clone(),
                served.clone(),
                app.clone(),
            ))
            .with_tasks(DurableTasks::tasks_only(tasks.clone()))
        })
        .public_routes(hooks::router(n8n.base.clone()))
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

/// `person` of `tenant-a`: `user-1` is the test user, who calls through the mesh too.
fn person(person: &str) -> Principal {
    let mut principal = testing::principal();
    principal.id = PrincipalId::parse(format!("https://idp.example.com#{person}")).unwrap();
    principal.subject = TokenSubject::parse(person).unwrap();
    principal
}

/// A forwarded token for `person` with `permissions` and, for a runner, their grant on
/// the linked repository holding `actions`.
fn token(person_id: &str, permissions: &[N8nPermission], actions: Option<&[&str]>) -> String {
    let grant = actions.map(|actions| RepositoryGrant {
        repository: RepositoryId::parse(REPOSITORY).unwrap(),
        actions: actions
            .iter()
            .map(|action| RepositoryAction::parse(*action).unwrap())
            .collect(),
    });
    HubInternalTokenIssuer::new(
        TokenIssuer::parse(testing::TEST_ISSUER).unwrap(),
        testing::signing_key(),
    )
    .unwrap()
    .issue_with_repository(
        ForwardedRequest {
            deployment: testing::deployment_id(),
            profile: HubProfileId::parse("operations").unwrap(),
            server: server(),
            audience: ResourceIndicator::parse(testing::TEST_RESOURCE).unwrap(),
            client: OAuthClientId::parse(testing::TEST_CLIENT).unwrap(),
            actor: person(person_id),
            authority: testing::authority(),
            permissions: permissions
                .iter()
                .map(|permission| permission.name())
                .collect(),
            request_context: None,
            acting_server: None,
            acting_task: None,
            delegation_context: None,
            expires_at: chrono::Utc::now() + chrono::TimeDelta::minutes(5),
        },
        grant,
    )
    .unwrap()
    .bearer_token
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

async fn read_as(hub: &TestHub, bearer: &str, uri: &str) -> Value {
    hub.rpc_with("resources/read", json!({"uri": uri}), Some(bearer))
        .await
        .1
}

async fn call_as(hub: &TestHub, bearer: &str, tool: &str, arguments: Value) -> Value {
    hub.rpc_with(
        "tools/call",
        json!({"name": tool, "arguments": arguments}),
        Some(bearer),
    )
    .await
    .1
}

/// The workflow the tests apply: a webhook, with the fake's one published workflow.
async fn record_hello(db: &TestDb) -> Value {
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
    applied
}

#[tokio::test]
async fn discovery_lists_the_overview_app_with_run_linked_and_no_editor() {
    tokio::time::timeout(Duration::from_secs(60), async {
        let db = TestDb::new().await;
        db.migrate_server(&server(), migrator()).await;
        let n8n = FakeN8n::start().await;
        let (full, pool) = hub(
            &db,
            &n8n,
            "tenant-a",
            &[WorkflowsRead, WorkflowsRun, ExecutionsWrite],
        )
        .await;

        let listed = full.rpc("resources/list", json!({})).await;
        let resources = listed["result"]["resources"]
            .as_array()
            .unwrap_or_else(|| panic!("{listed}"));
        let uris: Vec<&str> = resources
            .iter()
            .filter_map(|resource| resource["uri"].as_str())
            .collect();
        for uri in [
            "n8n://workflows",
            "n8n://executions",
            "n8n://status",
            "n8n://docs",
            "n8n://contract",
            "ui://n8n/overview",
        ] {
            assert!(uris.contains(&uri), "{uri}: {uris:?}");
        }
        assert!(!listed.to_string().contains("editor"), "{listed}");
        let templates = full
            .rpc("resources/templates/list", json!({}))
            .await
            .to_string();
        for template in [
            "n8n://workflow/{workflow}",
            "n8n://execution/{execution_id}",
            "n8n://executions{?workflow,status,limit,cursor}",
        ] {
            assert!(templates.contains(template), "{template}: {templates}");
        }

        // `run` is linked to the overview for the model and the App; nothing else is.
        let tools = full.rpc("tools/list", json!({})).await;
        let tools = tools["result"]["tools"]
            .as_array()
            .unwrap_or_else(|| panic!("{tools}"));
        let mut names: Vec<&str> = tools
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect();
        names.sort_unstable();
        assert_eq!(names, ["retry_execution", "run", "stop_execution"]);
        for tool in tools {
            let ui = &tool["_meta"]["ui"];
            if tool["name"] == "run" {
                assert_eq!(ui["resourceUri"], "ui://n8n/overview", "{tool}");
                assert_eq!(ui["visibility"], json!(["model", "app"]), "{tool}");
            } else {
                assert!(ui.is_null(), "{tool}");
            }
        }

        // The overview is self-contained: it finds the teams among the server's resources
        // the host lists, reads each link's, runs with `{link}__run`, opens the person's
        // Computer with ui/open-link and frames nothing.
        let document = full
            .rpc("resources/read", json!({"uri": "ui://n8n/overview"}))
            .await;
        let html = document["result"]["contents"][0]["text"]
            .as_str()
            .unwrap_or_default();
        for needle in [
            "resources/list",
            "__run",
            "ui/open-link",
            "tasks/get",
            "status$",
        ] {
            assert!(html.contains(needle), "{needle}");
        }
        assert!(!html.contains("<iframe"));
        let descriptor = resources
            .iter()
            .find(|resource| resource["uri"] == "ui://n8n/overview")
            .unwrap();
        assert!(!descriptor.to_string().contains("Domains"), "{descriptor}");

        // A view of the runners' shared App calls `{link}__run`, which the host's shared
        // admission resolves to the Hub's name of that link's `run`, as the Hub lists the
        // runner's tools; no other tool of the link is the App's.
        let overview: rmcp::model::Resource = serde_json::from_value(descriptor.clone()).unwrap();
        let linked: Vec<rmcp::model::Tool> = tools
            .iter()
            .map(|tool| {
                let mut tool = tool.clone();
                tool["name"] = json!(format!(
                    "n8n__marketing__{}",
                    tool["name"].as_str().unwrap()
                ));
                serde_json::from_value(tool).unwrap()
            })
            .collect();
        let resolved = veoveo_hub_sdk::apps::resolve_app_tool(&overview, &linked, "marketing__run")
            .expect("the App runs a link's workflow");
        assert_eq!(resolved.name, "n8n__marketing__run");
        for refused in [
            "marketing__stop_execution",
            "marketing__retry_execution",
            "sales__run",
        ] {
            assert!(
                veoveo_hub_sdk::apps::resolve_app_tool(&overview, &linked, refused).is_none(),
                "{refused}"
            );
        }
        pool.close().await;
    })
    .await
    .expect("the discovery test exceeded 60 seconds");
}

#[tokio::test]
async fn workflows_and_status_read_with_their_permissions_for_one_tenant() {
    tokio::time::timeout(Duration::from_secs(60), async {
        let db = TestDb::new().await;
        db.migrate_server(&server(), migrator()).await;
        let n8n = FakeN8n::start().await;
        let (full, full_pool) =
            hub(&db, &n8n, "tenant-a", &[WorkflowsRead, WorkflowsRun, ExecutionsWrite]).await;

        // A team without workflows lists none.
        let listing = read_json(&read(&full, "n8n://workflows").await);
        assert_eq!(listing["items"], json!([]));

        // One workflow the server applied reads with n8n's published state and its
        // webhook's public URL.
        let applied = record_hello(&db).await;
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

        // The status names what the runner applied and, through a runner, the linked
        // repository the caller reached it through, whether their grant writes, and where
        // their own Computer edits it.
        let writer = token("user-2", &[WorkflowsRead], Some(&["write"]));
        let status = read_json(&read_as(&full, &writer, "n8n://status").await);
        assert_eq!(
            status,
            json!({
                "n8nVersion": "2.46.0",
                "serverVersion": env!("CARGO_PKG_VERSION"),
                "startedAt": "2026-10-31T10:00:00Z",
                "workflows": [{"workflow": "hello-veoveo", "fileSha256": "07".repeat(32)}],
                "hooksUrl": "https://hub.veoveo.test/instances/019a5b30-0000-7000-8000-000000000001/n8n/hooks/",
                "repository": REPOSITORY,
                "writer": true,
                "editUrl": format!("https://hub.veoveo.test/computer/apps/n8n/open?repository={REPOSITORY}")
            })
        );
        let reader = token("user-2", &[WorkflowsRead], Some(&[]));
        let status = read_json(&read_as(&full, &reader, "n8n://status").await);
        assert_eq!(status["writer"], false);
        assert_eq!(status["repository"], REPOSITORY);
        // A server that is no runner names no repository and no Computer to edit in.
        let status = read_json(&read(&full, "n8n://status").await);
        assert_eq!(status["writer"], false);
        assert!(status.get("repository").is_none() && status.get("editUrl").is_none(), "{status}");

        // `run` is a Task: a caller without Task support is told so.
        let run = full
            .rpc("tools/call", json!({"name": "run", "arguments": {"workflow": "hello-veoveo"}}))
            .await;
        assert!(run["error"].is_object() || run["result"]["isError"] == true, "{run}");

        // Each surface needs its own permission.
        let (reader, reader_pool) = hub(&db, &n8n, "tenant-a", &[WorkflowsRead]).await;
        let denied = reader
            .rpc("tools/call", json!({"name": "stop_execution", "arguments": {"execution": "2"}}))
            .await;
        assert!(error_message(&denied).contains("n8n.executions.write"), "{denied}");
        let denied = reader
            .rpc("tools/call", json!({"name": "run", "arguments": {"workflow": "hello-veoveo"}}))
            .await;
        assert!(error_message(&denied).contains("n8n.workflows.run"), "{denied}");
        let (none, none_pool) = hub(&db, &n8n, "tenant-a", &[]).await;
        for uri in ["n8n://workflows", "n8n://status", "ui://n8n/overview"] {
            let denied = read(&none, uri).await;
            assert!(error_message(&denied).contains("n8n.workflows.read"), "{uri}: {denied}");
        }

        // A runner that serves another tenant refuses this caller.
        let (other, other_pool) =
            hub(&db, &n8n, "reference", &[WorkflowsRead, ExecutionsWrite]).await;
        for uri in ["n8n://workflows", "n8n://workflow/hello-veoveo", "n8n://status", "n8n://executions", "n8n://execution/1"] {
            let refused = read(&other, uri).await;
            assert!(
                error_message(&refused).contains("serves another tenant"),
                "{uri}: {refused}"
            );
        }
        let refused = other
            .rpc("tools/call", json!({"name": "stop_execution", "arguments": {"execution": "1"}}))
            .await;
        assert!(refused.to_string().contains("serves another tenant"), "{refused}");
        for pool in [full_pool, reader_pool, none_pool, other_pool] {
            pool.close().await;
        }
    })
    .await
    .expect("the MCP test exceeded 60 seconds");
}

#[tokio::test]
async fn an_executions_data_reaches_its_runs_caller_or_a_writer_and_others_read_its_summary() {
    tokio::time::timeout(Duration::from_secs(60), async {
        let db = TestDb::new().await;
        db.migrate_server(&server(), migrator()).await;
        let n8n = FakeN8n::start().await;
        let (hub, pool) = hub(&db, &n8n, "tenant-a", &[]).await;
        record_hello(&db).await;
        // Executions 1 and 2 are a webhook's; 3 and 4 are runs' that user-1's Tasks
        // dispatched.
        {
            let mut state = n8n.state();
            for (id, status) in [(1, "success"), (2, "error"), (3, "success"), (4, "running")] {
                state.executions.insert(
                    id,
                    FakeExecution {
                        status: status.to_owned(),
                        dispatch: json!({"operationId": format!("op-{id}")}),
                        output: vec![json!({"greeting": "hi"})],
                        ..FakeExecution::default()
                    },
                );
            }
        }
        let runs = RunStore::new(db.server_store(&server()).await);
        let tenant = stored_tenant(&veoveo_types::TenantId::parse("tenant-a").unwrap()).unwrap();
        let caller = person("user-1").id;
        for execution in ["3", "4"] {
            runs.record_dispatched(
                tenant,
                N8N_INSTANCE,
                &ExecutionId::parse(execution).unwrap(),
                uuid::Uuid::now_v7(),
                caller.as_str(),
            )
            .await
            .unwrap();
        }
        let all = [WorkflowsRead, ExecutionsWrite];
        let run_caller = token("user-1", &all, Some(&[]));
        let writer = token("user-2", &all, Some(&["write"]));
        let reader = token("user-2", &all, Some(&[]));
        let unlinked = token("user-2", &all, None);

        let data = |execution: &Value| {
            assert_eq!(execution["access"], "data", "{execution}");
            assert!(execution.get("runData").is_some(), "{execution}");
        };
        let summary = |execution: &Value| {
            assert_eq!(execution["access"], "summary", "{execution}");
            for detail in ["output", "runData"] {
                assert!(execution.get(detail).is_none(), "{detail}: {execution}");
            }
        };
        // A webhook's execution: its data to a writer of the repository.
        let one = read_json(&read_as(&hub, &writer, "n8n://execution/1").await);
        data(&one);
        assert_eq!(one["output"], json!([{"greeting": "hi"}]));
        for bearer in [&reader, &run_caller, &unlinked] {
            summary(&read_json(
                &read_as(&hub, bearer, "n8n://execution/1").await,
            ));
        }
        // A failure's summary names the failing node and the error, never its data.
        let failed = read_json(&read_as(&hub, &reader, "n8n://execution/2").await);
        summary(&failed);
        assert_eq!(failed["failedNode"], "Done");
        assert_eq!(failed["error"], "the step failed");
        assert_eq!(failed["status"], "error");
        // A run's execution: its data to the run's caller alone, a writer included out.
        let three = read_json(&read_as(&hub, &run_caller, "n8n://execution/3").await);
        data(&three);
        assert_eq!(three["output"], json!([{"greeting": "hi"}]));
        for bearer in [&writer, &reader, &unlinked] {
            summary(&read_json(
                &read_as(&hub, bearer, "n8n://execution/3").await,
            ));
        }
        // A listing, newest first and by the workflow's file name, carries no detail and
        // names no access.
        let listing = read_json(&read_as(&hub, &writer, "n8n://executions").await);
        let ids: Vec<&str> = listing["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["execution"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["4", "3", "2", "1"], "{listing}");
        for item in listing["items"].as_array().unwrap() {
            assert_eq!(item["workflow"], "hello-veoveo", "{item}");
            for detail in ["access", "output", "runData", "failedNode", "error"] {
                assert!(item.get(detail).is_none(), "{detail}: {item}");
            }
        }
        let failed = read_json(
            &read_as(
                &hub,
                &reader,
                "n8n://executions?workflow=hello-veoveo&status=error",
            )
            .await,
        );
        assert_eq!(failed["items"].as_array().unwrap().len(), 1, "{failed}");
        let none =
            read_json(&read_as(&hub, &reader, "n8n://executions?workflow=not-applied").await);
        assert_eq!(none["items"], json!([]));

        // Stopping and retrying need the same access.
        let refusal = "only the caller of the run that started this execution";
        for (bearer, execution) in [(&reader, "1"), (&writer, "4"), (&reader, "4")] {
            let refused = call_as(
                &hub,
                bearer,
                "stop_execution",
                json!({"execution": execution}),
            )
            .await;
            assert!(
                refused.to_string().contains(refusal),
                "{execution}: {refused}"
            );
        }
        let refused = call_as(&hub, &reader, "retry_execution", json!({"execution": "2"})).await;
        assert!(refused.to_string().contains(refusal), "{refused}");
        assert!(n8n.state().stops.is_empty() && n8n.state().retries.is_empty());
        let finished = call_as(&hub, &writer, "stop_execution", json!({"execution": "1"})).await;
        assert!(
            finished.to_string().contains("already finished"),
            "{finished}"
        );
        let stopped = call_as(
            &hub,
            &run_caller,
            "stop_execution",
            json!({"execution": "4"}),
        )
        .await;
        assert_eq!(
            stopped["result"]["structuredContent"]["status"], "canceled",
            "{stopped}"
        );
        assert_eq!(
            stopped["result"]["structuredContent"]["access"], "data",
            "{stopped}"
        );
        // A writer's retry of a webhook's execution stays the writers'.
        let retried = call_as(&hub, &writer, "retry_execution", json!({"execution": "2"})).await;
        let retried = &retried["result"]["structuredContent"];
        assert_eq!(retried["retryOf"], "2", "{retried}");
        assert_eq!(retried["access"], "data", "{retried}");
        let id = retried["execution"].as_str().unwrap().to_owned();
        summary(&read_json(
            &read_as(&hub, &run_caller, &format!("n8n://execution/{id}")).await,
        ));
        // A retry of a run's execution stays its caller's, who alone may ask for it.
        let retried = call_as(
            &hub,
            &run_caller,
            "retry_execution",
            json!({"execution": "3"}),
        )
        .await;
        let retried = &retried["result"]["structuredContent"];
        assert_eq!(retried["access"], "data", "{retried}");
        let id = retried["execution"].as_str().unwrap().to_owned();
        summary(&read_json(
            &read_as(&hub, &writer, &format!("n8n://execution/{id}")).await,
        ));
        data(&read_json(
            &read_as(&hub, &run_caller, &format!("n8n://execution/{id}")).await,
        ));
        assert_eq!(n8n.state().stops, [1, 4]);
        assert_eq!(n8n.state().retries, [2, 3]);
        pool.close().await;
    })
    .await
    .expect("the data rule test exceeded 60 seconds");
}

#[tokio::test]
async fn the_hooks_reach_n8n_s_webhooks_alone() {
    let db = TestDb::new().await;
    db.migrate_server(&server(), migrator()).await;
    let n8n = FakeN8n::start().await;
    let (hub, hub_pool) = hub(&db, &n8n, "tenant-a", &[WorkflowsRead]).await;

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
    // n8n's editor and REST API are nowhere on the server's surface.
    for path in ["/rest/login", "/", "/home/workflows"] {
        let (status, _) = hub
            .send(
                hub.request(path)
                    .header("authorization", format!("Bearer {}", hub.token()))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
    }
    hub_pool.close().await;
}
