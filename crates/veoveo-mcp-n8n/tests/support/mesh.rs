//! The server in the SDK's in-process mesh with its Task worker and loopback listener on a
//! migrated database, the fake n8n behind it and a fake Time server whose `resolve_time`
//! the bridge calls. The test user holds `granted`.
use std::{
    collections::BTreeSet,
    sync::{Arc, LazyLock},
    time::Duration,
};

use rmcp::model::{Implementation, ServerCapabilities, ServerConfig};
use schemars::JsonSchema;
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use url::Url;
use veoveo_fixtures::store::TestDb;
use veoveo_hub_sdk::{
    HubInternalIdentity,
    apps::AppHtml,
    calls::ProviderContract,
    docs::ServerDocs,
    durable_tasks::DurableTasks,
    hosting::{
        DomainServer, Hosted, RoutePolicy, Routes, ServerCalls, hub_identity,
        testing::{self, RunningMesh, TestMesh},
    },
    registration::{Access, PermissionDefinition},
    server_contract::{McpServerContract, McpServerSetup},
};
use veoveo_mcp_n8n::{
    bridge::Bridge,
    contract::{ExecutionId, FileDigest, WorkflowName},
    loopback,
    mcp::{AUTHORIZATION, N8nMcp, Served},
    migrator,
    n8n::{N8nClient, N8nWorkflowId, VEOVEO_RESULT, VEOVEO_TRIGGER},
    runs::{N8nTasks, Runner},
    server,
    store::{AppliedWorkflow, Dispatched, RunStore, WorkflowStore, stored_tenant},
};
use veoveo_store::TenantId;
use veoveo_task::{Instance, TaskRuntime};
use veoveo_types::{PermissionName, ResourceScheme, ServerSlug, Vocabulary};

use super::{API_KEY, FakeN8n, N8N_ID, VERSION};

pub const WORKFLOW: &str = "hello-run";
pub const TRIGGER_WEBHOOK: &str = "4f9a0c1d-2e3f-4a5b-8c6d-7e8f9a0b1c2d";
const DEADLINE: Duration = Duration::from_secs(45);

// The fake Time server: one tool, `resolve_time`, which answers with its arguments and
// the caller it acted for.

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Vocabulary)]
#[vocabulary(scope)]
pub enum TimeScope {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Vocabulary)]
pub enum TimePermission {
    #[vocabulary(rename = "time.authorities.read")]
    AuthoritiesRead,
}
impl PermissionDefinition for TimePermission {
    fn resource(self) -> &'static str {
        "time.authority"
    }
    fn access(self) -> Access {
        Access::Read
    }
}

pub struct TimeProvider;
impl ProviderContract for TimeProvider {
    fn slug() -> ServerSlug {
        ServerSlug::parse("time").unwrap()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[veoveo_hub_sdk::tool(provider = TimeProvider, output = Resolved, name = "resolve_time")]
pub struct ResolveTime {
    pub expression: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[derive(veoveo_hub_sdk::SingleOutput)]
pub struct Resolved {
    pub expression: Value,
    /// The caller the call acted for, and the server and Task it came through.
    pub caller: String,
    pub acting_server: Option<String>,
    pub acting_task: bool,
}

static TIME_DOCS: LazyLock<ServerDocs> = LazyLock::new(|| {
    ServerDocs::new(TimeProvider::slug())
        .with_doc("agents", "Agents", "# Agents")
        .with_doc("design", "Design", "# Design")
});
pub struct TimeContract;
#[derive(Clone)]
pub struct TimeDomain;
static TIME_SETUP: LazyLock<McpServerSetup<TimeContract>> =
    LazyLock::new(|| McpServerSetup::new().unwrap());
static TIME_ROUTES: LazyLock<Routes<TimeDomain>> = LazyLock::new(|| {
    let read = || RoutePolicy::permission(TimePermission::AuthoritiesRead).unwrap();
    Routes::<TimeDomain>::builder(read())
        .tool::<ResolveTime, _, _>("Resolve a time", read(), |_, request, context| async move {
            let identity: HubInternalIdentity = hub_identity(&context)?;
            Ok(Resolved {
                expression: request.expression,
                caller: identity.actor.id.as_str().to_owned(),
                acting_server: identity
                    .acting_server
                    .as_ref()
                    .map(|acting| acting.server.as_str().to_owned()),
                acting_task: identity.acting_task.is_some(),
            })
        })
        .build()
});
impl McpServerContract for TimeContract {
    type Server = TimeDomain;
    type Provider = TimeProvider;
    type Scope = TimeScope;
    fn scheme() -> ResourceScheme {
        ResourceScheme::parse("time").unwrap()
    }
    fn scopes() -> &'static [TimeScope] {
        TimeScope::ALL
    }
    fn documents() -> &'static ServerDocs {
        &TIME_DOCS
    }
    fn server_config() -> ServerConfig {
        let mut config = ServerConfig::new(ServerCapabilities::builder().enable_tools().build());
        config.server_info = Implementation::new("time", "1");
        config
    }
}
impl DomainServer for TimeDomain {
    type Contract = TimeContract;
    fn setup() -> &'static McpServerSetup<TimeContract> {
        &TIME_SETUP
    }
    fn routes() -> &'static Routes<Self> {
        &TIME_ROUTES
    }
}

