//! The server's registration declaration. The server registers it with the Hub and
//! advertises the same value in MCP Discover, so both have one source.
use std::sync::OnceLock;

use veoveo_hub_sdk::{
    PublicDeployment,
    hosting::DomainServer,
    registration::{RegistrationDeclaration, ServerEndpoint, ServerVersion},
    server_contract::McpServerContract,
};

use super::{N8nMcp, setup::N8nContract};

/// The registration the server advertises. Startup sets it from the deployment before
/// the MCP setup is built; until then Discover advertises no declarations.
pub static REGISTRATION: OnceLock<RegistrationDeclaration> = OnceLock::new();

/// The server's identity, the URL where the Hub reaches its MCP endpoint,
/// `{public_base_url}/n8n/mcp`, and the surface its routes declare: the hooks.
pub fn registration(
    deployment: &PublicDeployment,
    deployment_id: veoveo_hub_sdk::DeploymentId,
) -> anyhow::Result<RegistrationDeclaration> {
    let endpoint = deployment.server(crate::server().as_str())?;
    let url = url::Url::parse(&format!("{}/mcp", endpoint.public_url()))?;
    Ok(RegistrationDeclaration {
        deployment: deployment_id,
        slug: crate::server(),
        uri_scheme: N8nContract::scheme(),
        version: ServerVersion::new(env!("CARGO_PKG_VERSION"))?,
        endpoint: ServerEndpoint::new(url)?,
        surfaces: N8nMcp::routes().surfaces(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_registration_declares_the_mcp_url_and_the_hooks() {
        let deployment = PublicDeployment::new("http://vv-n8n.internal:8811").unwrap();
        let registration = registration(
            &deployment,
            veoveo_hub_sdk::hosting::testing::deployment_id(),
        )
        .unwrap();
        assert_eq!(
            registration.endpoint.url().as_str(),
            "http://vv-n8n.internal:8811/n8n/mcp"
        );
        assert_eq!(registration.slug.as_str(), "n8n");
        assert_eq!(registration.uri_scheme.as_str(), "n8n");
        let surfaces = serde_json::to_value(registration.surfaces.unwrap()).unwrap();
        assert_eq!(
            surfaces,
            serde_json::json!({
                "publicRoutes": [
                    {"prefix": "/n8n/hooks/", "maxBodyBytes": 1_048_576, "requestsPerMinute": 600}
                ]
            })
        );
    }
}
