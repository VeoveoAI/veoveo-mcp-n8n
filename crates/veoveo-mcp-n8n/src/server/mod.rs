//! The hosted process: read the branch's instance and workflows, register with the Hub,
//! connect with the registration's database login, hold a process instance, migrate,
//! start n8n as the child process, apply the workflows, then serve MCP, the hooks, the
//! editor and the Veoveo nodes' loopback listener and run the Task worker until SIGTERM
//! or Ctrl-C, until the instance loses its lock or until n8n exits.
mod config;
mod registration;

use std::{net::SocketAddr, sync::Arc};

use anyhow::{Context, Result};
use clap::Parser;
use tokio_util::sync::CancellationToken;
use url::Url;
use veoveo_hub_sdk::{
    PublicDeployment, TelemetryGuard,
    apps::AppHtml,
    durable_tasks::DurableTasks,
    hosting::{Hosted, HostedServer},
    init_server_telemetry,
};
use veoveo_store::{Instance as ProcessInstance, PlatformStore, ServerSchema, StoreConfig};
use veoveo_task::TaskRuntime;

use crate::{
    bridge::Bridge,
    editor::{self, EditorSession},
    hooks,
    instance::Instance,
    loopback,
    mcp::{self, N8nMcp, Served},
    n8n::{N8n, N8nClient, N8nDatabase, N8nSettings, STOP_GRACE, StateDirectory, generate_secret},
    runs::{N8nTasks, Runner},
    store::{InstanceKeyStore, RunStore, WorkflowStore, stored_tenant},
    workflows::{self, WorkflowFile},
};

use config::Args;
use registration::Registered;

/// What the registration issued that serving uses.
struct Issued {
    registration: veoveo_hub_sdk::registration::client::Registration,
    http: reqwest::Client,
    calls: veoveo_hub_sdk::hosting::ServerCalls,
}

pub async fn run() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let _ = dotenvy::dotenv();
    let args = Args::parse();
    let deployment = args.public_deployment()?;
    let declaration = mcp::registration(&deployment, args.deployment_id)?;
    mcp::REGISTRATION
        .set(declaration.clone())
        .map_err(|_| anyhow::anyhow!("the registration was declared twice"))?;
    mcp::force_setup();
    if args.describe_registration {
        let metadata = registration::prepare(&args, declaration)?;
        serde_json::to_writer_pretty(std::io::stdout().lock(), &metadata)?;
        println!();
        return Ok(());
    }
    let _telemetry: TelemetryGuard =
        init_server_telemetry("veoveo-mcp-n8n", "info,veoveo_mcp_n8n=debug")?;
    // The branch's own inputs are checked before the server contacts anything.
    let instance = Instance::load(&args.instance)?;
    let files = workflows::read_directory(&args.workflows)?;
    let app = AppHtml::load(&args.editor_app)?;
    tracing::info!(
        tenant = instance.tenant.as_str(),
        workflows = files.len(),
        nodes = instance.nodes.len(),
        credentials = instance.credentials.len(),
        "read the branch's instance and workflows"
    );
    let Registered {
        client,
        registration,
        http,
        calls,
    } = registration::register(&args, declaration).await?;
    // Process stop preserves saved native registration; explicit retirement and the
    // Hub's existing orphan grace remain separate.
    let issued = Issued {
        registration: registration.clone(),
        http,
        calls,
    };
    let mut served = None;
    let maintained = client
        .maintain(registration, async {
            served = Some(serve(&args, &deployment, &issued, &instance, &files, app).await);
        })
        .await;
    if let Err(error) = maintained {
        tracing::warn!(%error, "registration lifecycle failed");
    }
    served.context("the registration ended before serving finished")?
}