/// The server's Task worker and loopback listener on their own process instance.
pub struct Worker {
    pub runtime: TaskRuntime,
    stop: CancellationToken,
    join: tokio::task::JoinHandle<()>,
    serve: tokio::task::JoinHandle<()>,
}

impl Worker {
    pub async fn stop(self) {
        self.stop.cancel();
        let _ = self.join.await;
        let _ = self.serve.await;
    }
}

pub struct Harness {
    pub db: TestDb,
    pub n8n: FakeN8n,
    pub mesh: RunningMesh,
    pub calls: ServerCalls,
    pub tenant: TenantId,
    /// The n8n instance the worker names its jobs with.
    pub instance: uuid::Uuid,
    pub loopback: Url,
    listener_port: u16,
    pub worker: Option<Worker>,
}

fn applied() -> AppliedWorkflow {
    AppliedWorkflow {
        workflow: WorkflowName::parse(WORKFLOW).unwrap(),
        n8n_id: N8nWorkflowId::parse(N8N_ID).unwrap(),
        digest: FileDigest::from_bytes([1; 32]),
        version: uuid::Uuid::parse_str(VERSION).unwrap(),
        runnable: true,
        applied: json!({
            "name": "Hello Run",
            "nodes": [
                {"name": "Veoveo Trigger", "type": VEOVEO_TRIGGER, "webhookId": TRIGGER_WEBHOOK},
                {"name": "Done", "type": VEOVEO_RESULT}
            ],
            "connections": {},
            "settings": {}
        }),
    }
}

