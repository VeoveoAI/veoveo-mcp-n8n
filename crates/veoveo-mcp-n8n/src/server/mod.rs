//! The hosted process: read the branch's instance and workflows, register with the Hub,
//! connect with the registration's database login, hold a process instance, migrate,
//! start n8n as the child process, apply the workflows, then serve MCP until SIGTERM or
//! Ctrl-C, until the instance loses its lock or until n8n exits.
mod config;
mod registration;

use std::{net::SocketAddr, sync::Arc};

use anyhow::{Context, Result};
use clap::Parser;
use url::Url;
use veoveo_hub_sdk::{
    PublicDeployment, TelemetryGuard,
    hosting::{Hosted, HostedServer},
    init_server_telemetry,
    registration::client::Registration,
};
use veoveo_store::{Instance as ProcessInstance, PlatformStore, ServerSchema, StoreConfig};

use crate::{
    instance::Instance,
    mcp::{self, N8nMcp, Served},
    n8n::{N8n, N8nDatabase, N8nSettings, STOP_GRACE, StateDirectory},
    store::{WorkflowStore, stored_tenant},
    workflows::{self, WorkflowFile},
};

use config::Args;

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
    tracing::info!(
        tenant = instance.tenant.as_str(),
        workflows = files.len(),
        nodes = instance.nodes.len(),
        credentials = instance.credentials.len(),
        "read the branch's instance and workflows"
    );
    let (client, registration, http) = registration::register(&args, declaration).await?;
    // Process stop preserves saved native registration; explicit retirement and the
    // Hub's existing orphan grace remain separate.
    let issued = registration.clone();
    let mut served = None;
    let maintained = client
        .maintain(registration, async {
            served = Some(serve(&args, &deployment, &issued, http, &instance, &files).await);
        })
        .await;
    if let Err(error) = maintained {
        tracing::warn!(%error, "registration lifecycle failed");
    }
    served.context("the registration ended before serving finished")?
}

/// Connect with the registration's database login, migrate the server's schema, start
/// n8n and apply the branch's workflows, then serve until SIGTERM or Ctrl-C, until the
/// process instance loses its lock or until n8n exits.
async fn serve(
    args: &Args,
    deployment: &PublicDeployment,
    registration: &Registration,
    http: reqwest::Client,
    instance: &Instance,
    files: &[WorkflowFile],
) -> Result<()> {
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
    };
    let mut n8n = N8n::start(&settings, &state, &instance.credentials, |name| {
        std::env::var(name).ok()
    })
    .await
    .context("starting n8n")?;
    let workflows = WorkflowStore::new(store.clone());
    let tenant = stored_tenant(&instance.tenant)?;
    let applied = workflows::apply(n8n.client(), &workflows, tenant, files).await;
    let applied = match applied {
        Ok(decisions) => decisions,
        Err(error) => {
            n8n.stop(STOP_GRACE).await;
            return Err(error);
        }
    };
    tracing::info!(decisions = applied.len(), "applied the branch's workflows");

    let served = Served {
        tenant: instance.tenant.clone(),
        editor_url,
        hooks_url,
    };
    let client = n8n.client().clone();
    let ready_process = process.clone();
    let deployment = deployment
        .clone()
        .with_public_base(Some(public_base.clone()));
    let hosted = HostedServer::for_domain::<N8nMcp>()
        .deployment(&deployment, args.allow_loopback_hosts)?
        .allowed_hosts(args.allowed_hosts.iter().cloned())
        .internal_trust(registration.access_tokens(), http)?
        .handler(move || {
            Hosted::new(N8nMcp::new(
                workflows.clone(),
                client.clone(),
                served.clone(),
            ))
        })
        .public_routes(mcp::hooks())
        .authenticated_routes(mcp::editor())
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
    };
    n8n.stop(STOP_GRACE).await;
    if let Err(error) = process.shutdown().await {
        tracing::warn!(%error, "the process instance did not stop cleanly");
    }
    store.close().await;
    served
}
