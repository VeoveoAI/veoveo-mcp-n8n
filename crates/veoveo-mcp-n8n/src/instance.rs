//! `n8n/instance.toml`: the one tenant the branch serves, the node types its n8n admits
//! and the credentials it imports, each secret field naming the provider environment
//! variable whose placeholder fills it. The file never holds a credential value.
use std::{collections::BTreeMap, fmt, path::Path};

use serde::Deserialize;
use veoveo_types::TenantId;

/// Node types no instance admits: n8n's command, SSH, Git and local-file nodes, which
/// run programs or reach the sandbox's files, and its MCP nodes, since n8n reaches other
/// servers only through the server's bridge.
pub const EXCLUDED_NODES: &[&str] = &[
    "n8n-nodes-base.executeCommand",
    "n8n-nodes-base.ssh",
    "n8n-nodes-base.git",
    "n8n-nodes-base.localFileTrigger",
    "n8n-nodes-base.readWriteFile",
    "n8n-nodes-base.readBinaryFile",
    "n8n-nodes-base.readBinaryFiles",
    "n8n-nodes-base.writeBinaryFile",
    "@n8n/n8n-nodes-langchain.mcpClient",
    "@n8n/n8n-nodes-langchain.mcpClientTool",
    "@n8n/n8n-nodes-langchain.mcpRegistryClientTool",
    "@n8n/n8n-nodes-langchain.mcpTrigger",
];

/// Why `n8n/instance.toml` was refused.
#[derive(Debug, thiserror::Error)]
pub enum InstanceError {
    #[error("reading {path}: {source}")]
    Read {
        path: String,
        source: std::io::Error,
    },
    #[error("parsing {path}: {source}")]
    Parse {
        path: String,
        source: Box<toml::de::Error>,
    },
    #[error("the instance admits no node type; name each one under `nodes`")]
    NoNodes,
    #[error("the instance admits {0}, which no instance admits")]
    ExcludedNode(NodeType),
    #[error("credential {0} is declared twice")]
    DuplicateCredential(CredentialId),
    #[error("credential {credential} sets {field} both as data and as a secret")]
    FieldTwice {
        credential: CredentialId,
        field: String,
    },
    #[error("credential {credential} names no secret field")]
    NoSecret { credential: CredentialId },
}

/// The instance as `n8n/instance.toml` declares it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Instance {
    /// The one tenant the branch serves; a caller of another tenant is refused.
    pub tenant: TenantId,
    /// The node types n8n loads. n8n loads no other.
    pub nodes: Vec<NodeType>,
    /// The credentials the server imports at every start.
    #[serde(default)]
    pub credentials: Vec<CredentialMapping>,
}

impl Instance {
    pub fn load(path: &Path) -> Result<Self, InstanceError> {
        let text = std::fs::read_to_string(path).map_err(|source| InstanceError::Read {
            path: path.display().to_string(),
            source,
        })?;
        Self::parse(&text).map_err(|error| match error {
            InstanceError::Parse { source, .. } => InstanceError::Parse {
                path: path.display().to_string(),
                source,
            },
            other => other,
        })
    }

    pub fn parse(text: &str) -> Result<Self, InstanceError> {
        let instance: Self = toml::from_str(text).map_err(|source| InstanceError::Parse {
            path: "n8n/instance.toml".to_owned(),
            source: Box::new(source),
        })?;
        if instance.nodes.is_empty() {
            return Err(InstanceError::NoNodes);
        }
        if let Some(node) = instance
            .nodes
            .iter()
            .find(|node| EXCLUDED_NODES.contains(&node.as_str()))
        {
            return Err(InstanceError::ExcludedNode(node.clone()));
        }
        let mut seen = std::collections::BTreeSet::new();
        for credential in &instance.credentials {
            if !seen.insert(&credential.id) {
                return Err(InstanceError::DuplicateCredential(credential.id.clone()));
            }
            if credential.secrets.is_empty() {
                return Err(InstanceError::NoSecret {
                    credential: credential.id.clone(),
                });
            }
            if let Some(field) = credential
                .secrets
                .keys()
                .find(|field| occupied(&credential.data, field.keys()))
            {
                return Err(InstanceError::FieldTwice {
                    credential: credential.id.clone(),
                    field: field.to_string(),
                });
            }
        }
        Ok(instance)
    }
}

