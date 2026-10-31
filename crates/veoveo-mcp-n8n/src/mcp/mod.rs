//! One typed route table binds tools, reads, the editor's App, discovery, current
//! permissions and the server's surfaces: the hooks, a public route prefix, and n8n's
//! editor on the app origin.
mod app;
mod declarations;
mod requirements;
mod resources;
mod setup;
mod tools;

use std::sync::LazyLock;

use rmcp::ErrorData as McpError;
use url::Url;
use veoveo_hub_sdk::{
    HubInternalIdentity,
    apps::AppHtml,
    hosting::{DomainServer, RoutePolicy, Routes},
    server_contract::McpServerSetup,
};

use crate::{
    bridge::BridgeTool,
    contract::N8nPermission,
    hooks,
    n8n::{N8nClient, N8nError},
    store::{StoreError, WorkflowStore, stored_tenant},
};

pub use app::{EDITOR_APP_URI, editor_app};
pub use declarations::{REGISTRATION, registration};
pub use requirements::AUTHORIZATION;
pub use setup::N8nContract;
use setup::SERVER_SETUP;

pub fn force_setup() {
    LazyLock::force(&SERVER_SETUP);
}

/// The one tenant the branch serves and where its public surfaces are.
#[derive(Clone)]
pub struct Served {
    pub tenant: veoveo_types::TenantId,
    /// The root of the server's app origin, where n8n's editor is.
    pub editor_url: Url,
    /// The public URL of the hooks, `{public base}/hooks/`.
    pub hooks_url: Url,
}

#[derive(Clone)]
pub struct N8nMcp {
    workflows: WorkflowStore,
    n8n: N8nClient,
    served: Served,
    app: AppHtml,
}

impl N8nMcp {
    pub fn new(workflows: WorkflowStore, n8n: N8nClient, served: Served, app: AppHtml) -> Self {
        Self {
            workflows,
            n8n,
            served,
            app,
        }
    }

    /// The stored tenant of a caller of the branch's own tenant; another tenant's caller
    /// is refused.
    fn tenant(&self, identity: &HubInternalIdentity) -> Result<veoveo_store::TenantId, McpError> {
        if identity.authority.tenant != self.served.tenant {
            return Err(McpError::invalid_request(
                "this n8n instance serves another tenant",
                None,
            ));
        }
        stored_tenant(&identity.authority.tenant).map_err(|_| {
            McpError::invalid_request("the forwarded identity names no stored tenant", None)
        })
    }
}

fn policy(permission: N8nPermission) -> RoutePolicy {
    RoutePolicy::permission(permission).expect("n8n permission")
}

static ROUTES: LazyLock<Routes<N8nMcp>> = LazyLock::new(|| {
    let mut routes = Routes::<N8nMcp>::builder(policy(N8nPermission::WorkflowsRead))
        .public_route(
            hooks::PATH,
            hooks::MAX_BODY_BYTES,
            hooks::REQUESTS_PER_MINUTE,
        )
        .app_origin(policy(N8nPermission::Editor));
    for tool in BridgeTool::ALL {
        routes = routes.uses(tool.permission());
    }
    let routes = tools::register(routes);
    let routes = resources::register(routes);
    app::register(routes).build()
});

impl DomainServer for N8nMcp {
    type Contract = N8nContract;
    fn setup() -> &'static McpServerSetup<N8nContract> {
        &SERVER_SETUP
    }
    fn routes() -> &'static Routes<Self> {
        &ROUTES
    }
}

fn store_error(error: StoreError) -> McpError {
    match error {
        StoreError::Paging(
            veoveo_types::PagingError::Cursor(_) | veoveo_types::PagingError::Limit,
        ) => McpError::invalid_params(error.to_string(), None),
        failure => {
            tracing::error!(%failure, "an n8n store request failed");
            McpError::internal_error("the server could not complete the request", None)
        }
    }
}

fn n8n_error(error: N8nError) -> McpError {
    tracing::error!(%error, "an n8n request failed");
    McpError::internal_error("n8n could not answer the request", None)
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_setup_and_documents_admit() {
        std::sync::LazyLock::force(&super::SERVER_SETUP);
        assert!(super::setup::SERVER_DOCS.agent_manual().is_some());
    }
}
