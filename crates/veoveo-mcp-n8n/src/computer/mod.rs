//! `veoveo-n8n-computer`: n8n in a person's own Computer. It runs n8n on loopback with
//! SQLite under `$HOME/.veoveo/n8n`, its owner created from the environment with a
//! password it generates and keeps there, and serves the editor under the path the Hub
//! forwards to the Computer's program `n8n`, holding the owner's session itself. It
//! lists the teams the person reaches through the Hub as the owner, clones a team's
//! repository on demand at its runner's tracked branch, applies its workflows as drafts
//! and writes back what the person saves. It is the one program of the Computer that
//! calls the Hub and the model for n8n: n8n's Veoveo Tool node calls its loopback bridge,
//! and n8n's Assistant its loopback model endpoint and its sandbox service, whose
//! sandboxes are directories of the person's home.
mod bridge;
mod config;
mod editor;
mod hub;
mod model;
mod records;
mod sandboxes;
mod session;
mod teams;

use std::{net::SocketAddr, path::Path, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use clap::Parser;
use secrecy::{ExposeSecret, SecretString};
use veoveo_hub_sdk::client::{HubClient, StaticBearer};

pub use bridge::ComputerBridge;
pub use editor::{Assistant, Editor, router};
pub use hub::{Team, status_link, teams as list_teams};
pub use model::{ComputerModel, MODEL_PATH};
pub use records::TeamRecords;
pub use sandboxes::{ComputerSandboxes, WORKSPACE};
pub use session::EditorSession;
pub use teams::{Checkouts, file_name_of, tag_name};

use crate::n8n::{
    COMPUTER_API_KEY_SCOPES, N8n, N8nClient, N8nEditor, N8nSettings, N8nStorage, OWNER_EMAIL,
    STOP_GRACE, StateDirectory, generate_secret,
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

/// n8n's Assistant in the Computer: the installation's `model` through this program's
/// loopback endpoint on `port`, the sandboxes of its sandbox service, each a home under
/// `homes`, both admitted with `token`, and no browser use. Web search is off in n8n's
/// settings, which the program sets once n8n runs.
pub fn assistant_environment(
    port: u16,
    token: &SecretString,
    model: &str,
    homes: &Path,
) -> Vec<(&'static str, String)> {
    let local = format!("http://127.0.0.1:{port}");
    let base = MODEL_PATH
        .strip_suffix("/chat/completions")
        .expect("the model's path ends in /chat/completions");
    vec![
        // n8n's `custom/` provider with a URL calls `{url}/chat/completions` with the
        // model's name after `custom/`.
        ("N8N_INSTANCE_AI_MODEL", format!("custom/{model}")),
        ("N8N_INSTANCE_AI_MODEL_URL", format!("{local}{base}")),
        (
            "N8N_INSTANCE_AI_MODEL_API_KEY",
            token.expose_secret().to_owned(),
        ),
        ("N8N_INSTANCE_AI_BROWSER_USE_ENABLED", "false".to_owned()),
        ("N8N_INSTANCE_AI_SANDBOX_ENABLED", "true".to_owned()),
        ("N8N_INSTANCE_AI_SANDBOX_PROVIDER", "n8n-sandbox".to_owned()),
        (
            "N8N_SANDBOX_SERVICE_URL",
            format!("{local}/{}", crate::server().as_str()),
        ),
        (
            "N8N_SANDBOX_SERVICE_API_KEY",
            token.expose_secret().to_owned(),
        ),
        // The fork's variable: each sandbox's home is `{homes}/{id}`.
        ("N8N_SANDBOX_SERVICE_HOMES", homes.display().to_string()),
    ]
}

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
    let sandboxes = ComputerSandboxes::new(
        args.home.join("n8n").join(".sandboxes"),
        &args.home,
        token.clone(),
        WITHHELD,
    );
    let mut extra = vec![
        (BRIDGE_URL, bridge_url),
        (BRIDGE_TOKEN, token.expose_secret().to_owned()),
    ];
    extra.extend(assistant_environment(
        args.port,
        &token,
        &args.model,
        sandboxes.homes(),
    ));
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
        extra,
        withheld: WITHHELD,
    };
    let mut n8n = N8n::start(&settings, &state, key, &password, &[], |_| None)
        .await
        .context("starting n8n")?;
    if let Err(error) = disable_assistant_search(&password).await {
        tracing::error!(
            error = format!("{error:#}"),
            "turning the Assistant's web search off failed; the Assistant stays unset"
        );
    }
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
        ComputerBridge {
            token: token.clone(),
            hub,
        },
        Assistant {
            model: ComputerModel {
                http: http.clone(),
                endpoint: args.model_endpoint.clone(),
                key: args.model_key.clone(),
                token,
            },
            sandboxes,
        },
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

/// Turn the Assistant's web search off in n8n's settings, signed in as the owner.
async fn disable_assistant_search(password: &SecretString) -> Result<()> {
    let n8n = N8nClient::new(N8nSettings::base_url())?;
    let session = n8n.login(OWNER_EMAIL, password).await?;
    n8n.disable_assistant_search(&session).await?;
    tracing::info!("turned the Assistant's web search off in n8n's settings");
    Ok(())
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn n8n_s_assistant_uses_the_program_s_model_and_sandboxes() {
        let token = SecretString::from("start-token");
        let environment: BTreeMap<_, _> = assistant_environment(
            5680,
            &token,
            "XiaomiMiMo/MiMo-V2.6-Pro",
            Path::new("/sandbox/n8n/.sandboxes"),
        )
        .into_iter()
        .collect();
        let expected = BTreeMap::from([
            ("N8N_INSTANCE_AI_MODEL", "custom/XiaomiMiMo/MiMo-V2.6-Pro"),
            (
                "N8N_INSTANCE_AI_MODEL_URL",
                "http://127.0.0.1:5680/v1/openai",
            ),
            ("N8N_INSTANCE_AI_MODEL_API_KEY", "start-token"),
            ("N8N_INSTANCE_AI_BROWSER_USE_ENABLED", "false"),
            ("N8N_INSTANCE_AI_SANDBOX_ENABLED", "true"),
            ("N8N_INSTANCE_AI_SANDBOX_PROVIDER", "n8n-sandbox"),
            ("N8N_SANDBOX_SERVICE_URL", "http://127.0.0.1:5680/n8n"),
            ("N8N_SANDBOX_SERVICE_API_KEY", "start-token"),
            ("N8N_SANDBOX_SERVICE_HOMES", "/sandbox/n8n/.sandboxes"),
        ]);
        assert_eq!(
            environment
                .iter()
                .map(|(name, value)| (*name, value.as_str()))
                .collect::<BTreeMap<_, _>>(),
            expected
        );
        // The model's credential stays with the program.
        assert!(!environment.contains_key("DEEPINFRA_API_KEY"));
        assert!(WITHHELD.contains(&"DEEPINFRA_API_KEY"));
    }
}
