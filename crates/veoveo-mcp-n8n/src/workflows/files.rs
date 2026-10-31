//! The branch's `workflows/`: one JSON file per workflow, in the form n8n exports it,
//! named by its file. The server sends n8n the members of the public API's write shape
//! and leaves the members n8n sets itself.
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use url::Url;

use crate::{
    contract::{FileDigest, WebhookView, WorkflowName},
    n8n::VEOVEO_TRIGGER,
    store::MAX_WORKFLOWS,
};

/// The node type of n8n's Webhook trigger.
const WEBHOOK: &str = "n8n-nodes-base.webhook";
/// The largest workflow file the server reads.
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;

/// Members of the public API's write shape that the server sends as the file sets them.
const WRITTEN: &[&str] = &[
    "name",
    "description",
    "nodes",
    "connections",
    "settings",
    "staticData",
    "pinData",
    "nodeGroups",
];
/// Members of an n8n export that n8n sets itself, which the server leaves out.
const SET_BY_N8N: &[&str] = &[
    "id",
    "active",
    "activeVersionId",
    "activeVersion",
    "createdAt",
    "updatedAt",
    "isArchived",
    "versionId",
    "versionCounter",
    "triggerCount",
    "meta",
    "tags",
    "shared",
    "projectId",
    "parentFolder",
    "parentFolderId",
    "sourceWorkflowId",
    "homeProject",
    "scopes",
];

/// One file of `workflows/`, read and projected onto what the server sends n8n.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkflowFile {
    pub workflow: WorkflowName,
    /// The SHA-256 of the file's bytes.
    pub digest: FileDigest,
    /// The public API's write shape of the workflow.
    pub body: Value,
    pub runnable: bool,
}

/// Read every `*.json` file of `directory`, in name order. A missing directory holds no
/// workflow; any other file is not the server's.
pub fn read_directory(directory: &Path) -> Result<Vec<WorkflowFile>> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("reading {}", directory.display()));
        }
    };
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let Some(stem) = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_suffix(".json"))
        else {
            continue;
        };
        let metadata = std::fs::metadata(&path)?;
        if !metadata.is_file() {
            continue;
        }
        let workflow = WorkflowName::parse(stem).with_context(|| {
            format!(
                "{} is not a workflow file name: lowercase letters, digits and inner hyphens",
                path.display()
            )
        })?;
        if metadata.len() > MAX_FILE_BYTES {
            bail!("{} exceeds {MAX_FILE_BYTES} bytes", path.display());
        }
        let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        files.push(
            WorkflowFile::parse(workflow, &bytes)
                .with_context(|| format!("reading {}", path.display()))?,
        );
    }
    if files.len() > MAX_WORKFLOWS {
        bail!(
            "{} holds more than {MAX_WORKFLOWS} workflows",
            directory.display()
        );
    }
    files.sort_by(|left, right| left.workflow.cmp(&right.workflow));
    Ok(files)
}

impl WorkflowFile {
    /// The file `bytes` of `workflow`.
    pub fn parse(workflow: WorkflowName, bytes: &[u8]) -> Result<Self> {
        let digest = FileDigest::from_bytes(Sha256::digest(bytes).into());
        let Value::Object(exported) = serde_json::from_slice(bytes).context("not JSON")? else {
            bail!("a workflow file holds one JSON object");
        };
        let body = write_shape(&workflow, exported)?;
        let runnable = nodes(&body).any(|node| node_type(node) == Some(VEOVEO_TRIGGER));
        Ok(Self {
            workflow,
            digest,
            body,
            runnable,
        })
    }
}

/// The public API's write shape of an exported workflow. A member the export does not
/// name and the write shape does not hold is refused, so a typo never passes silently.
fn write_shape(workflow: &WorkflowName, exported: Map<String, Value>) -> Result<Value> {
    let mut body = Map::new();
    for (member, value) in exported {
        if WRITTEN.contains(&member.as_str()) {
            body.insert(member, value);
        } else if !SET_BY_N8N.contains(&member.as_str()) {
            bail!("unknown workflow member {member:?}");
        }
    }
    if !body.get("nodes").is_some_and(Value::is_array) {
        bail!("a workflow's nodes are a list");
    }
    if !body.get("connections").is_some_and(Value::is_object) {
        bail!("a workflow's connections are an object");
    }
    match body.get("name") {
        None => {
            body.insert("name".to_owned(), Value::String(workflow.to_string()));
        }
        Some(Value::String(name)) if !name.trim().is_empty() => {}
        Some(_) => bail!("a workflow's name is a nonempty string"),
    }
    match body.get("settings") {
        None => {
            body.insert("settings".to_owned(), Value::Object(Map::new()));
        }
        Some(Value::Object(_)) => {}
        Some(_) => bail!("a workflow's settings are an object"),
    }
    Ok(Value::Object(body))
}

/// The members of n8n's `document` of a workflow that a workflow file holds, in the form
/// the server applies: what a person's agent writes to the file.
pub fn file_members(document: &Value) -> Value {
    let members = document
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(member, _)| WRITTEN.contains(&member.as_str()))
        .map(|(member, value)| (member.clone(), value.clone()))
        .collect();
    Value::Object(members)
}

