//! One typed route table binds reads, discovery, current permissions and the server's
//! surfaces: the hooks, a public route prefix, and n8n's editor on the app origin.
mod declarations;
mod requirements;
mod resources;
mod setup;

use std::sync::LazyLock;

use axum::{Router, http::StatusCode, routing::any};
use rmcp::ErrorData as McpError;
use url::Url;
use veoveo_hub_sdk::{
    HubInternalIdentity,
    hosting::{DomainServer, RoutePolicy, Routes},
    server_contract::McpServerSetup,
};

use crate::{
    contract::N8nPermission,
    n8n::{N8nClient, N8nError},
    store::{StoreError, WorkflowStore, stored_tenant},
};

pub use declarations::{REGISTRATION, registration};
pub use requirements::AUTHORIZATION;
pub use setup::N8nContract;
use setup::SERVER_SETUP;

/// The hooks' prefix under the mount, with its body limit and rate.
pub const HOOKS_PATH: &str = "hooks/";
pub const HOOKS_MAX_BODY_BYTES: u64 = 1024 * 1024;
pub const HOOKS_REQUESTS_PER_MINUTE: u32 = 600;

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
}

impl N8nMcp {
    pub fn new(workflows: WorkflowStore, n8n: N8nClient, served: Served) -> Self {
        Self {
            workflows,
            n8n,
            served,
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
    let routes = Routes::<N8nMcp>::builder(policy(N8nPermission::WorkflowsRead))
        .public_route(HOOKS_PATH, HOOKS_MAX_BODY_BYTES, HOOKS_REQUESTS_PER_MINUTE)
        .app_origin(policy(N8nPermission::Editor));
    resources::register(routes).build()
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

/// The hooks under `/n8n/hooks/`. Deliveries to n8n's webhook listener are not served
/// yet, so every delivery is answered 503.
pub fn hooks() -> Router {
    Router::new().route(
        "/hooks/{*path}",
        any(|| async {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "this server does not deliver webhooks to n8n yet",
            )
        }),
    )
}

/// The authenticated routes the app origin serves at its root. n8n's editor is not
/// forwarded yet, so its root answers 503 to a person the Hub admitted.
pub fn editor() -> Router {
    Router::new().route(
        "/",
        any(|| async {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "this server does not serve n8n's editor yet",
            )
        }),
    )
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