/// Connect with the registration's database login, migrate the server's schema, start
/// n8n and apply the branch's workflows, then serve and run Tasks until SIGTERM or
/// Ctrl-C, until the process instance loses its lock or until n8n exits.
async fn serve(
    args: &Args,
    deployment: &PublicDeployment,
    registered: &Issued,
    instance: &Instance,
    files: &[WorkflowFile],
    app: AppHtml,
) -> Result<()> {
    let registration = &registered.registration;
    let grant = registration
        .database()
        .context("the Hub issued no database login")?;
    let public_base = registration
        .public_base()
        .context("the Hub named no public base for the server's hooks")?;
    let app_origin = registration
        .app_origin()
        .context("the Hub named no app origin for n8n's editor")?;
    let store = PlatformStore::connect(
        StoreConfig::builder(grant.url.as_str(), grant.password.clone())
            .username(grant.username.as_str())
            .build()?,
    )
    .await
    .context("connecting to the installation database")?;
    let process = Arc::new(
        ProcessInstance::start(&store, &crate::server())
            .await
            .context("starting the process instance")?,
    );
    store
        .migrate_server(&crate::server(), crate::migrator())
        .await
        .context("applying the server's migrations")?;
    let tasks = TaskRuntime::new(
        store.clone(),
        crate::server(),
        registration.access_tokens().deployment,
        process.clone(),
    );

    let schema = ServerSchema::new(&crate::server())?;
    let hooks_url = public_base.join(["hooks", ""]);
    let editor_url = app_origin.url().clone();
    let state = StateDirectory::prepare(&args.n8n_state)
        .with_context(|| format!("preparing {}", args.n8n_state.display()))?;
    let settings = N8nSettings {
        program: args.n8n_program.clone(),
        user_folder: state.user_folder(),
        files: args.n8n_files.clone(),
        database: N8nDatabase::from_grant(
            &Url::parse(grant.url.as_str()).context("the database URL")?,
            grant.username.as_str(),
            grant.password.clone(),
            args.database_roots.as_deref(),
        )?,
        schema: schema.schema().to_owned(),
        group_role: schema.group_role(),
        editor_url: editor_url.clone(),
        webhook_url: hooks_url.clone(),
        nodes: instance.nodes.clone(),
        veoveo_nodes: std::fs::canonicalize(&args.n8n_nodes)
            .with_context(|| format!("finding the Veoveo nodes at {}", args.n8n_nodes.display()))?,
    };
    let tenant = stored_tenant(&instance.tenant)?;
    // n8n's key lives with n8n's data, which outlives the sandbox's disk; the owner's
    // password is new at every start, and n8n applies it from the environment.
    let (key, origin) = InstanceKeyStore::new(store.clone())
        .encryption_key(tenant, state.kept_encryption_key()?)
        .await?;
    tracing::info!(?origin, instance = %key.instance, "n8n's encryption key");
    let password = generate_secret()?;
    let mut n8n = N8n::start(
        &settings,
        &state,
        key.encryption_key,
        &password,
        &instance.credentials,
        |name| std::env::var(name).ok(),
    )
    .await
    .context("starting n8n")?;
    let workflows = WorkflowStore::new(store.clone());
    let runs = RunStore::new(store.clone());
    let applied = workflows::apply(n8n.client(), &workflows, tenant, files).await;
    let applied = match applied {
        Ok(decisions) => decisions,
        Err(error) => {
            n8n.stop(STOP_GRACE).await;
            return Err(error);
        }
    };
    tracing::info!(decisions = applied.len(), "applied the branch's workflows");

    let stop = CancellationToken::new();
    let runner = Runner {
        runtime: tasks.clone(),
        workflows: workflows.clone(),
        runs: runs.clone(),
        n8n: n8n.client().clone(),
        tenant,
        instance: key.instance,
        loopback: loopback::base(),
    };
    let mut working = tokio::spawn(
        runner
            .clone()
            .work(Some(registered.calls.clone()), stop.clone()),
    );
    let bridge = Bridge::new(tasks.clone(), runs, tenant, Some(registered.calls.clone()));
    let loopback_stop = stop.clone();
    let mut listening = tokio::spawn(loopback::serve(
        loopback::router(tasks.clone(), bridge),
        async move { loopback_stop.cancelled().await },
    ));
    let session = Arc::new(EditorSession::new(
        N8nClient::new(N8nSettings::base_url())?,
        password,
    ));

    let served = Served {
        tenant: instance.tenant.clone(),
        editor_url,
        hooks_url,
    };
    let client = n8n.client().clone();
    let ready_process = process.clone();
    let task_service = N8nTasks::new(tasks.clone(), workflows.clone(), instance.tenant.clone());
    let deployment = deployment
        .clone()
        .with_public_base(Some(public_base.clone()));
    let hosted = HostedServer::for_domain::<N8nMcp>()
        .deployment(&deployment, args.allow_loopback_hosts)?
        .allowed_hosts(args.allowed_hosts.iter().cloned())
        .internal_trust(registration.access_tokens(), registered.http.clone())?
        .handler(move || {
            Hosted::new(N8nMcp::new(
                workflows.clone(),
                client.clone(),
                served.clone(),
                app.clone(),
            ))
            .with_tasks(DurableTasks::tasks_only(task_service.clone()))
        })
        .calls(registered.calls.clone())
        .public_routes(hooks::router(N8nSettings::base_url()))
        .authenticated_routes(editor::router(
            N8nSettings::base_url(),
            session,
            Some(app_origin.host_authority()),
        ))
        .app_origin(Some(app_origin.clone()))
        // The SDK's anonymous /readyz is installed only once n8n is ready and the
        // branch's workflows are applied.
        .readiness(move || {
            let process = ready_process.clone();
            async move { !process.is_lost() }
        })
        .build();
    let served = tokio::select! {
        served = hosted.serve(SocketAddr::from(([0, 0, 0, 0], args.port))) => served,
        () = process.lost() => Err(anyhow::anyhow!("the process instance lost its lock")),
        status = n8n.exited() => Err(match status {
            Ok(status) => anyhow::anyhow!("n8n exited with {status}"),
            Err(error) => anyhow::anyhow!("waiting for n8n failed: {error}"),
        }),
        worked = &mut working => Err(match worked {
            Ok(Ok(())) => anyhow::anyhow!("the Task worker stopped"),
            Ok(Err(error)) => error.context("the Task worker stopped claiming runs"),
            Err(error) => anyhow::Error::from(error).context("the Task worker failed"),
        }),
        listened = &mut listening => Err(match listened {
            Ok(Ok(())) => anyhow::anyhow!("the loopback listener stopped"),
            Ok(Err(error)) => error.context("the loopback listener failed"),
            Err(error) => anyhow::Error::from(error).context("the loopback listener failed"),
        }),
    };
    // The worker claims nothing more and lets running dispatches settle for up to its
    // drain time before n8n stops.
    stop.cancel();
    if !working.is_finished() {
        let _ = working.await;
    }
    n8n.stop(STOP_GRACE).await;
    if let Err(error) = process.shutdown().await {
        tracing::warn!(%error, "the process instance did not stop cleanly");
    }
    store.close().await;
    served
}
