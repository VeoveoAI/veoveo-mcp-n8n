//! The server's checked contract: slug, scheme, scope, documents, the fixed resources it
//! lists and the templates it declares.
use std::sync::LazyLock;

use rmcp::model::{ServerCapabilities, ServerConfig};
use veoveo_hub_sdk::{
    docs::ServerDocs,
    server_contract::{McpServerContract, McpServerSetup},
    skills::ServerSkills,
};
use veoveo_types::ResourceScheme;

use crate::contract::{N8nProvider, N8nScope};

/// The crate documents embedded at build time and served under `n8n://docs`,
/// `n8n://docs/{doc_id}`, `n8n://contract` and the admin document routes.
pub(crate) static SERVER_DOCS: LazyLock<ServerDocs> =
    LazyLock::new(|| veoveo_hub_sdk::server_docs!("n8n"));

/// The skills under `skills/`, embedded at build time and served at `skill://n8n/…`.
pub(crate) static SERVER_SKILLS: LazyLock<ServerSkills> =
    LazyLock::new(|| veoveo_hub_sdk::server_skills!("n8n"));

pub struct N8nContract;

pub(crate) static SERVER_SETUP: LazyLock<McpServerSetup<N8nContract>> =
    LazyLock::new(|| McpServerSetup::new().expect("the n8n server's MCP contract setup"));

impl McpServerContract for N8nContract {
    type Scope = N8nScope;
    type Server = super::N8nMcp;
    type Provider = N8nProvider;

    fn scheme() -> ResourceScheme {
        ResourceScheme::parse("n8n").expect("declared n8n scheme")
    }
    fn scopes() -> &'static [N8nScope] {
        <N8nScope as veoveo_types::Vocabulary>::ALL
    }
    fn documents() -> &'static ServerDocs {
        &SERVER_DOCS
    }
    fn skills() -> &'static ServerSkills {
        &SERVER_SKILLS
    }
    fn server_config() -> ServerConfig {
        server_config()
    }
}

fn server_config() -> ServerConfig {
    let mut capabilities = ServerCapabilities::builder()
        .enable_resources()
        .enable_completions()
        .build();
    if let Some(registration) = super::declarations::REGISTRATION.get() {
        veoveo_hub_sdk::registration::mcp::advertise(
            &mut capabilities,
            registration,
            &super::requirements::AUTHORIZATION,
        );
    }
    let mut info = ServerConfig::default();
    info.capabilities = capabilities;
    info.server_info = rmcp::model::Implementation::new("n8n", env!("CARGO_PKG_VERSION"));
    info.instructions = Some(INSTRUCTIONS.to_owned());
    info
}

/// What the server is and which skills say how to use it.
const INSTRUCTIONS: &str = "A team's n8n workflows, each team at its own link: \
     `n8n__{link}__run` and `n8n://{link}/…`, with the overview App at ui://n8n/overview. \
     Its skills say how: skill://n8n/author-a-workflow/SKILL.md and \
     skill://n8n/run-a-workflow/SKILL.md.";

#[cfg(test)]
mod tests {
    #[test]
    fn the_instructions_name_every_skill_the_server_serves() {
        let skills = &*super::SERVER_SKILLS;
        assert_eq!(skills.skills().len(), 2);
        for skill in skills.skills() {
            assert!(super::INSTRUCTIONS.contains(&skill.uri), "{}", skill.uri);
        }
    }

    #[test]
    fn the_setup_round_trips_every_resource_and_template() {
        let setup = &*super::SERVER_SETUP;
        let resources: Vec<_> = setup
            .resources()
            .iter()
            .map(|resource| resource.descriptor().uri.clone())
            .collect();
        for uri in [
            "n8n://workflows",
            "n8n://status",
            "ui://n8n/overview",
            "n8n://contract",
            "n8n://docs",
        ] {
            assert!(
                resources.iter().any(|listed| listed == uri),
                "{uri}: {resources:?}"
            );
        }
        let templates: Vec<_> = setup
            .resource_templates()
            .iter()
            .map(|template| template.descriptor().uri_template.clone())
            .collect();
        assert!(
            templates
                .iter()
                .any(|template| template == "n8n://workflow/{workflow}"),
            "{templates:?}"
        );
    }
}
