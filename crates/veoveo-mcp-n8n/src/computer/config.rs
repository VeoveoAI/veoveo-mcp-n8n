//! `veoveo-n8n-computer`'s configuration, from the Computer's environment. Every flag has
//! its variable; the Computer template sets the Hub's, Lore's and the Computer's own.
use std::path::PathBuf;

use clap::Parser;
use secrecy::SecretString;
use url::Url;

#[derive(Parser)]
#[command(name = "veoveo-n8n-computer", about = "n8n in a person's own Computer")]
pub(super) struct Args {
    /// The loopback port the Hub forwards the Computer's program `n8n` to.
    #[arg(long, env = "VEOVEO_N8N_COMPUTER_PORT", default_value_t = 5680)]
    pub port: u16,
    /// The person's home: n8n's state lives under `.veoveo/n8n`, the teams' checkouts
    /// under `n8n/`.
    #[arg(long, env = "HOME")]
    pub home: PathBuf,
    /// The Computer's base path on the Hub, `/computer/c/{computer}`, under which the
    /// browser reaches the program at `/apps/n8n/`.
    #[arg(long, env = "PUBLIC_BASE_PATH")]
    pub base_path: String,
    /// The Hub's public origin.
    #[arg(long, env = "VEOVEO_CLI_HUB")]
    pub hub: Url,
    /// The Hub's MCP endpoint of the Computers profile.
    #[arg(long, env = "COMPUTER_HUB_URL")]
    pub hub_mcp: Url,
    /// The Computer's Hub credential: the placeholder the proxy replaces with the owner's
    /// token on the Hub's MCP endpoint.
    #[arg(long, env = "COMPUTER_HUB_REFERENCE", hide_env_values = true, value_parser = parse_secret)]
    pub hub_bearer: SecretString,
    /// Lore's remote, under which each repository is `{remote}/{id}`.
    #[arg(long, env = "VEOVEO_LORE_REMOTE")]
    pub lore_remote: String,
    /// The Lore command line, which acts as the owner.
    #[arg(long, env = "VEOVEO_N8N_LORE", default_value = "lore")]
    pub lore: PathBuf,
    /// n8n's command line program.
    #[arg(long, env = "VEOVEO_N8N_PROGRAM", default_value = "n8n")]
    pub n8n_program: PathBuf,
    /// The Veoveo nodes, which n8n loads through `N8N_CUSTOM_EXTENSIONS`.
    #[arg(
        long,
        env = "VEOVEO_N8N_NODES",
        default_value = "/opt/veoveo/n8n/nodes/n8n-nodes-veoveo"
    )]
    pub n8n_nodes: PathBuf,
    /// How often, in seconds, a change saved in the editor is written back to its file.
    #[arg(long, env = "VEOVEO_N8N_SYNC_SECONDS", default_value_t = 5)]
    pub sync_seconds: u64,
}

fn parse_secret(value: &str) -> Result<SecretString, String> {
    (!value.is_empty())
        .then(|| SecretString::from(value))
        .ok_or_else(|| "secret must not be empty".to_owned())
}

impl Args {
    /// The path the browser reaches the editor under: the Computer's base path, then
    /// `/apps/n8n/`.
    pub fn editor_path(&self) -> String {
        format!("{}{}", self.base_path.trim_end_matches('/'), super::MOUNT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_computer_names_the_hub_lore_and_its_base_path() {
        let args = Args::try_parse_from([
            "veoveo-n8n-computer",
            "--home",
            "/sandbox",
            "--base-path",
            "/computer/c/019a5b30-0000-7000-8000-000000000001",
            "--hub",
            "https://veoveo.example",
            "--hub-mcp",
            "https://veoveo.example/mcp/computer",
            "--hub-bearer",
            "placeholder",
            "--lore-remote",
            "grpcs://lore.example:41337",
        ])
        .unwrap();
        assert_eq!(args.port, 5680);
        assert_eq!(
            args.editor_path(),
            "/computer/c/019a5b30-0000-7000-8000-000000000001/apps/n8n/"
        );
        assert_eq!(args.sync_seconds, 5);
    }
}
