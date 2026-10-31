//! The server's identities: a workflow, named by its file under `workflows/`, and an n8n
//! execution. Each admits its input once, so internal code never handles a raw string
//! where a workflow or an execution is meant.
use std::{borrow::Cow, fmt};

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};

/// The most characters a workflow name holds.
pub const MAX_WORKFLOW_NAME_CHARS: usize = 64;
/// The most digits an execution ID holds.
const MAX_EXECUTION_DIGITS: usize = 19;

/// The JSON Schema pattern of a workflow name.
const WORKFLOW_NAME_PATTERN: &str = "^[a-z0-9]+(-[a-z0-9]+)*$";

/// A value that failed admission as one of the server's identities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum N8nIdError {
    #[error("a workflow name is 1 to 64 lowercase ASCII letters, digits and single inner hyphens")]
    WorkflowName,
    #[error("an execution ID is n8n's decimal execution number")]
    Execution,
}

/// A workflow, named by its file: `invoice-intake` for `workflows/invoice-intake.json`.
/// Lowercase ASCII letters and digits in groups joined by single hyphens, at most 64
/// characters, so its order is the same in Rust and in PostgreSQL's `C` collation.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct WorkflowName(String);

impl WorkflowName {
    pub fn parse(value: impl Into<String>) -> Result<Self, N8nIdError> {
        let value = value.into();
        let admitted = !value.is_empty()
            && value.len() <= MAX_WORKFLOW_NAME_CHARS
            && value.split('-').all(|group| {
                !group.is_empty()
                    && group
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
            });
        admitted
            .then_some(Self(value))
            .ok_or(N8nIdError::WorkflowName)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The workflow's file name under `workflows/`.
    pub fn file_name(&self) -> String {
        format!("{}.json", self.0)
    }
}

impl fmt::Display for WorkflowName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for WorkflowName {
    type Error = N8nIdError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl From<WorkflowName> for String {
    fn from(value: WorkflowName) -> Self {
        value.0
    }
}

impl JsonSchema for WorkflowName {
    fn schema_name() -> Cow<'static, str> {
        "WorkflowName".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "minLength": 1,
            "maxLength": MAX_WORKFLOW_NAME_CHARS,
            "pattern": WORKFLOW_NAME_PATTERN,
            "description": "The workflow's file name under workflows/, without .json."
        })
    }
}

/// An n8n execution: n8n's decimal execution number, without leading zeros. It travels
/// as a string, so a browser reads it exactly.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ExecutionId(String);

impl ExecutionId {
    pub fn parse(value: impl Into<String>) -> Result<Self, N8nIdError> {
        let value = value.into();
        let admitted = !value.is_empty()
            && value.len() <= MAX_EXECUTION_DIGITS
            && value.bytes().all(|b| b.is_ascii_digit())
            && (value == "0" || !value.starts_with('0'));
        admitted.then_some(Self(value)).ok_or(N8nIdError::Execution)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ExecutionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for ExecutionId {
    type Error = N8nIdError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl From<ExecutionId> for String {
    fn from(value: ExecutionId) -> Self {
        value.0
    }
}

impl JsonSchema for ExecutionId {
    fn schema_name() -> Cow<'static, str> {
        "ExecutionId".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "pattern": "^(0|[1-9][0-9]{0,18})$",
            "description": "n8n's execution number."
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workflow_names_are_file_stems() {
        for name in [
            "a",
            "invoice-intake",
            "hello-veoveo",
            "2026-report",
            &"x".repeat(MAX_WORKFLOW_NAME_CHARS),
        ] {
            let parsed = WorkflowName::parse(name).unwrap();
            assert_eq!(parsed.as_str(), name);
            assert_eq!(parsed.file_name(), format!("{name}.json"));
        }
        let long = "x".repeat(MAX_WORKFLOW_NAME_CHARS + 1);
        for name in [
            "",
            "-leading",
            "trailing-",
            "double--hyphen",
            "Upper",
            "under_score",
            "dot.json",
            "space name",
            "ünïcode",
            "../escape",
            long.as_str(),
        ] {
            assert_eq!(
                WorkflowName::parse(name),
                Err(N8nIdError::WorkflowName),
                "{name:?}"
            );
        }
    }

    #[test]
    fn workflow_names_order_as_bytes() {
        let mut names: Vec<_> = ["b", "a-b", "a", "a0", "ab", "a-a"]
            .into_iter()
            .map(|name| WorkflowName::parse(name).unwrap())
            .collect();
        names.sort();
        let sorted: Vec<_> = names.iter().map(WorkflowName::as_str).collect();
        assert_eq!(sorted, ["a", "a-a", "a-b", "a0", "ab", "b"]);
    }

    #[test]
    fn execution_ids_are_canonical_decimals() {
        for id in ["0", "1", "42", "9223372036854775807"] {
            assert_eq!(ExecutionId::parse(id).unwrap().as_str(), id);
        }
        for id in ["", "01", "-1", "1.0", "abc", "12345678901234567890"] {
            assert_eq!(ExecutionId::parse(id), Err(N8nIdError::Execution), "{id:?}");
        }
    }

    #[test]
    fn identities_travel_as_strings() {
        let name: WorkflowName = serde_json::from_str("\"invoice-intake\"").unwrap();
        assert_eq!(serde_json::to_string(&name).unwrap(), "\"invoice-intake\"");
        assert!(serde_json::from_str::<WorkflowName>("\"Invoice\"").is_err());
        let execution: ExecutionId = serde_json::from_str("\"17\"").unwrap();
        assert_eq!(serde_json::to_string(&execution).unwrap(), "\"17\"");
        assert!(serde_json::from_str::<ExecutionId>("17").is_err());
    }
}