/// Whether `keys` names a value in `data`, or passes through one that is not a table.
fn occupied(data: &toml::Table, keys: &[String]) -> bool {
    match keys {
        [] => false,
        [last] => data.contains_key(last),
        [first, rest @ ..] => match data.get(first) {
            None => false,
            Some(toml::Value::Table(table)) => occupied(table, rest),
            Some(_) => true,
        },
    }
}

/// An n8n node type, `{package}.{node}`, such as `n8n-nodes-base.webhook`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(try_from = "String")]
pub struct NodeType(String);

impl NodeType {
    pub fn parse(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        let package_char = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-';
        let admitted = value.split_once('.').is_some_and(|(package, node)| {
            let package = match package.strip_prefix('@') {
                Some(scoped) => scoped.split_once('/').filter(|(scope, name)| {
                    !scope.is_empty()
                        && !name.is_empty()
                        && scope.bytes().all(package_char)
                        && name.bytes().all(package_char)
                }),
                None => (!package.is_empty() && package.bytes().all(package_char))
                    .then_some((package, package)),
            };
            package.is_some() && !node.is_empty() && node.bytes().all(|b| b.is_ascii_alphanumeric())
        });
        admitted.then_some(Self(value.clone())).ok_or_else(|| {
            format!("{value:?} is not an n8n node type such as n8n-nodes-base.webhook")
        })
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for NodeType {
    type Error = String;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl fmt::Display for NodeType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// An n8n credential, imported with the same ID at every start, so workflows name it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialMapping {
    pub id: CredentialId,
    /// The credential's name in n8n.
    pub name: String,
    /// n8n's credential type, such as `githubApi`.
    #[serde(rename = "type")]
    pub credential_type: CredentialType,
    /// The fields that hold no secret, such as an API's base URL.
    #[serde(default)]
    pub data: toml::Table,
    /// Each secret field, by its path in the credential's data, and the provider
    /// environment variable whose placeholder fills it.
    pub secrets: BTreeMap<FieldPath, EnvironmentName>,
}

/// An n8n credential ID: 1 to 36 ASCII letters and digits.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(try_from = "String")]
pub struct CredentialId(String);

impl CredentialId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for CredentialId {
    type Error = String;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        (!value.is_empty() && value.len() <= 36 && value.bytes().all(|b| b.is_ascii_alphanumeric()))
            .then_some(Self(value))
            .ok_or_else(|| "a credential ID is 1 to 36 ASCII letters and digits".to_owned())
    }
}

impl fmt::Display for CredentialId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// An n8n credential type, such as `githubApi` or `oAuth2Api`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct CredentialType(String);

impl CredentialType {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for CredentialType {
    type Error = String;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        (!value.is_empty()
            && value.len() <= 128
            && value.bytes().all(|b| b.is_ascii_alphanumeric()))
        .then_some(Self(value))
        .ok_or_else(|| "a credential type is n8n's name for it, such as githubApi".to_owned())
    }
}

/// A field of a credential's data, with `.` between the keys of nested objects, such as
/// `oauthTokenData.access_token`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(try_from = "String")]
pub struct FieldPath(Vec<String>);

impl FieldPath {
    /// The keys from the data's root to the field.
    pub fn keys(&self) -> &[String] {
        &self.0
    }
}

impl TryFrom<String> for FieldPath {
    type Error = String;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        let keys: Vec<String> = value.split('.').map(str::to_owned).collect();
        keys.iter()
            .all(|key| {
                !key.is_empty() && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            })
            .then_some(Self(keys))
            .ok_or_else(|| format!("{value:?} is not a credential field such as accessToken"))
    }
}

impl fmt::Display for FieldPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0.join("."))
    }
}

/// The name of an environment variable a provider sets, such as `GITHUB_TOKEN`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(try_from = "String")]
pub struct EnvironmentName(String);

