//! Against a real n8n: the server starts n8n on a migrated database as one of its logins,
//! which has no CREATE privilege on the database, takes its API key, imports a
//! credential, then applies `workflows/` by file name: creates and publishes, keeps,
//! updates, replaces an editor's change and archives a removed file; after a restart it
//! reuses its key. The n8n program is
//! `VEOVEO_N8N_PROGRAM`, or `n8n` on `PATH`; the test also needs the PostgreSQL 18 server
//! programs on `PATH` and port 5678 on loopback.
use std::{collections::BTreeSet, path::PathBuf, time::Duration};

use secrecy::ExposeSecret;
use serde_json::{Value, json};
use url::Url;
use veoveo_fixtures::store::TestDb;
use veoveo_mcp_n8n::{
    contract::WorkflowName,
    instance::Instance,
    migrator,
    n8n::{N8n, N8nDatabase, N8nSettings, STOP_GRACE, StateDirectory},
    server,
    store::{WorkflowStore, stored_tenant},
    workflows::{self, Decision, WorkflowFile},
};
use veoveo_store::ServerSchema;

const REPOSITORY: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
const CREDENTIAL: &str = "veoveoHeader001";

fn name(value: &str) -> WorkflowName {
    WorkflowName::parse(value).unwrap()
}

/// The repository's example workflow, with its greeting replaced by `greeting`.
fn hello(greeting: &str) -> WorkflowFile {
    let path = PathBuf::from(REPOSITORY).join("workflows/hello-veoveo.json");
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
            } => "update replacing an editor change",
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
        let instance =
            Instance::load(&PathBuf::from(REPOSITORY).join("n8n/instance.toml")).unwrap();
        let credentials = Instance::parse(&format!(
            "tenant = \"tenant-a\"\nnodes = [\"n8n-nodes-base.set\"]\n\
             [[credentials]]\nid = \"{CREDENTIAL}\"\nname = \"Header\"\ntype = \"httpHeaderAuth\"\n\
             data = {{ name = \"Authorization\" }}\nsecrets = {{ value = \"EXAMPLE_TOKEN\" }}\n"
        ))
        .unwrap()
        .credentials;
        let directory = tempfile::tempdir().unwrap();
        let state = StateDirectory::prepare(directory.path().join("state/n8n")).unwrap();
        let settings = N8nSettings {
            program: std::env::var_os("VEOVEO_N8N_PROGRAM")
                .map_or_else(|| PathBuf::from("n8n"), PathBuf::from),
            user_folder: state.user_folder(),
            files: directory.path().join("n8n-files"),
            database: N8nDatabase::from_grant(
                config.url(),
                config.username(),
                config.password().clone(),
                None,
            )
            .unwrap(),
            schema: schema.schema().to_owned(),
            group_role: schema.group_role(),
            editor_url: Url::parse("https://n8n.apps.veoveo.test/").unwrap(),
            webhook_url: Url::parse(
                "https://hub.veoveo.test/instances/019a5b30-0000-7000-8000-000000000001/n8n/hooks/",
            )
            .unwrap(),
            nodes: instance.nodes.clone(),
        };

        let started = tokio::time::Instant::now();
        let n8n = N8n::start(&settings, &state, &credentials, |variable| {
            Some(format!("placeholder-for-{variable}"))
        })
        .await
        .unwrap();
        println!("n8n ready, keyed and credentialed in {:?}", started.elapsed());
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
        let own = BTreeSet::from(["_sqlx_migrations", "applied_workflow"]);
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
        let tenant =
            stored_tenant(&veoveo_types::TenantId::parse("tenant-a").unwrap()).unwrap();
        let client = n8n.client().clone();

        // The branch's file is created and published, and its webhook runs.
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

        // A version saved in n8n since is replaced by the branch's file.
        client
            .update_workflow(&updated.n8n_id, &hello("Edited in n8n").body)
            .await
            .unwrap();
        let applied = workflows::apply(&client, &store, tenant, std::slice::from_ref(&second))
            .await
            .unwrap();
        assert_eq!(decided(&applied), ["update replacing an editor change"]);
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

        // A restart reuses the kept key and keeps what the branch's file already applied.
        let restarted = N8n::start(&settings, &state, &[], |_| None).await.unwrap();
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
    })
    .await
    .expect("the n8n test exceeded 600 seconds");
}
