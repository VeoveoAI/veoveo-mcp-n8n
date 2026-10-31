//! n8n as the server's child process: its environment, its first-start bootstrap, its
//! credentials and its loopback client. The server starts n8n once and never restarts
//! it: when n8n exits, the server exits, and OpenShell restarts the workload.
mod client;
mod credentials;
mod environment;
mod state;

use std::{process::ExitStatus, time::Duration};

use anyhow::{Context, Result, bail};
use secrecy::SecretString;
use tokio::process::Child;

pub use client::{API_KEY_LABEL, API_KEY_SCOPES, N8nClient, N8nError, N8nWorkflow, N8nWorkflowId};
pub use credentials::import_document;
pub use environment::{
    DatabaseTls, LISTEN_ADDRESS, N8nDatabase, N8nSecrets, N8nSettings, OWNER_EMAIL, PORT,
    TABLE_PREFIX, WITHHELD_PREFIXES,
};
pub use state::{StateDirectory, generate_secret};

use crate::instance::CredentialMapping;

/// How long n8n has to answer readiness after it starts; its first start runs its
/// migrations.
pub const READINESS_TIMEOUT: Duration = Duration::from_secs(300);
/// How long one `import:credentials` may run.
const IMPORT_TIMEOUT: Duration = Duration::from_secs(300);
/// How long n8n has to stop after SIGTERM before it is killed.
pub const STOP_GRACE: Duration = Duration::from_secs(30);
const READINESS_POLL: Duration = Duration::from_millis(500);

/// A running n8n and the client that calls its public API with the server's key.
pub struct N8n {
    child: Child,
    client: N8nClient,
}

impl N8n {
    /// Start n8n with `settings`, wait until it is ready, take the server's API key and
    /// import `credentials`, whose secret fields hold the placeholders `placeholder`
    /// reads: in the server, the values the providers set in its environment.
    pub async fn start(
        settings: &N8nSettings,
        state: &StateDirectory,
        credentials: &[CredentialMapping],
        placeholder: impl Fn(&str) -> Option<String>,
    ) -> Result<Self> {
        let password = state
            .owner_password()
            .context("loading the owner's password")?;
        let secrets = N8nSecrets::new(
            state
                .encryption_key()
                .context("loading n8n's encryption key")?,
            &password,
        )?;
        std::fs::create_dir_all(&settings.files)
            .with_context(|| format!("creating {}", settings.files.display()))?;
        let mut child = settings
            .command(&secrets, &["start"])
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("starting {}", settings.program.display()))?;
        tracing::info!(pid = child.id(), "n8n started");
        let client = N8nClient::new(N8nSettings::base_url())?;
        wait_ready(&mut child, &client).await?;
        let client = api_client(&client, state, &password).await?;
        let mut n8n = Self { child, client };
        if !credentials.is_empty() {
            import_credentials(settings, &secrets, state, credentials, placeholder).await?;
        }
        if let Some(status) = n8n.child.try_wait()? {
            bail!("n8n exited with {status} during its bootstrap");
        }
        tracing::info!(credentials = credentials.len(), "n8n is ready");
        Ok(n8n)
    }

    /// The client of n8n's public API, with the server's key.
    pub fn client(&self) -> &N8nClient {
        &self.client
    }

    /// Wait until n8n exits.
    pub async fn exited(&mut self) -> std::io::Result<ExitStatus> {
        self.child.wait().await
    }

    /// Stop n8n: SIGTERM, then a kill after `grace`.
    pub async fn stop(mut self, grace: Duration) {
        if let Ok(Some(_)) = self.child.try_wait() {
            return;
        }
        if let Some(pid) = self.child.id().and_then(|pid| i32::try_from(pid).ok()) {
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(pid),
                nix::sys::signal::Signal::SIGTERM,
            );
        }
        match tokio::time::timeout(grace, self.child.wait()).await {
            Ok(Ok(status)) => tracing::info!(%status, "n8n stopped"),
            Ok(Err(error)) => tracing::warn!(%error, "waiting for n8n to stop failed"),
            Err(_) => {
                tracing::warn!(?grace, "n8n did not stop in time; killing it");
                let _ = self.child.kill().await;
            }
        }
    }
}

async fn wait_ready(child: &mut Child, client: &N8nClient) -> Result<()> {
    let started = tokio::time::Instant::now();
    loop {
        if client.is_ready().await {
            tracing::info!(elapsed = ?started.elapsed(), "n8n answered readiness");
            return Ok(());
        }
        if started.elapsed() > READINESS_TIMEOUT {
            bail!("n8n did not answer /healthz/readiness within {READINESS_TIMEOUT:?}");
        }
        tokio::select! {
            status = child.wait() => {
                bail!("n8n exited with {} before it was ready", status?);
            }
            () = tokio::time::sleep(READINESS_POLL) => {}
        }
    }
}

/// The client with the server's API key: the kept one while n8n accepts it, otherwise a
/// new one, minted as the owner and kept. n8n refuses a kept key on first start and when
/// the branch's data environment holds a different n8n than the sandbox's disk.
async fn api_client(
    client: &N8nClient,
    state: &StateDirectory,
    password: &SecretString,
) -> Result<N8nClient> {
    if let Some(key) = state.api_key().context("reading the kept API key")? {
        let kept = client.with_api_key(key);
        if kept.accepts_api_key().await? {
            return Ok(kept);
        }
        tracing::info!("n8n does not know the kept API key; minting a new one");
    }
    let session = client
        .login(OWNER_EMAIL, password)
        .await
        .context("logging in as n8n's owner")?;
    let key = client
        .mint_api_key(&session)
        .await
        .context("minting the server's API key")?;
    state.keep_api_key(&key).context("keeping the API key")?;
    let minted = client.with_api_key(key);
    if !minted.accepts_api_key().await? {
        bail!("n8n refuses the API key it just minted");
    }
    tracing::info!(label = API_KEY_LABEL, "minted the server's n8n API key");
    Ok(minted)
}

/// Import `credentials` with `n8n import:credentials` from a plaintext document of mode
/// 0600 that exists only while the import runs.
async fn import_credentials(
    settings: &N8nSettings,
    secrets: &N8nSecrets,
    state: &StateDirectory,
    credentials: &[CredentialMapping],
    placeholder: impl Fn(&str) -> Option<String>,
) -> Result<()> {
    let document = import_document(credentials, placeholder)?;
    let path = state.credentials_file();
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("the credentials file has a name")?;
    state.write_private(name, &document.to_string())?;
    let input = format!("--input={}", path.display());
    let imported = async {
        let mut command = settings.command(secrets, &["import:credentials", &input]);
        let status = tokio::time::timeout(IMPORT_TIMEOUT, command.kill_on_drop(true).status())
            .await
            .context("n8n import:credentials did not finish in time")??;
        if !status.success() {
            bail!("n8n import:credentials exited with {status}");
        }
        Ok(())
    }
    .await;
    let removed = std::fs::remove_file(&path);
    imported?;
    removed.with_context(|| format!("removing {}", path.display()))?;
    tracing::info!(
        credentials = credentials.len(),
        "imported the instance's credentials"
    );
    Ok(())
}
