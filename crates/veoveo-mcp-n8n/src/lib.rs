//! The n8n server's library. The `contract` feature holds the server's public types;
//! the default `server` feature adds the hosted server, which runs n8n beside itself,
//! applies the branch's workflows to it and serves them over MCP through the SDK.

#[cfg(feature = "contract")]
pub mod contract;

#[cfg(feature = "server")]
pub mod bridge;
#[cfg(feature = "server")]
pub mod editor;
#[cfg(feature = "server")]
pub mod hooks;
#[cfg(feature = "server")]
pub mod instance;
#[cfg(feature = "server")]
pub mod loopback;
#[cfg(feature = "server")]
pub mod mcp;
#[cfg(feature = "server")]
pub mod n8n;
#[cfg(feature = "server")]
pub mod proxy;
#[cfg(feature = "server")]
pub mod runs;
#[cfg(feature = "server")]
pub mod server;
#[cfg(feature = "server")]
pub mod store;
#[cfg(feature = "server")]
pub mod workflows;

#[cfg(feature = "contract")]
use veoveo_types::ServerSlug;

/// The component name, taken from the crate name.
pub fn component() -> &'static str {
    env!("CARGO_PKG_NAME")
}

/// The server's slug, which names its schema `n8n`, its group role `server_n8n`, its
/// channel and its URI scheme.
#[cfg(feature = "contract")]
pub fn server() -> ServerSlug {
    ServerSlug::parse("n8n").expect("the n8n server's slug")
}

/// The migrations in `migrations/`, which the server applies in its own schema at
/// startup and which record their history in `n8n._sqlx_migrations`.
#[cfg(feature = "server")]
pub fn migrator() -> sqlx::migrate::Migrator {
    sqlx::migrate!()
}

#[cfg(test)]
mod tests {
    #[test]
    fn component_is_the_crate_name() {
        assert_eq!(super::component(), "veoveo-mcp-n8n");
    }
}
