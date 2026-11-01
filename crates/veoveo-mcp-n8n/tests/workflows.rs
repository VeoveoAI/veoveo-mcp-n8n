//! Against a real n8n: the server starts n8n on a migrated database as one of its logins,
//! which has no CREATE privilege on the database, takes its API key, reads n8n's version,
//! imports a credential, then applies a team's `workflows/` by file name: creates and
//! publishes, keeps, updates, replaces a version saved in n8n and archives a removed file;
//! after a restart it reuses its key; on a new sandbox's empty disk it starts with the key
//! the database keeps, a new owner password and a newly minted API key; it applies the
//! team template's examples and the tests' team's workflows with the Veoveo nodes, and runs
//! `hello-run` as an MCP Task through the SDK's in-process mesh, whose Veoveo Result
//! completes it and whose execution is recorded as the Task caller's. The n8n program is
//! `VEOVEO_N8N_PROGRAM`, or `n8n` on `PATH`; the test also needs the PostgreSQL 18 server
//! programs on `PATH` and port 5678 on loopback. n8n reaches the server's loopback
//! listener at `VEOVEO_N8N_TEST_LOOPBACK_HOST`, `127.0.0.1` unless n8n runs where the
//! test's loopback is another host's, as in a container.
use std::{collections::BTreeSet, path::PathBuf, sync::Arc, time::Duration};

use secrecy::ExposeSecret;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use url::Url;
use veoveo_fixtures::store::TestDb;
use veoveo_hub_sdk::{
    apps::AppHtml,
    durable_tasks::DurableTasks,
    hosting::{Hosted, testing},
};
use veoveo_mcp_n8n::{
    bridge::Bridge,
    contract::WorkflowName,
    instance::{Admitted, Instance},
    loopback,
    mcp::{AUTHORIZATION, N8nMcp, Served},
    migrator,
    n8n::{
        API_KEY_SCOPES, N8n, N8nDatabase, N8nSettings, N8nStorage, STOP_GRACE, StateDirectory,
        UNKNOWN_VERSION, generate_secret,
    },
    runs::{N8nTasks, Runner},
    server,
    store::{InstanceKeyStore, KeyOrigin, RunStore, WorkflowStore, stored_tenant},
    workflows::{self, Decision, WorkflowFile},
};
use veoveo_store::ServerSchema;
use veoveo_task::{Instance as ProcessInstance, TaskRuntime};

const REPOSITORY: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
/// The tests' team, whose instance the parent's `n8n/admitted.toml` admits.
const TEAM: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/team");
const CREDENTIAL: &str = "veoveoHeader001";

fn name(value: &str) -> WorkflowName {
    WorkflowName::parse(value).unwrap()
}

/// The team template's example webhook workflow, with its greeting replaced by
/// `greeting`.
fn hello(greeting: &str) -> WorkflowFile {
    let path = PathBuf::from(REPOSITORY).join("templates/team/workflows/hello-veoveo.json");
    let mut body: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    body["nodes"][1]["parameters"]["assignments"]["assignments"][0]["value"] = json!(greeting);
    WorkflowFile::parse(
        name("hello-veoveo"),
        &serde_json::to_vec_pretty(&body).unwrap(),
    )
    .unwrap()
}

