//! The server's resource addresses under the `n8n` scheme. The route declaration owns
//! each shape and its typed parameters; the shared URI machinery owns encoding and
//! parsing, so no address is built or split by hand.
use std::{borrow::Cow, error::Error, fmt};

use veoveo_types::{ResourceFieldCodec, ResourceRouteError, ResourceUriError, Vocabulary};

use super::WorkflowName;

/// A resource address that failed admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum N8nResourceError {
    Uri(ResourceUriError),
    UnknownResource,
    InvalidWorkflow,
}

impl fmt::Display for N8nResourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Uri(error) => error.fmt(f),
            Self::UnknownResource => f.write_str("unknown or noncanonical n8n resource address"),
            Self::InvalidWorkflow => f.write_str("invalid workflow name"),
        }
    }
}

impl Error for N8nResourceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Uri(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ResourceUriError> for N8nResourceError {
    fn from(error: ResourceUriError) -> Self {
        Self::Uri(error)
    }
}

/// A document the server embeds and serves under `n8n://docs/{doc_id}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Vocabulary)]
pub enum N8nDocument {
    /// The agent work manual, `AGENTS.md`.
    Agents,
    /// The design, `DESIGN.md`.
    Design,
}

/// A resource of the server. A usable address carries only the identities and cursor of
/// its own family.
#[veoveo_types::resource_address(routes(N8nAddresses), schema = derived, wire, parse = borrowed, display)]
pub enum N8nResource {
    #[resource(template = "n8n://docs")]
    Docs,
    #[resource(template = "n8n://docs/{doc_id}")]
    Document(#[resource(variable = "doc_id", codec = N8nFieldCodec)] N8nDocument),
    #[resource(template = "n8n://contract")]
    Contract,
    #[resource(template = "n8n://workflows")]
    Workflows,
    #[resource(template = "n8n://workflow/{workflow}")]
    Workflow(#[resource(variable = "workflow", codec = N8nFieldCodec)] WorkflowName),
    #[resource(template = "n8n://editor")]
    Editor,
}

fn route_error(error: ResourceRouteError) -> N8nResourceError {
    match error {
        ResourceRouteError::Uri(error) => N8nResourceError::Uri(error),
        _ => N8nResourceError::UnknownResource,
    }
}

struct N8nFieldCodec;

impl ResourceFieldCodec<N8nDocument> for N8nFieldCodec {
    type Error = N8nResourceError;
    fn parse(value: &str) -> Result<N8nDocument, Self::Error> {
        value.parse().map_err(|_| N8nResourceError::UnknownResource)
    }
    fn text(value: &N8nDocument) -> Cow<'_, str> {
        value.as_str().into()
    }
}

impl ResourceFieldCodec<WorkflowName> for N8nFieldCodec {
    type Error = N8nResourceError;
    fn parse(value: &str) -> Result<WorkflowName, Self::Error> {
        WorkflowName::parse(value).map_err(|_| N8nResourceError::InvalidWorkflow)
    }
    fn text(value: &WorkflowName) -> Cow<'_, str> {
        value.as_str().into()
    }
}

#[doc(hidden)]
pub struct N8nAddresses;
impl veoveo_types::ResourceProfile for N8nAddresses {
    type Error = N8nResourceError;
    const PROFILE: veoveo_types::ResourceProfileSpec<Self::Error> =
        veoveo_types::ResourceProfileSpec {
            route_error: |_, error| route_error(error),
        };
}

#[cfg(test)]
mod tests {
    use super::*;
    use veoveo_types::ResourceAddress;

    fn parse(uri: &str) -> Result<N8nResource, N8nResourceError> {
        N8nResource::parse(uri)
    }

    #[test]
    fn addresses_round_trip_through_their_routes() {
        for uri in [
            "n8n://docs",
            "n8n://docs/agents",
            "n8n://docs/design",
            "n8n://contract",
            "n8n://workflows",
            "n8n://workflow/hello-veoveo",
            "n8n://editor",
        ] {
            let address = parse(uri).unwrap_or_else(|error| panic!("{uri}: {error}"));
            assert_eq!(address.to_uri().unwrap().as_str(), uri);
        }
        assert_eq!(
            parse("n8n://workflow/hello-veoveo").unwrap(),
            N8nResource::Workflow(WorkflowName::parse("hello-veoveo").unwrap())
        );
    }

    #[test]
    fn malformed_addresses_are_refused() {
        for uri in [
            "n8n://workflow",
            "n8n://workflow/",
            "n8n://workflow/Hello",
            "n8n://workflow/hello.json",
            "n8n://workflow/a/b",
            "n8n://workflows?unknown=1",
            "n8n://docs/unknown",
            "n8n://editor/extra",
            "n8n://other",
            "template://workflows",
        ] {
            assert!(parse(uri).is_err(), "{uri}");
        }
    }
}
