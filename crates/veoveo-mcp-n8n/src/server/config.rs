//! The server's configuration. Every flag has its environment variable. The Hub
//! issues the database login, the access-token trust, the public base and the app origin
//! when the server registers, so none of them is configured here.
use std::path::PathBuf;

use clap::Parser;
use secrecy::SecretString;
use url::Url;
use veoveo_hub_sdk::{PublicDeployment, parse_allowed_host_authority};

#[derive(Parser)]
#[command(name = "veoveo-mcp-n8n", about = "n8n MCP server")]
pub(super) struct Args {
    /// Print native public metadata without Hub, database or telemetry startup.
    #[arg(long)]
    pub describe_registration: bool,
    #[arg(long, env = "VEOVEO_DEPLOYMENT_ID")]
    pub deployment_id: veoveo_hub_sdk::DeploymentId,
    #[arg(long, env = "VEOVEO_PORT", default_value_t = 8811)]
    pub port: u16,
    /// The origin where the Hub reaches the server; it serves MCP at
    /// `{public_base_url}/n8n/mcp` and registers that URL.
    #[arg(long, env = "VEOVEO_PUBLIC_BASE_URL")]
    pub public_base_url: String,
    /// The Hub's RFC 8414 issuer; registration discovers its endpoint from there.
    #[arg(
        long,
        env = "VEOVEO_HUB_ISSUER",
        required_unless_present = "describe_registration"
    )]
    pub hub_issuer: Option<Url>,
    /// The installation's registration secret, or the placeholder the installation's edge
    /// replaces.
    #[arg(
        long,
        env = "VEOVEO_REGISTRATION_SECRET",
        required_unless_present = "describe_registration",
        hide_env_values = true,
        value_parser = parse_secret
    )]
    pub registration_secret: Option<SecretString>,
    /// Private deployment key and saved native registration retained across process restarts.
    #[arg(
        long,
        env = "VEOVEO_KEY_DIRECTORY",
        default_value = "/var/lib/veoveo/n8n/keys"
    )]
    pub key_directory: PathBuf,
    #[arg(long, env = "VEOVEO_ALLOW_LOOPBACK_HOSTS", default_value_t = false)]
    pub allow_loopback_hosts: bool,
    #[arg(
        long = "allowed-host",
        env = "VEOVEO_ALLOWED_HOSTS",
        value_name = "HOST",
        value_delimiter = ',',
        value_parser = parse_allowed_host
    )]
    pub allowed_hosts: Vec<String>,
    /// The branch's workflows, one JSON file each.
    #[arg(long, env = "VEOVEO_N8N_WORKFLOWS", default_value = "workflows")]
    pub workflows: PathBuf,
    /// The instance's tenant, admitted node types and credential map.
    #[arg(long, env = "VEOVEO_N8N_INSTANCE", default_value = "n8n/instance.toml")]
    pub instance: PathBuf,
    /// What the server keeps for n8n: the owner's password, the encryption key, the API
    /// key and n8n's user folder.
    #[arg(long, env = "VEOVEO_N8N_STATE", default_value = "/sandbox/state/n8n")]
    pub n8n_state: PathBuf,
    /// The one folder workflows may read and write files in.
    #[arg(long, env = "VEOVEO_N8N_FILES", default_value = "/sandbox/n8n-files")]
    pub n8n_files: PathBuf,
    /// The Veoveo nodes, which n8n loads through `N8N_CUSTOM_EXTENSIONS`.
    #[arg(
        long,
        env = "VEOVEO_N8N_NODES",
        default_value = "nodes/n8n-nodes-veoveo"
    )]
    pub n8n_nodes: PathBuf,
    /// The document of the editor's App, `ui://n8n/editor`.
    #[arg(
        long,
        env = "VEOVEO_N8N_EDITOR_APP",
        default_value = "crates/veoveo-mcp-n8n/assets/editor.html"
    )]
    pub editor_app: PathBuf,
    /// n8n's command line program.
    #[arg(long, env = "VEOVEO_N8N_PROGRAM", default_value = "n8n")]
    pub n8n_program: PathBuf,
    /// The platform roots that verify PostgreSQL, the bundle OpenShell names in
    /// `SSL_CERT_FILE`.
    #[arg(long, env = "SSL_CERT_FILE")]
    pub database_roots: Option<PathBuf>,
}

impl Args {
    pub fn public_deployment(&self) -> anyhow::Result<PublicDeployment> {
        PublicDeployment::new(&self.public_base_url)
    }
}

fn parse_secret(value: &str) -> Result<SecretString, String> {
    (!value.is_empty())
        .then(|| SecretString::from(value))
        .ok_or_else(|| "secret must not be empty".to_owned())
}

fn parse_allowed_host(value: &str) -> Result<String, String> {
    let value = value.trim();
    parse_allowed_host_authority(value)
        .map(|_| value.to_owned())
        .ok_or_else(|| "expected a host authority such as vv-n8n.internal:8811".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hub_and_the_secret_are_required() {
        let base = [
            "veoveo-mcp-n8n",
            "--deployment-id",
            "019a5b30-0000-7000-8000-000000000001",
            "--public-base-url",
            "http://vv-n8n.internal:8811",
        ];
        assert!(Args::try_parse_from(base).is_err());
        let args = Args::try_parse_from(base.into_iter().chain([
            "--hub-issuer",
            "http://vv-hub.internal:8080",
            "--registration-secret",
            "placeholder",
            "--allowed-host",
            "vv-n8n.internal:8811",
        ]))
        .unwrap();
        assert_eq!(args.port, 8811);
        assert_eq!(args.allowed_hosts, ["vv-n8n.internal:8811"]);
        assert_eq!(args.workflows, PathBuf::from("workflows"));
        assert_eq!(args.instance, PathBuf::from("n8n/instance.toml"));
        assert_eq!(args.n8n_state, PathBuf::from("/sandbox/state/n8n"));
        assert!(args.public_deployment().is_ok());
    }

    #[test]
    fn offline_description_requires_no_hub_or_admission_credentials() {
        let args = Args::try_parse_from([
            "veoveo-mcp-n8n",
            "--describe-registration",
            "--deployment-id",
            "019a5b30-0000-7000-8000-000000000001",
            "--public-base-url",
            "https://n8n.example",
        ])
        .unwrap();
        assert!(args.describe_registration);
        assert!(args.hub_issuer.is_none());
        assert!(args.registration_secret.is_none());
    }
}
