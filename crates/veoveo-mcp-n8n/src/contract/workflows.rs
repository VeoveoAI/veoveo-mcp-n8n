//! Workflows as the server serves them: a page of the listing, one workflow as applied
//! with its webhooks' public URLs, and what the server runs.
use std::{borrow::Cow, fmt};

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};

use super::{N8nResource, WorkflowName};

/// The SHA-256 of a workflow file's bytes, spelled as 64 lowercase hexadecimal digits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct FileDigest([u8; 32]);

/// A value that is not a SHA-256 digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("a file digest is 64 lowercase hexadecimal digits")]
pub struct FileDigestError;

impl FileDigest {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn parse(value: &str) -> Result<Self, FileDigestError> {
        let digits = value.as_bytes();
        if digits.len() != 64 {
            return Err(FileDigestError);
        }
        let mut bytes = [0; 32];
        let (pairs, _) = digits.as_chunks::<2>();
        for (byte, [high, low]) in bytes.iter_mut().zip(pairs) {
            *byte = (nibble(*high)? << 4) | nibble(*low)?;
        }
        Ok(Self(bytes))
    }
}

fn nibble(digit: u8) -> Result<u8, FileDigestError> {
    match digit {
        b'0'..=b'9' => Ok(digit - b'0'),
        b'a'..=b'f' => Ok(digit - b'a' + 10),
        _ => Err(FileDigestError),
    }
}

impl fmt::Display for FileDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.iter().try_for_each(|byte| write!(f, "{byte:02x}"))
    }
}

impl TryFrom<String> for FileDigest {
    type Error = FileDigestError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<FileDigest> for String {
    fn from(value: FileDigest) -> Self {
        value.to_string()
    }
}

impl JsonSchema for FileDigest {
    fn schema_name() -> Cow<'static, str> {
        "FileDigest".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "pattern": "^[0-9a-f]{64}$",
            "description": "The SHA-256 of the workflow file's bytes."
        })
    }
}

/// One workflow of the team, as the listing shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[derive(veoveo_hub_sdk::SingleOutput)]
pub struct WorkflowSummary {
    pub workflow: WorkflowName,
    /// `n8n://workflow/{workflow}`, where the workflow stays readable.
    pub workflow_uri: N8nResource,
    /// Whether the workflow starts with the Veoveo Trigger, so `run` can start it.
    pub runnable: bool,
    /// Whether n8n has the workflow published now.
    pub active: bool,
    /// The SHA-256 of the file the server applied.
    pub file_sha256: FileDigest,
}

/// One public webhook of a workflow: a Webhook node's production URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WebhookView {
    /// The node's name in the workflow.
    pub node: String,
    /// The HTTP method the node answers, as the workflow sets it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    /// The public URL a sender calls, under the instance's public base.
    pub url: String,
}

/// One workflow as the server applied it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[derive(veoveo_hub_sdk::SingleOutput)]
pub struct WorkflowView {
    pub workflow: WorkflowName,
    pub workflow_uri: N8nResource,
    pub runnable: bool,
    pub active: bool,
    pub file_sha256: FileDigest,
    /// The workflow's JSON as the server sent it to n8n.
    pub applied: serde_json::Value,
    pub webhooks: Vec<WebhookView>,
}

/// What the server runs and applied, as `n8n://status` serves it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[derive(veoveo_hub_sdk::SingleOutput)]
pub struct StatusView {
    /// n8n's version, as its command line reports it.
    pub n8n_version: String,
    /// The server's own version.
    pub server_version: String,
    /// When the server applied its workflows, RFC 3339.
    pub started_at: String,
    /// Each workflow file applied, by name, with its digest.
    pub workflows: Vec<AppliedFile>,
    /// The public URL under which n8n's production webhooks are, `{hooksUrl}webhook/{path}`.
    pub hooks_url: String,
    /// The linked repository the caller reaches this runner through, by its Lore ID, from
    /// the forwarded token's repository entry; none for a server that is no runner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    /// Whether the caller's grant on that repository writes, which shows them the data of
    /// executions no run of theirs started.
    pub writer: bool,
    /// Where the caller edits one of these workflows in their own Computer:
    /// `{issuer}/computer/apps/n8n/open?repository=…`, to which a view adds
    /// `&workflow={file}`; none for a server that is no runner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edit_url: Option<String>,
}

/// One workflow file the server applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AppliedFile {
    pub workflow: WorkflowName,
    pub file_sha256: FileDigest,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digests_are_lowercase_hex() {
        let digest = FileDigest::from_bytes([0xab; 32]);
        let text = digest.to_string();
        assert_eq!(text, "ab".repeat(32));
        assert_eq!(FileDigest::parse(&text).unwrap(), digest);
        for value in [
            "",
            "AB".repeat(32).as_str(),
            "ab".repeat(31).as_str(),
            "zz".repeat(32).as_str(),
        ] {
            assert!(FileDigest::parse(value).is_err(), "{value}");
        }
    }
}
