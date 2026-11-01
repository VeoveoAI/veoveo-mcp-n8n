//! `veoveo-n8n-computer`: n8n in a person's own Computer. It runs n8n on loopback with
//! SQLite under `$HOME/.veoveo/n8n`, its owner created from the environment with a
//! password it generates and keeps there, and serves the editor under the path the Hub
//! forwards to the Computer's program `n8n`, holding the owner's session itself. It
//! lists the teams the person reaches through the Hub as the owner, clones a team's
//! repository on demand at its runner's tracked branch, applies its workflows as drafts
//! and writes back what the person saves. It is the one program of the Computer that
//! calls the Hub for n8n: n8n's Veoveo Tool node calls its loopback bridge.
mod bridge;
mod config;
mod editor;
mod hub;
mod records;
mod session;
mod teams;

use std::{net::SocketAddr, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use clap::Parser;
use secrecy::ExposeSecret;
use veoveo_hub_sdk::client::{HubClient, StaticBearer};

pub use bridge::ComputerBridge;
pub use editor::{Editor, router};
pub use hub::{Team, status_link, teams as list_teams};
pub use records::TeamRecords;
pub use session::EditorSession;
pub use teams::{Checkouts, file_name_of, tag_name};

use crate::n8n::{
    COMPUTER_API_KEY_SCOPES, N8n, N8nClient, N8nEditor, N8nSettings, N8nStorage, STOP_GRACE,
    StateDirectory, generate_secret,
};

/// Where the Hub forwards the Computer's program `n8n`, after the Computer's base path.
pub const MOUNT: &str = "/apps/n8n/";
/// The variables of n8n's environment that name the Computer's bridge and its token.
pub const BRIDGE_URL: &str = "VEOVEO_BRIDGE_URL";
pub const BRIDGE_TOKEN: &str = "VEOVEO_BRIDGE_TOKEN";
/// The Computer's credential placeholders, which n8n's process never receives: only this
/// program calls the Hub, Lore and the model for n8n.
const WITHHELD: &[&str] = &[
    "COMPUTER_HUB_REFERENCE",
    "COMPUTER_HUB_URL",
    "DEEPINFRA_API_KEY",
    "LORE_ACCESS_TOKEN",
    "LORE_IDENTITY",
    "PUBLIC_BASE_PATH",
];
const OWNER_PASSWORD: &str = "owner-password";
const ENCRYPTION_KEY: &str = "encryption-key";

pub async fn run() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let args = config::Args::parse();
    let _telemetry =
        veoveo_hub_sdk::init_server_telemetry("veoveo-n8n-computer", "info,veoveo_mcp_n8n=debug")?;
    let root = args.home.join(".veoveo").join("n8n");
    let state =
        StateDirectory::prepare(&root).with_context(|| format!("preparing {}", root.display()))?;
    // One person, one Computer: the owner's password and n8n's key are kept for good.
    let password = state.kept_secret(OWNER_PASSWORD)?;
    let key = state.kept_secret(ENCRYPTION_KEY)?;
    let token = generate_secret()?;
    let editor_path = args.editor_path();
    let editor_url = args
        .hub
        .join(editor_path.trim_start_matches('/'))
        .context("the editor's public URL")?;
    let bridge_url = format!(
        "http://127.0.0.1:{}/{}/bridge/calls",
        args.port,
        crate::server().as_str()
    );
    let settings = N8nSettings {
        program: args.n8n_program.clone(),
        user_folder: state.user_folder(),
        files: root.join("files"),
        storage: N8nStorage::Sqlite,
        webhook_url: editor_url.clone(),
        nodes: None,
        veoveo_nodes: std::fs::canonicalize(&args.n8n_nodes)
            .with_context(|| format!("finding the Veoveo nodes at {}", args.n8n_nodes.display()))?,
        editor: Some(N8nEditor {
            path: editor_path.clone(),
            base_url: editor_url,
        }),
        api_key_scopes: COMPUTER_API_KEY_SCOPES,
        extra: vec![
            (BRIDGE_URL, bridge_url),
            (BRIDGE_TOKEN, token.expose_secret().to_owned()),
        ],
        withheld: WITHHELD,
    };
    let mut n8n = N8n::start(&settings, &state, key, &password, &[], |_| None)
        .await
        .context("starting n8n")?;
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let hub = HubClient::new(
        args.hub_mcp.clone(),
        http.clone(),
        StaticBearer::new(args.hub_bearer.expose_secret()),
    )
    .named("veoveo-n8n-computer", env!("CARGO_PKG_VERSION"));
    let checkouts = Arc::new(Checkouts::new(
        args.home.join("n8n"),
        root.join("teams"),
        args.lore.clone(),
        args.lore_remote.clone(),
        n8n.client().clone(),
    ));
    let session = Arc::new(EditorSession::new(
        N8nClient::new(N8nSettings::base_url())?,
        password,
    ));
    let local = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let routes = router(
        Editor {
            http: local,
            n8n: N8nSettings::base_url(),
            session,
            hub: hub.clone(),
            checkouts: checkouts.clone(),
            path: editor_path,
        },
        ComputerBridge { token, hub },
    );
    let writing = tokio::spawn(write_back(
        checkouts,
        Duration::from_secs(args.sync_seconds.max(1)),
    ));
    let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], args.port)))
        .await
        .with_context(|| format!("listening on 127.0.0.1:{}", args.port))?;
    tracing::info!(
        port = args.port,
        version = n8n.version(),
        "serving n8n in the Computer"
    );
    let served = tokio::select! {
        served = axum::serve(listener, routes).into_future() => served.context("serving the editor"),
        status = n8n.exited() => Err(match status {
            Ok(status) => anyhow::anyhow!("n8n exited with {status}"),
            Err(error) => anyhow::anyhow!("waiting for n8n failed: {error}"),
        }),
        () = terminated() => Ok(()),
    };
    writing.abort();
    n8n.stop(STOP_GRACE).await;
    served
}

/// Write back every team's saved workflows every `interval`.
async fn write_back(checkouts: Arc<Checkouts>, interval: Duration) {
    let mut ticks = tokio::time::interval(interval);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticks.tick().await;
        match checkouts.write_back_all().await {
            Ok(0) => {}
            Ok(written) => tracing::info!(written, "wrote back workflows saved in n8n"),
            Err(error) => tracing::warn!(
                error = format!("{error:#}"),
                "writing back saved workflows failed"
            ),
        }
    }
}

/// SIGTERM or Ctrl-C.
async fn terminated() {
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("a SIGTERM handler");
    tokio::select! {
        _ = term.recv() => {}
        _ = tokio::signal::ctrl_c() => {}
    }
}