/// What n8n's production webhook of the example answers, once n8n registered it.
async fn call_hello() -> Value {
    let http = reqwest::Client::new();
    let url = N8nSettings::base_url()
        .join("webhook/hello-veoveo")
        .unwrap();
    for _ in 0..40 {
        let response = http.post(url.clone()).body("{}").send().await.unwrap();
        if response.status().is_success() {
            return serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("n8n never answered the production webhook");
}

fn decided(decisions: &[Decision]) -> Vec<&'static str> {
    decisions
        .iter()
        .map(|decision| match decision {
            Decision::Create { .. } => "create",
            Decision::Update {
                replaced: Some(_), ..
            } => "update replacing a version saved in n8n",
            Decision::Update {
                unarchive: true, ..
            } => "update restoring",
            Decision::Update { .. } => "update",
            Decision::Publish { .. } => "publish",
            Decision::Keep { .. } => "keep",
            Decision::Archive { .. } => "archive",
            Decision::Forget { .. } => "forget",
        })
        .collect()
}

#[tokio::test]
async fn n8n_runs_on_the_servers_login_and_applies_workflows_by_file_name() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    tokio::time::timeout(Duration::from_secs(600), async {
        let db = TestDb::new().await;
        db.migrate_server(&server(), migrator()).await;
        let login = db.server_store(&server()).await;
        let config = login.config().clone();
        let schema = ServerSchema::new(&server()).unwrap();
        let admitted = Admitted::load(&PathBuf::from(REPOSITORY).join("n8n/admitted.toml")).unwrap();
        let instance =
            Instance::load(&PathBuf::from(TEAM).join("n8n/instance.toml"), &admitted).unwrap();
        let credentials = Instance::parse(&format!(
            "tenant = \"tenant-a\"\nnodes = [\"n8n-nodes-base.set\"]\n\
             [[credentials]]\nid = \"{CREDENTIAL}\"\nname = \"Header\"\ntype = \"httpHeaderAuth\"\n\
             data = {{ name = \"Authorization\" }}\nsecrets = {{ value = \"EXAMPLE_TOKEN\" }}\n"
        ))
        .unwrap()
        .credentials;
        let directory = tempfile::tempdir().unwrap();
        let state = StateDirectory::prepare(directory.path().join("state/n8n")).unwrap();
        let mut settings = N8nSettings {
            program: std::env::var_os("VEOVEO_N8N_PROGRAM")
                .map_or_else(|| PathBuf::from("n8n"), PathBuf::from),
            user_folder: state.user_folder(),
            files: directory.path().join("n8n-files"),
            storage: N8nStorage::Postgres {
                database: N8nDatabase::from_grant(
                    config.url(),
                    config.username(),
                    config.password().clone(),
                    None,
                )
                .unwrap(),
                schema: schema.schema().to_owned(),
                group_role: schema.group_role(),
            },
            webhook_url: Url::parse(
                "https://hub.veoveo.test/instances/019a5b30-0000-7000-8000-000000000001/n8n/hooks/",
            )
            .unwrap(),
            nodes: Some(instance.nodes.clone()),
            veoveo_nodes: PathBuf::from(REPOSITORY)
                .join("nodes/n8n-nodes-veoveo")
                .canonicalize()
                .unwrap(),
            editor: None,
            api_key_scopes: API_KEY_SCOPES,
            extra: Vec::new(),
            withheld: &[],
        };

        // n8n's key lives in the database with its data: a new n8n's is generated. A disk
        // that outlived a reset of n8n's data holds n8n's settings file with a stale key,
        // which n8n would refuse; the server makes it name the database's key first.
        let keys = InstanceKeyStore::new(login.clone());
        let tenant =
            stored_tenant(&veoveo_types::TenantId::parse("tenant-a").unwrap()).unwrap();
        let (instance_key, origin) = keys.encryption_key(tenant).await.unwrap();
        assert_eq!(origin, KeyOrigin::Generated);
        std::fs::create_dir_all(state.user_folder().join(".n8n")).unwrap();
        std::fs::write(
            state.n8n_settings_file(),
            r#"{"encryptionKey":"a-stale-key-of-n8n-s-earlier-data"}"#,
        )
        .unwrap();
        let started = tokio::time::Instant::now();
        let n8n = N8n::start(
            &settings,
            &state,
            instance_key.encryption_key.clone(),
            &generate_secret().unwrap(),
            &credentials,
            |variable| Some(format!("placeholder-for-{variable}")),
        )
        .await
        .unwrap();
        println!(
            "n8n {} ready, keyed and credentialed in {:?}",
            n8n.version(),
            started.elapsed()
        );
        assert_ne!(n8n.version(), UNKNOWN_VERSION);
        let settings_file: Value =
            serde_json::from_str(&std::fs::read_to_string(state.n8n_settings_file()).unwrap())
                .unwrap();
        assert_eq!(
            settings_file["encryptionKey"],
            instance_key.encryption_key.expose_secret()
        );
        let key = state.api_key().unwrap().unwrap();
        assert!(!key.expose_secret().is_empty());

        // n8n migrated as the server's login, which may not create in the database, and
        // every table it made is prefixed and belongs to the server's group role.
        let creates: bool = sqlx::query_scalar(
            "SELECT pg_catalog.has_database_privilege(current_user, pg_catalog.current_database(), 'CREATE')",
        )
        .fetch_one(login.pool())
        .await
        .unwrap();
        assert!(!creates, "the server's login may create in the database");
        let tables: Vec<(String, String)> = sqlx::query_as(
            "SELECT tablename::text, tableowner::text FROM pg_catalog.pg_tables WHERE schemaname = 'n8n'",
        )
        .fetch_all(db.a.pool())
        .await
        .unwrap();
        let own = BTreeSet::from([
            "_sqlx_migrations",
            "applied_workflow",
            "instance_key",
            "run",
            "run_execution",
        ]);
        let n8n_tables: Vec<_> = tables
            .iter()
            .filter(|(table, _)| !own.contains(table.as_str()))
            .collect();
        println!("n8n made {} tables in schema n8n", n8n_tables.len());
        assert!(n8n_tables.len() > 100, "{tables:?}");
        for (table, owner) in &tables {
            assert_eq!(owner, "server_n8n", "{table}");
            assert!(table.starts_with("n8n_") || own.contains(table.as_str()), "{table}");
        }
        let in_public: i64 = sqlx::query_scalar(
            "SELECT pg_catalog.count(*) FROM pg_catalog.pg_tables WHERE schemaname = 'public'",
        )
        .fetch_one(db.a.pool())
        .await
        .unwrap();
        assert_eq!(in_public, 0);
        // The credential arrived with its placeholder and n8n stored it encrypted.
        let stored: String =
            sqlx::query_scalar("SELECT data FROM n8n.n8n_credentials_entity WHERE id = $1")
                .bind(CREDENTIAL)
                .fetch_one(login.pool())
                .await
                .unwrap();
        assert!(!stored.contains("placeholder-for-EXAMPLE_TOKEN"));
        assert!(!state.credentials_file().exists());

        let store = WorkflowStore::new(login.clone());
        let client = n8n.client().clone();

        // The team's file is created and published, and its webhook runs.
        let first = hello("Hello from Veoveo");
        let applied = workflows::apply(&client, &store, tenant, std::slice::from_ref(&first))
            .await
            .unwrap();
        assert_eq!(decided(&applied), ["create"]);
        let recorded = store.workflow(tenant, &first.workflow).await.unwrap().unwrap();
        assert_eq!(recorded.digest, first.digest);
        let in_n8n = client.workflow(&recorded.n8n_id).await.unwrap().unwrap();
        assert!(in_n8n.is_active());
        assert_eq!(in_n8n.version_id, recorded.version);
        assert_eq!(call_hello().await["greeting"], "Hello from Veoveo");

        // An unchanged start keeps it.
        let applied = workflows::apply(&client, &store, tenant, std::slice::from_ref(&first))
            .await
            .unwrap();
        assert_eq!(decided(&applied), ["keep"]);

        // A changed file updates it in place.
        let second = hello("Hello again");
        let applied = workflows::apply(&client, &store, tenant, std::slice::from_ref(&second))
            .await
            .unwrap();
        assert_eq!(decided(&applied), ["update"]);
        let updated = store.workflow(tenant, &second.workflow).await.unwrap().unwrap();
        assert_eq!(updated.n8n_id, recorded.n8n_id);
        assert_eq!(updated.digest, second.digest);
        assert_ne!(updated.version, recorded.version);
        assert_eq!(call_hello().await["greeting"], "Hello again");

        // A version saved in n8n since is replaced by the team's file.
        client
            .update_workflow(&updated.n8n_id, &hello("Edited in n8n").body)
            .await
            .unwrap();
        let applied = workflows::apply(&client, &store, tenant, std::slice::from_ref(&second))
            .await
            .unwrap();
        assert_eq!(decided(&applied), ["update replacing a version saved in n8n"]);
        assert_eq!(call_hello().await["greeting"], "Hello again");

        // The file's removal archives the workflow and forgets it.
        let applied = workflows::apply(&client, &store, tenant, &[]).await.unwrap();
        assert_eq!(decided(&applied), ["archive"]);
        let archived = client.workflow(&updated.n8n_id).await.unwrap().unwrap();
        assert!(archived.is_archived && !archived.is_active());
        assert!(store.workflow(tenant, &second.workflow).await.unwrap().is_none());
        let applied = workflows::apply(&client, &store, tenant, &[]).await.unwrap();
        assert!(applied.is_empty());
        n8n.stop(STOP_GRACE).await;

        // A restart reads n8n's key from the database, gives the owner a new password,
        // which n8n applies, reuses the kept API key and keeps what the team's file
        // already applied.
        let (kept, origin) = keys.encryption_key(tenant).await.unwrap();
        assert_eq!(origin, KeyOrigin::Kept);
        assert_eq!(kept.instance, instance_key.instance);
        let restarted = N8n::start(
            &settings,
            &state,
            kept.encryption_key,
            &generate_secret().unwrap(),
            &[],
            |_| None,
        )
        .await
        .unwrap();
        assert_eq!(
            state.api_key().unwrap().unwrap().expose_secret(),
            key.expose_secret()
        );
        let client = restarted.client().clone();
        let applied = workflows::apply(&client, &store, tenant, std::slice::from_ref(&first))
            .await
            .unwrap();
        assert_eq!(decided(&applied), ["create"]);
        let applied = workflows::apply(&client, &store, tenant, std::slice::from_ref(&first))
            .await
            .unwrap();
        assert_eq!(decided(&applied), ["keep"]);
        assert_eq!(call_hello().await["greeting"], "Hello from Veoveo");
        restarted.stop(STOP_GRACE).await;

        // A new sandbox: its disk is empty while n8n's data lives on in the database. n8n
        // starts with the key the database keeps, the owner signs in with a new password,
        // and the server mints a new API key.
        std::fs::remove_dir_all(state.root()).unwrap();
        let state = StateDirectory::prepare(directory.path().join("new-sandbox/state/n8n")).unwrap();
        settings.user_folder = state.user_folder();
        assert!(state.api_key().unwrap().is_none());
        let (kept, origin) = keys.encryption_key(tenant).await.unwrap();
        assert_eq!(origin, KeyOrigin::Kept);
        let password = generate_secret().unwrap();
        let fresh = N8n::start(&settings, &state, kept.encryption_key, &password, &[], |_| None)
            .await
            .unwrap();
        let minted = state.api_key().unwrap().expect("a key minted on the new disk");
        assert_ne!(minted.expose_secret(), key.expose_secret());
        let client = fresh.client().clone();
        let applied = workflows::apply(&client, &store, tenant, std::slice::from_ref(&first))
            .await
            .unwrap();
        assert_eq!(decided(&applied), ["keep"]);
        assert_eq!(call_hello().await["greeting"], "Hello from Veoveo");

        // The template's examples and the tests' team's workflows apply with the Veoveo
        // nodes n8n loaded.
        let mut branch =
            workflows::read_directory(&PathBuf::from(REPOSITORY).join("templates/team/workflows"))
                .unwrap();
        branch.extend(workflows::read_directory(&PathBuf::from(TEAM).join("workflows")).unwrap());
        let applied = workflows::apply(&client, &store, tenant, &branch).await.unwrap();
        let decisions: BTreeSet<_> = applied
            .iter()
            .map(|decision| (decision.workflow().to_string(), decided(std::slice::from_ref(decision))[0]))
            .collect();
        assert!(decisions.contains(&("hello-run".to_owned(), "create")), "{decisions:?}");
        assert!(decisions.contains(&("resolve-time".to_owned(), "create")), "{decisions:?}");
        let hello_run = store.workflow(tenant, &name("hello-run")).await.unwrap().unwrap();
        assert!(hello_run.runnable);
        assert!(client.workflow(&hello_run.n8n_id).await.unwrap().unwrap().is_active());

        // `run` as an MCP Task: the worker calls the Veoveo Trigger, n8n runs the
        // workflow, and its Veoveo Result's callback completes the Task.
        let tenant_id = veoveo_types::TenantId::parse("tenant-a").unwrap();
        let mut mesh = testing::TestMesh::bind().await;
        let calls = mesh.register(AUTHORIZATION.clone());
        let routes_runtime = TaskRuntime::new(
            login.clone(),
            server(),
            testing::deployment_id(),
            Arc::new(ProcessInstance::start(&login, &server()).await.unwrap()),
        );
        let task_service = N8nTasks::new(routes_runtime, store.clone(), tenant_id.clone());
        let served = Served {
            tenant: tenant_id.clone(),
            hooks_url: settings.webhook_url.clone(),
            issuer: Url::parse(testing::TEST_ISSUER).unwrap(),
            n8n_version: fresh.version().to_owned(),
            started_at: "2026-10-31T10:00:00Z".to_owned(),
            tracked_branch: None,
        };
        let app = AppHtml::load(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/overview.html").as_ref())
            .unwrap();
        let handler_store = store.clone();
        let handler_runs = RunStore::new(login.clone());
        let handler_client = client.clone();
        mesh.serve(
            testing::for_domain::<N8nMcp>()
                .calls(calls.clone())
                .handler(move || {
                    Hosted::new(N8nMcp::new(
                        handler_store.clone(),
                        handler_runs.clone(),
                        instance_key.instance,
                        handler_client.clone(),
                        served.clone(),
                        app.clone(),
                    ))
                    .with_tasks(DurableTasks::tasks_only(task_service.clone()))
                })
                .build(),
        );
        let mesh = mesh
            .granting([veoveo_mcp_n8n::contract::N8nPermission::WorkflowsRun.name()])
            .start();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let loopback_host = std::env::var("VEOVEO_N8N_TEST_LOOPBACK_HOST")
            .unwrap_or_else(|_| "127.0.0.1".to_owned());
        let loopback_base = Url::parse(&format!(
            "http://{loopback_host}:{}/",
            listener.local_addr().unwrap().port()
        ))
        .unwrap();
        let worker_runtime = TaskRuntime::new(
            login.clone(),
            server(),
            testing::deployment_id(),
            Arc::new(ProcessInstance::start(&login, &server()).await.unwrap()),
        );
        let runs = RunStore::new(login.clone());
        let stop = CancellationToken::new();
        let runner = Runner {
            runtime: worker_runtime.clone(),
            workflows: store.clone(),
            runs: runs.clone(),
            n8n: client.clone(),
            tenant,
            instance: instance_key.instance,
            loopback: loopback_base,
        };
        let working = tokio::spawn(runner.work(Some(calls.clone()), stop.clone()));
        let serving = tokio::spawn(loopback::serve_on(
            listener,
            loopback::router(
                worker_runtime.clone(),
                Bridge::new(worker_runtime.clone(), runs, tenant, Some(calls)),
            ),
            {
                let stop = stop.clone();
                async move { stop.cancelled().await }
            },
        ));
        let started_run = tokio::time::Instant::now();
        let started = mesh
            .start_task("n8n", "run", json!({"workflow": "hello-run", "input": {"name": "Ada"}}))
            .await;
        let task = started["result"]["taskId"]
            .as_str()
            .or_else(|| started["result"]["task"]["taskId"].as_str())
            .unwrap_or_else(|| panic!("no Task started: {started}"))
            .to_owned();
        let task_id = veoveo_types::TaskId::parse(&task).unwrap();
        let completed = tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                let snapshot = worker_runtime.get(task_id).await.unwrap().unwrap();
                if snapshot.is_terminal() {
                    return snapshot;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .expect("the run never ended");
        println!("the run ended {:?} in {:?}", completed.status, started_run.elapsed());
        let result = completed.result.clone().unwrap_or_else(|| panic!("no result: {completed:?}"));
        assert_eq!(
            result["structuredContent"]["output"],
            json!([{"greeting": "Hello, Ada"}]),
            "{result}"
        );
        let execution = result["structuredContent"]["execution"].as_str().unwrap().to_owned();
        let execution = veoveo_mcp_n8n::contract::ExecutionId::parse(execution).unwrap();
        let recorded = RunStore::new(login.clone())
            .dispatched(tenant, instance_key.instance, &execution)
            .await
            .unwrap()
            .expect("the run recorded its caller");
        assert_eq!(recorded.caller, testing::principal().id.as_str());
        let execution = client.execution(&execution, true).await.unwrap().unwrap();
        assert_eq!(execution.status.as_str(), "success");
        stop.cancel();
        let _ = working.await;
        let _ = serving.await;

        fresh.stop(STOP_GRACE).await;

        // Without its key n8n's data cannot be read: with n8n's tables and no key in the
        // database, the start refuses to make a new one.
        sqlx::query("DELETE FROM n8n.instance_key WHERE tenant = $1")
            .bind(tenant)
            .execute(login.pool())
            .await
            .unwrap();
        let lost = keys.encryption_key(tenant).await.err().expect("a lost key");
        assert!(
            lost.to_string()
                .contains("cannot be read without its encryption key"),
            "{lost}"
        );
    })
    .await
    .expect("the n8n test exceeded 600 seconds");
}
