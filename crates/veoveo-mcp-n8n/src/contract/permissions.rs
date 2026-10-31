//! The server's OAuth scope and permissions. Each permission names one action on one
//! resource type, with the access kind and risks the `ai.veoveo/authorization`
//! declaration carries; installation roles grant them and the Hub forwards the granted
//! ones. The declaration derives from the routes, so it carries a permission once a
//! route requires it.
use veoveo_hub_sdk::registration::{Access, PermissionDefinition, Risk};
use veoveo_types::{PermissionName, Vocabulary};

/// The OAuth scope that names the server's namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Vocabulary)]
#[vocabulary(scope)]
pub enum N8nScope {
    #[vocabulary(rename = "n8n")]
    N8n,
}

/// The server's permissions.
/// ```compile_fail
/// use veoveo_mcp_n8n::contract::N8nPermission;
/// fn requires(_: N8nPermission) {}
/// requires("n8n.workflows.read");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Vocabulary)]
pub enum N8nPermission {
    /// Read the team's workflows, their executions and the server's documents.
    #[vocabulary(rename = "n8n.workflows.read")]
    WorkflowsRead,
    /// Run a workflow: it acts on outside services with the installation's credentials.
    #[vocabulary(rename = "n8n.workflows.run")]
    WorkflowsRun,
    /// Retry or stop an execution.
    #[vocabulary(rename = "n8n.executions.write")]
    ExecutionsWrite,
}

impl N8nPermission {
    /// The permission's wire name, as declared and as granted.
    pub fn name(self) -> PermissionName {
        PermissionName::parse(self.as_str()).expect("declared n8n permission spelling")
    }
}

impl PermissionDefinition for N8nPermission {
    fn resource(self) -> &'static str {
        match self {
            Self::WorkflowsRead | Self::WorkflowsRun => "n8n.workflow",
            Self::ExecutionsWrite => "n8n.execution",
        }
    }

    fn access(self) -> Access {
        match self {
            Self::WorkflowsRead => Access::Read,
            Self::WorkflowsRun | Self::ExecutionsWrite => Access::Write,
        }
    }

    fn risks(self) -> &'static [Risk] {
        match self {
            Self::WorkflowsRun => &[Risk::Sensitive],
            Self::WorkflowsRead | Self::ExecutionsWrite => &[],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permissions_follow_the_naming_form() {
        for permission in N8nPermission::ALL {
            let name = permission.as_str();
            assert!(name.starts_with("n8n."), "{name}");
            assert!(permission.resource().starts_with("n8n."), "{name}");
            assert_eq!(permission.name().as_str(), name);
        }
        assert_eq!(N8nScope::N8n.as_str(), "n8n");
    }

    #[test]
    fn running_is_sensitive() {
        assert_eq!(N8nPermission::WorkflowsRun.risks(), &[Risk::Sensitive]);
        assert!(N8nPermission::WorkflowsRead.risks().is_empty());
        assert_eq!(N8nPermission::WorkflowsRead.access(), Access::Read);
    }
}