pub async fn harness(granted: &[&str]) -> Harness {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();
    let db = TestDb::new().await;
    db.migrate_server(&server(), migrator()).await;
    let n8n = FakeN8n::start().await;
    let tenant_id = veoveo_types::TenantId::parse("tenant-a").unwrap();
    let tenant = stored_tenant(&tenant_id).unwrap();
    let store = db.server_store(&server()).await;
    WorkflowStore::new(store.clone())
        .record(tenant, &applied())
        .await
        .unwrap();

    let mut mesh = TestMesh::bind().await;
    let calls = mesh.register(AUTHORIZATION.clone());
    let routes_runtime = TaskRuntime::new(
        store.clone(),
        server(),
        testing::deployment_id(),
        Arc::new(Instance::start(&store, &server()).await.unwrap()),
    );
    let workflows = WorkflowStore::new(store.clone());
    let tasks = N8nTasks::new(routes_runtime, workflows.clone(), tenant_id.clone());
    let client = N8nClient::new(n8n.base.clone())
        .unwrap()
        .with_api_key(SecretString::from(API_KEY));
    let served = Served {
        tenant: tenant_id,
        hooks_url: Url::parse("https://hub.veoveo.test/instances/x/n8n/hooks/").unwrap(),
        issuer: Url::parse(testing::TEST_ISSUER).unwrap(),
        n8n_version: "2.46.0".to_owned(),
        started_at: "2026-10-31T10:00:00Z".to_owned(),
        tracked_branch: None,
    };
    let app = AppHtml::load(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/overview.html").as_ref())
        .unwrap();
    let instance = uuid::Uuid::now_v7();
    let runs = RunStore::new(store.clone());
    mesh.serve(
        testing::for_domain::<N8nMcp>()
            .calls(calls.clone())
            .handler(move || {
                Hosted::new(N8nMcp::new(
                    workflows.clone(),
                    runs.clone(),
                    instance,
                    client.clone(),
                    served.clone(),
                    app.clone(),
                ))
                .with_tasks(DurableTasks::tasks_only(tasks.clone()))
            })
            .build(),
    );
    mesh.register(TIME_ROUTES.authorization());
    mesh.serve(
        testing::for_domain::<TimeDomain>()
            .handler(|| Hosted::new(TimeDomain))
            .build(),
    );
    let mesh = mesh
        .granting(
            granted
                .iter()
                .map(|name| PermissionName::parse(*name).unwrap()),
        )
        .start();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let listener_port = listener.local_addr().unwrap().port();
    let loopback = Url::parse(&format!("http://127.0.0.1:{listener_port}/")).unwrap();
    let mut harness = Harness {
        db,
        n8n,
        mesh,
        calls,
        tenant,
        instance,
        loopback,
        listener_port,
        worker: None,
    };
    harness.worker = Some(harness.start_worker(Some(listener)).await);
    harness
}

impl Harness {
    /// A worker and loopback listener on a process instance of their own.
    pub async fn start_worker(&self, listener: Option<tokio::net::TcpListener>) -> Worker {
        let store = self.db.server_store(&server()).await;
        let instance = Arc::new(Instance::start(&store, &server()).await.unwrap());
        let runtime = TaskRuntime::new(store.clone(), server(), testing::deployment_id(), instance);
        let n8n = N8nClient::new(self.n8n.base.clone())
            .unwrap()
            .with_api_key(SecretString::from(API_KEY));
        let runs = RunStore::new(store.clone());
        let runner = Runner {
            runtime: runtime.clone(),
            workflows: WorkflowStore::new(store.clone()),
            runs: runs.clone(),
            n8n,
            tenant: self.tenant,
            instance: self.instance,
            loopback: self.loopback.clone(),
        };
        let stop = CancellationToken::new();
        let join = tokio::spawn({
            let stop = stop.clone();
            let calls = self.calls.clone();
            async move {
                let _ = runner.work(Some(calls), stop).await;
            }
        });
        let listener = match listener {
            Some(listener) => listener,
            None => tokio::net::TcpListener::bind(("127.0.0.1", self.listener_port))
                .await
                .unwrap(),
        };
        let bridge = Bridge::new(runtime.clone(), runs, self.tenant, Some(self.calls.clone()));
        let serve = tokio::spawn({
            let stop = stop.clone();
            let router = loopback::router(runtime.clone(), bridge);
            async move {
                let _ = loopback::serve_on(listener, router, async move { stop.cancelled().await })
                    .await;
            }
        });
        Worker {
            runtime,
            stop,
            join,
            serve,
        }
    }

    /// Start a run of the example workflow with `input` and return its Task.
    pub async fn run(&self, input: Value) -> String {
        let started = self
            .mesh
            .start_task("n8n", "run", json!({"workflow": WORKFLOW, "input": input}))
            .await;
        started["result"]["taskId"]
            .as_str()
            .or_else(|| started["result"]["task"]["taskId"].as_str())
            .unwrap_or_else(|| panic!("no Task started: {started}"))
            .to_owned()
    }

    pub async fn task(&self, task: &str) -> Value {
        self.mesh.task("n8n", task).await["result"].clone()
    }

    /// Wait until the Task's MCP view satisfies `done`.
    pub async fn until(&self, task: &str, done: impl Fn(&Value) -> bool) -> Value {
        let last = std::sync::Mutex::new(Value::Null);
        let waited = tokio::time::timeout(DEADLINE, async {
            loop {
                let view = self.task(task).await;
                if done(&view) {
                    return view;
                }
                *last.lock().unwrap() = view;
                self.db.a.reap_instances().await.unwrap();
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await;
        waited.unwrap_or_else(|_| {
            panic!(
                "Task {task} never reached the expected state; last {}",
                last.lock().unwrap()
            )
        })
    }

    /// Wait until the fake's trigger received `count` dispatches and return the last.
    pub async fn dispatched(&self, count: usize) -> Value {
        tokio::time::timeout(DEADLINE, async {
            loop {
                {
                    let state = self.n8n.state();
                    if state.dispatches.len() >= count {
                        return state.dispatches[count - 1].1.clone();
                    }
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("the run never dispatched")
    }

    /// Wait until the run of `task` recorded its job, the n8n instance and execution, and
    /// return it.
    pub async fn submitted(&self, task: &str) -> String {
        let runtime = self.worker.as_ref().unwrap().runtime.clone();
        let id = veoveo_types::TaskId::parse(task).unwrap();
        let key = veoveo_task::OperationKey::parse("execution").unwrap();
        tokio::time::timeout(DEADLINE, async {
            loop {
                if let Some(job) = runtime
                    .operation(id, &key)
                    .await
                    .unwrap()
                    .and_then(|operation| operation.external_id)
                {
                    return job.as_str().to_owned();
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the run never recorded its job")
    }

    /// The run's Task that dispatched `execution`, and its caller, as the worker recorded
    /// them.
    pub async fn dispatched_to(&self, execution: u64) -> Option<Dispatched> {
        RunStore::new(self.db.server_store(&server()).await)
            .dispatched(
                self.tenant,
                self.instance,
                &ExecutionId::parse(execution.to_string()).unwrap(),
            )
            .await
            .unwrap()
    }

    pub fn dispatch_count(&self) -> usize {
        self.n8n.state().dispatches.len()
    }

    /// The executions the fake started, by dispatch order.
    pub fn execution_of(&self, dispatch: &Value) -> u64 {
        let state = self.n8n.state();
        *state
            .executions
            .iter()
            .find(|(_, execution)| execution.dispatch == *dispatch)
            .expect("the dispatch started an execution")
            .0
    }

    pub fn stops(&self) -> BTreeSet<u64> {
        self.n8n.state().stops.iter().copied().collect()
    }
}

/// The status of an MCP Task view.
pub fn status(view: &Value) -> &str {
    view["status"].as_str().unwrap_or_default()
}