impl EnvironmentName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for EnvironmentName {
    type Error = String;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        let mut bytes = value.bytes();
        let admitted = bytes
            .next()
            .is_some_and(|b| b.is_ascii_uppercase() || b == b'_')
            && bytes.all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
            && !value.starts_with("VEOVEO_");
        admitted.then_some(Self(value.clone())).ok_or_else(|| {
            format!("{value:?} is not a provider environment variable such as GITHUB_TOKEN")
        })
    }
}

impl fmt::Display for EnvironmentName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INSTANCE: &str = r#"
        tenant = "reference"
        nodes = ["n8n-nodes-base.webhook", "n8n-nodes-base.set", "@n8n/n8n-nodes-langchain.agent"]

        [[credentials]]
        id = "githubToken0001"
        name = "GitHub"
        type = "githubApi"
        data = { server = "https://api.github.com", user = "veoveo" }
        secrets = { accessToken = "GITHUB_TOKEN" }

        [[credentials]]
        id = "slackOauth00001"
        name = "Slack"
        type = "slackOAuth2Api"
        data = { grantType = "authorizationCode", oauthTokenData = { token_type = "Bearer" } }
        secrets = { "oauthTokenData.access_token" = "SLACK_ACCESS_TOKEN" }
    "#;

    #[test]
    fn the_instance_names_its_tenant_nodes_and_credentials() {
        let instance = Instance::parse(INSTANCE).unwrap();
        assert_eq!(instance.tenant.as_str(), "reference");
        assert_eq!(instance.nodes.len(), 3);
        assert_eq!(instance.credentials.len(), 2);
        let slack = &instance.credentials[1];
        let (field, variable) = slack.secrets.iter().next().unwrap();
        assert_eq!(field.keys(), ["oauthTokenData", "access_token"]);
        assert_eq!(variable.as_str(), "SLACK_ACCESS_TOKEN");
        let empty =
            Instance::parse("tenant = \"reference\"\nnodes = [\"n8n-nodes-base.set\"]").unwrap();
        assert!(empty.credentials.is_empty());
    }

    #[test]
    fn unsafe_or_malformed_instances_are_refused() {
        for (text, expected) in [
            ("tenant = \"reference\"\nnodes = []", "admits no node"),
            (
                "tenant = \"reference\"\nnodes = [\"n8n-nodes-base.executeCommand\"]",
                "which no instance admits",
            ),
            (
                "tenant = \"reference\"\nnodes = [\"@n8n/n8n-nodes-langchain.mcpClientTool\"]",
                "which no instance admits",
            ),
            (
                "tenant = \"reference\"\nnodes = [\"webhook\"]",
                "not an n8n node type",
            ),
            (
                "tenant = \"reference\"\nnodes = [\"n8n-nodes-base.set\"]\nextra = 1",
                "unknown field",
            ),
            (
                "tenant = \"reference\"\nnodes = [\"n8n-nodes-base.set\"]\n[[credentials]]\nid = \"a\"\nname = \"A\"\ntype = \"githubApi\"\nsecrets = { accessToken = \"VEOVEO_REGISTRATION_SECRET\" }",
                "provider environment variable",
            ),
            (
                "tenant = \"reference\"\nnodes = [\"n8n-nodes-base.set\"]\n[[credentials]]\nid = \"a\"\nname = \"A\"\ntype = \"githubApi\"\nsecrets = {}",
                "names no secret",
            ),
            (
                "tenant = \"reference\"\nnodes = [\"n8n-nodes-base.set\"]\n[[credentials]]\nid = \"a\"\nname = \"A\"\ntype = \"githubApi\"\ndata = { accessToken = \"literal\" }\nsecrets = { accessToken = \"GITHUB_TOKEN\" }",
                "both as data and as a secret",
            ),
            (
                "tenant = \"reference\"\nnodes = [\"n8n-nodes-base.set\"]\n[[credentials]]\nid = \"a-b\"\nname = \"A\"\ntype = \"githubApi\"\nsecrets = { accessToken = \"GITHUB_TOKEN\" }",
                "credential ID",
            ),
        ] {
            let error = Instance::parse(text).unwrap_err().to_string();
            assert!(error.contains(expected), "{text}: {error}");
        }
    }
}
