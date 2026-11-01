//! One typed route table binds tools, reads, the overview App, discovery, current
//! permissions and the server's surface: the hooks, a public route prefix.
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
    contract::{ExecutionAccess, ExecutionId, N8nPermission},
    hooks,
    n8n::{N8nClient, N8nError},
    store::{RunStore, StoreError, WorkflowStore, stored_tenant},
};

pub use app::{OVERVIEW_APP_URI, overview_app};
pub use declarations::{REGISTRATION, registration};
pub use requirements::AUTHORIZATION;
pub use setup::N8nContract;
use setup::SERVER_SETUP;

pub fn force_setup() {
    LazyLock::force(&SERVER_SETUP);
}

/// The one tenant the runner serves, where its public surface is and what it runs.
#[derive(Clone)]
pub struct Served {
    pub tenant: veoveo_types::TenantId,
    /// The public URL of the hooks, `{public base}/hooks/`.
    pub hooks_url: Url,
    /// The Hub's issuer, under which a person's own Computer is.
    pub issuer: Url,
    /// n8n's version, as its command line reports it.
    pub n8n_version: String,
    /// When the server applied its workflows, RFC 3339.
    pub started_at: String,
    /// The tracked branch of the runner's linked repository, by its Lore ID, as the Hub
    /// names it in the runner's environment.
    pub tracked_branch: Option<String>,
}

#[derive(Clone)]
pub struct N8nMcp {
    workflows: WorkflowStore,
    runs: RunStore,
    /// The n8n instance whose data the server keeps the key of: the dispatched
    /// executions are recorded under it.
    instance: uuid::Uuid,
    n8n: N8nClient,
    served: Served,
    app: AppHtml,
}

/// The Lore action whose grant on the repository shows a caller the data of executions
/// no run of theirs started.
const WRITE: &str = "write";

impl N8nMcp {
    pub fn new(
        workflows: WorkflowStore,
        runs: RunStore,
        instance: uuid::Uuid,
        n8n: N8nClient,
        served: Served,
        app: AppHtml,
    ) -> Self {
        Self {
            workflows,
            runs,
            instance,
            n8n,
            served,
            app,
        }
    }

    /// Whether the caller's grant on the runner's linked repository, which the forwarded
    /// token carries, holds `write`.
    fn writer(identity: &HubInternalIdentity) -> bool {
        identity.repository_grant().is_some_and(|grant| {
            veoveo_hub_sdk::registration::RepositoryAction::parse(WRITE)
                .is_ok_and(|write| grant.allows(&write))
        })
    }

    /// What of `execution` the caller reads: an execution a run's Task dispatched shows
    /// its data to that Task's caller alone; any other, a webhook's, a schedule's or a
    /// retry's, to a writer of the repository; everyone else reads its summary.
    async fn access(
        &self,
        tenant: veoveo_store::TenantId,
        identity: &HubInternalIdentity,
        execution: &ExecutionId,
    ) -> Result<ExecutionAccess, McpError> {
        let dispatched = self
            .runs
            .dispatched(tenant, self.instance, execution)
            .await
            .map_err(store_error)?;
        let data = match dispatched {
            Some(dispatched) => dispatched.caller == identity.actor.id.as_str(),
            None => Self::writer(identity),
        };
        Ok(if data {
            ExecutionAccess::Data
        } else {
            ExecutionAccess::Summary
        })
    }

    /// The stored tenant of a caller of the runner's own tenant; another tenant's caller
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
    let mut routes = Routes::<N8nMcp>::builder(policy(N8nPermission::WorkflowsRead)).public_route(
        hooks::PATH,
        hooks::MAX_BODY_BYTES,
        hooks::REQUESTS_PER_MINUTE,
    );
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