fn nodes(body: &Value) -> impl Iterator<Item = &Map<String, Value>> {
    body.get("nodes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
}

fn node_type(node: &Map<String, Value>) -> Option<&str> {
    node.get("type").and_then(Value::as_str)
}

/// The public URL of each enabled Webhook node of `body`: n8n's production webhook path
/// under `hooks`, the server's public hooks URL, as `{hooks}webhook/{path}`.
pub fn webhooks(body: &Value, hooks: &Url) -> Vec<WebhookView> {
    nodes(body)
        .filter(|node| node_type(node) == Some(WEBHOOK))
        .filter(|node| node.get("disabled").and_then(Value::as_bool) != Some(true))
        .filter_map(|node| {
            let parameters = node.get("parameters").and_then(Value::as_object);
            let path = parameters
                .and_then(|parameters| parameters.get("path"))
                .and_then(Value::as_str)
                .filter(|path| !path.is_empty())
                .or_else(|| node.get("webhookId").and_then(Value::as_str))?;
            let mut url = hooks.clone();
            url.path_segments_mut()
                .ok()?
                .pop_if_empty()
                .push("webhook")
                .extend(path.split('/').filter(|segment| !segment.is_empty()));
            Some(WebhookView {
                node: node.get("name").and_then(Value::as_str)?.to_owned(),
                method: parameters
                    .and_then(|parameters| parameters.get("httpMethod"))
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                url: url.to_string(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(value: &str) -> WorkflowName {
        WorkflowName::parse(value).unwrap()
    }

    const EXPORTED: &str = r#"{
        "id": "Au7bKCOK5h1uXPLz",
        "name": "Hello Veoveo",
        "active": true,
        "versionId": "0190f3a2-7b1c-4d2e-8f3a-4b5c6d7e8f90",
        "meta": {"instanceId": "abc"},
        "tags": [],
        "settings": {"executionOrder": "v1"},
        "nodes": [
            {"id": "a1", "name": "Hook", "type": "n8n-nodes-base.webhook", "typeVersion": 2,
             "position": [0, 0], "webhookId": "3f6f7a52-8c1e-4a59-9a43-1d2f2b8b0c11",
             "parameters": {"httpMethod": "POST", "path": "hello/veoveo", "responseMode": "lastNode"}},
            {"id": "a2", "name": "Off", "type": "n8n-nodes-base.webhook", "typeVersion": 2,
             "position": [0, 200], "disabled": true, "parameters": {"path": "off"}},
            {"id": "a3", "name": "Greeting", "type": "n8n-nodes-base.set", "typeVersion": 3.4,
             "position": [200, 0], "parameters": {}}
        ],
        "connections": {"Hook": {"main": [[{"node": "Greeting", "type": "main", "index": 0}]]}}
    }"#;

    #[test]
    fn an_export_becomes_the_write_shape() {
        let file = WorkflowFile::parse(name("hello-veoveo"), EXPORTED.as_bytes()).unwrap();
        let body = file.body.as_object().unwrap();
        let mut members: Vec<_> = body.keys().map(String::as_str).collect();
        members.sort_unstable();
        assert_eq!(members, ["connections", "name", "nodes", "settings"]);
        assert_eq!(body["name"], "Hello Veoveo");
        assert!(!file.runnable);
        assert_eq!(
            file.digest,
            FileDigest::from_bytes(Sha256::digest(EXPORTED.as_bytes()).into())
        );
        // A file without a name or settings takes its file's name and empty settings.
        let bare = WorkflowFile::parse(
            name("bare"),
            br#"{"nodes": [{"name": "Start", "type": "CUSTOM.veoveoTrigger"}], "connections": {}}"#,
        )
        .unwrap();
        assert_eq!(bare.body["name"], "bare");
        assert_eq!(bare.body["settings"], serde_json::json!({}));
        assert!(bare.runnable);
    }

    #[test]
    fn malformed_files_are_refused() {
        for (bytes, expected) in [
            (&b"[]"[..], "one JSON object"),
            (b"{", "not JSON"),
            (br#"{"connections": {}}"#, "nodes are a list"),
            (br#"{"nodes": []}"#, "connections are an object"),
            (
                br#"{"nodes": [], "connections": {}, "nmae": "x"}"#,
                "unknown workflow member",
            ),
            (
                br#"{"nodes": [], "connections": {}, "name": ""}"#,
                "nonempty string",
            ),
            (
                br#"{"nodes": [], "connections": {}, "settings": []}"#,
                "settings are an object",
            ),
        ] {
            let error = WorkflowFile::parse(name("bad"), bytes).unwrap_err();
            assert!(format!("{error:#}").contains(expected), "{error:#}");
        }
    }

    #[test]
    fn webhooks_are_public_urls_under_the_hooks() {
        let file = WorkflowFile::parse(name("hello-veoveo"), EXPORTED.as_bytes()).unwrap();
        let hooks = Url::parse("https://veoveo.example/instances/019a/n8n/hooks/").unwrap();
        assert_eq!(
            webhooks(&file.body, &hooks),
            [WebhookView {
                node: "Hook".to_owned(),
                method: Some("POST".to_owned()),
                url: "https://veoveo.example/instances/019a/n8n/hooks/webhook/hello/veoveo"
                    .to_owned(),
            }]
        );
    }

    #[test]
    fn the_directory_holds_one_workflow_per_json_file() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("b-two.json"), EXPORTED).unwrap();
        std::fs::write(directory.path().join("a-one.json"), EXPORTED).unwrap();
        std::fs::write(directory.path().join("README.md"), "not a workflow").unwrap();
        std::fs::create_dir(directory.path().join("nested.json")).unwrap();
        let files = read_directory(directory.path()).unwrap();
        let names: Vec<_> = files.iter().map(|file| file.workflow.as_str()).collect();
        assert_eq!(names, ["a-one", "b-two"]);
        assert!(
            read_directory(&directory.path().join("missing"))
                .unwrap()
                .is_empty()
        );
        std::fs::write(directory.path().join("Bad_Name.json"), EXPORTED).unwrap();
        let error = read_directory(directory.path()).unwrap_err();
        assert!(
            error.to_string().contains("not a workflow file name"),
            "{error}"
        );
    }
}
