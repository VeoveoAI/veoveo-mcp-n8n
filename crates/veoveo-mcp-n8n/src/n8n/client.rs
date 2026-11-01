//! n8n's loopback HTTP surface as the server uses it: readiness, the owner's login and the
//! API-key route on first start, and the public API (`/api/v1`, `X-N8N-API-KEY`) after.
use std::{fmt, time::Duration};

use reqwest::{Method, StatusCode, header};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use url::Url;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const API_KEY_HEADER: &str = "X-N8N-API-KEY";
const SESSION_COOKIE: &str = "n8n-auth";
/// The label of the API key the server mints in n8n.
pub const API_KEY_LABEL: &str = "veoveo-mcp-n8n";
/// The scopes of the server's API key: workflows for applying the team's workflows and executions
/// for runs.
pub const API_KEY_SCOPES: &[&str] = &[
    "workflow:create",
    "workflow:read",
    "workflow:update",
    "workflow:delete",
    "workflow:list",
    "workflow:activate",
    "workflow:deactivate",
    "execution:read",
    "execution:list",
    "execution:retry",
    "execution:stop",
];

/// The scopes of the API key `veoveo-n8n-computer` mints in a person's own n8n: workflows
/// and tags, by which it keeps each team's workflows together.
pub const COMPUTER_API_KEY_SCOPES: &[&str] = &[
    "workflow:create",
    "workflow:read",
    "workflow:update",
    "workflow:delete",
    "workflow:list",
    "tag:create",
    "tag:read",
    "tag:list",
    "workflowTags:update",
    "workflowTags:list",
];

/// A failed exchange with n8n.
#[derive(Debug, thiserror::Error)]
pub enum N8nError {
    #[error("n8n could not be reached: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("n8n answered {method} {path} with {status}: {message}")]
    Refused {
        method: Method,
        path: String,
        status: StatusCode,
        message: String,
    },
    #[error("n8n's answer to {path} is not what its API documents: {reason}")]
    Unexpected { path: String, reason: String },
}

impl N8nError {
    /// Whether n8n answered that the target does not exist.
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::Refused { status, .. } if *status == StatusCode::NOT_FOUND)
    }
}

/// An n8n workflow ID, such as `Au7bKCOK5h1uXPLz`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct N8nWorkflowId(String);

impl N8nWorkflowId {
    pub fn parse(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        (!value.is_empty() && value.len() <= 36 && value.bytes().all(|b| b.is_ascii_alphanumeric()))
            .then_some(Self(value.clone()))
            .ok_or_else(|| format!("{value:?} is not an n8n workflow ID"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for N8nWorkflowId {
    type Error = String;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl From<N8nWorkflowId> for String {
    fn from(value: N8nWorkflowId) -> Self {
        value.0
    }
}

impl fmt::Display for N8nWorkflowId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A workflow as n8n's public API returns it, with the members the server reads.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct N8nWorkflow {
    pub id: N8nWorkflowId,
    /// The current version; every save makes a new one.
    pub version_id: uuid::Uuid,
    /// The published version, when n8n has one published.
    pub active_version_id: Option<uuid::Uuid>,
    pub is_archived: bool,
}

impl N8nWorkflow {
    /// Whether n8n runs the workflow's triggers: some version of it is published.
    pub fn is_active(&self) -> bool {
        self.active_version_id.is_some()
    }
}

/// The owner's n8n session: its `n8n-auth` cookie's value, a JWT, with which a program
/// mints its API key, and which a person's own Computer attaches to each editor request.
pub struct OwnerSession(SecretString);

impl OwnerSession {
    /// A session from an `n8n-auth` cookie's value.
    pub fn from_cookie(value: impl Into<String>) -> Self {
        Self(SecretString::from(value.into()))
    }

    /// The `n8n-auth` cookie's value.
    pub fn cookie(&self) -> &SecretString {
        &self.0
    }

    /// When the session expires, in seconds since the Unix epoch: its JWT's `exp`.
    pub fn expires_at(&self) -> Option<u64> {
        use base64::Engine;
        let payload = self.0.expose_secret().split('.').nth(1)?;
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(payload.trim_end_matches('='))
            .ok()?;
        serde_json::from_slice::<serde_json::Value>(&bytes)
            .ok()?
            .get("exp")?
            .as_u64()
    }
}

/// The name of n8n's session cookie.
pub const SESSION_COOKIE_NAME: &str = SESSION_COOKIE;

#[derive(Deserialize)]
struct Data<T> {
    data: T,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MintedKey {
    raw_api_key: String,
}

#[derive(Deserialize)]
struct Message {
    message: Option<String>,
}

/// n8n's loopback HTTP surface.
#[derive(Clone)]
pub struct N8nClient {
    pub(super) http: reqwest::Client,
    base: Url,
    api_key: Option<std::sync::Arc<SecretString>>,
}

impl N8nClient {
    /// A client of n8n at `base`, such as `http://127.0.0.1:5678/`.
    pub fn new(base: Url) -> Result<Self, N8nError> {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()?;
        Ok(Self {
            http,
            base,
            api_key: None,
        })
    }

    /// The same client, calling the public API with `key`.
    pub fn with_api_key(&self, key: SecretString) -> Self {
        Self {
            http: self.http.clone(),
            base: self.base.clone(),
            api_key: Some(std::sync::Arc::new(key)),
        }
    }

    pub fn base(&self) -> &Url {
        &self.base
    }

    pub(super) fn url(&self, path: &str) -> Url {
        self.base.join(path).expect("a relative n8n path")
    }

    /// Whether n8n answers `/healthz/readiness`, which it does once its migrations ran.
    pub async fn is_ready(&self) -> bool {
        self.http
            .get(self.url("healthz/readiness"))
            .send()
            .await
            .is_ok_and(|response| response.status() == StatusCode::OK)
    }

    /// Log in as the owner n8n created from the environment.
    pub async fn login(
        &self,
        email: &str,
        password: &SecretString,
    ) -> Result<OwnerSession, N8nError> {
        let path = "rest/login";
        let body = serde_json::json!({
            "emailOrLdapLoginId": email,
            "password": password.expose_secret(),
        });
        let response = self
            .http
            .post(self.url(path))
            .header(header::CONTENT_TYPE, "application/json")
            .body(body.to_string())
            .send()
            .await?;
        let response = refuse_failure(Method::POST, path, response).await?;
        response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .filter_map(|cookie| cookie.split(';').next())
            .find_map(|pair| {
                pair.strip_prefix(SESSION_COOKIE)
                    .and_then(|rest| rest.strip_prefix('='))
                    .map(|value| OwnerSession(SecretString::from(value.to_owned())))
            })
            .ok_or_else(|| N8nError::Unexpected {
                path: path.to_owned(),
                reason: "the login set no session cookie".to_owned(),
            })
    }

    /// Mint an API key with the server's scopes, without an expiry, for `session`.
    pub async fn mint_api_key(
        &self,
        session: &OwnerSession,
        scopes: &[&str],
    ) -> Result<SecretString, N8nError> {
        let path = "rest/api-keys";
        let body = serde_json::json!({
            "label": API_KEY_LABEL,
            "scopes": scopes,
            "expiresAt": null,
        });
        let response = self
            .http
            .post(self.url(path))
            .header(header::CONTENT_TYPE, "application/json")
            .header(
                header::COOKIE,
                format!("{SESSION_COOKIE}={}", session.0.expose_secret()),
            )
            .body(body.to_string())
            .send()
            .await?;
        let minted: Data<MintedKey> =
            parse(path, refuse_failure(Method::POST, path, response).await?).await?;
        Ok(SecretString::from(minted.data.raw_api_key))
    }

    /// Revoke every API key the owner holds under the server's label, which n8n lets one
    /// user hold once: the key of an earlier sandbox's disk, which the server no longer
    /// knows. Returns how many it revoked.
    pub async fn revoke_api_keys(&self, session: &OwnerSession) -> Result<usize, N8nError> {
        let path = format!("rest/api-keys?ownership=mine&take=100&label={API_KEY_LABEL}");
        let cookie = format!("{SESSION_COOKIE}={}", session.0.expose_secret());
        let response = self
            .http
            .get(self.url(&path))
            .header(header::COOKIE, &cookie)
            .send()
            .await?;
        let listed: Data<serde_json::Value> =
            parse(&path, refuse_failure(Method::GET, &path, response).await?).await?;
        let keys = listed
            .data
            .get("items")
            .cloned()
            .unwrap_or(listed.data)
            .as_array()
            .cloned()
            .unwrap_or_default();
        let mut revoked = 0;
        for key in keys {
            if key.get("label").and_then(serde_json::Value::as_str) != Some(API_KEY_LABEL) {
                continue;
            }
            let Some(id) = key.get("id").and_then(serde_json::Value::as_str) else {
                continue;
            };
            let delete = format!("rest/api-keys/{id}");
            let response = self
                .http
                .delete(self.url(&delete))
                .header(header::COOKIE, &cookie)
                .send()
                .await?;
            refuse_failure(Method::DELETE, "rest/api-keys/{id}", response).await?;
            revoked += 1;
        }
        Ok(revoked)
    }

    /// Turn the Assistant's web search off in n8n's settings, as the owner: a person's own
    /// n8n has no search provider, and the Assistant counts as set up once search is either
    /// connected or off.
    pub async fn disable_assistant_search(&self, session: &OwnerSession) -> Result<(), N8nError> {
        let path = "rest/instance-ai/settings";
        let response = self
            .http
            .put(self.url(path))
            .header(header::CONTENT_TYPE, "application/json")
            .header(
                header::COOKIE,
                format!("{SESSION_COOKIE}={}", session.0.expose_secret()),
            )
            .body(serde_json::json!({"searchDisabled": true}).to_string())
            .send()
            .await?;
        refuse_failure(Method::PUT, path, response).await?;
        Ok(())
    }

    /// Whether n8n accepts the client's API key: `false` when it answers 401.
    pub async fn accepts_api_key(&self) -> Result<bool, N8nError> {
        let path = "api/v1/workflows?limit=1&excludePinnedData=true";
        let response = self.api(Method::GET, path, None).await?;
        if response.status() == StatusCode::UNAUTHORIZED {
            return Ok(false);
        }
        refuse_failure(Method::GET, path, response).await?;
        Ok(true)
    }

    /// The workflow `id`, or `None` when n8n has no such workflow.
    pub async fn workflow(&self, id: &N8nWorkflowId) -> Result<Option<N8nWorkflow>, N8nError> {
        match self
            .call(
                Method::GET,
                &format!("api/v1/workflows/{id}?excludePinnedData=true"),
                None,
            )
            .await
        {
            Ok(workflow) => Ok(Some(workflow)),
            Err(error) if error.is_not_found() => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Create a workflow from `body`, the public API's write shape.
    pub async fn create_workflow(&self, body: &serde_json::Value) -> Result<N8nWorkflow, N8nError> {
        self.call(Method::POST, "api/v1/workflows", Some(body))
            .await
    }

    /// Replace workflow `id` with `body`. n8n republishes a published workflow.
    pub async fn update_workflow(
        &self,
        id: &N8nWorkflowId,
        body: &serde_json::Value,
    ) -> Result<N8nWorkflow, N8nError> {
        self.call(Method::PUT, &format!("api/v1/workflows/{id}"), Some(body))
            .await
    }

    /// Publish the current version of workflow `id`.
    pub async fn publish(&self, id: &N8nWorkflowId) -> Result<N8nWorkflow, N8nError> {
        self.call(
            Method::POST,
            &format!("api/v1/workflows/{id}/publish"),
            Some(&serde_json::json!({})),
        )
        .await
    }

    /// Archive workflow `id`, which unpublishes it first.
    pub async fn archive(&self, id: &N8nWorkflowId) -> Result<N8nWorkflow, N8nError> {
        self.call(
            Method::POST,
            &format!("api/v1/workflows/{id}/archive"),
            None,
        )
        .await
    }

    /// Restore workflow `id` from the archive.
    pub async fn unarchive(&self, id: &N8nWorkflowId) -> Result<N8nWorkflow, N8nError> {
        self.call(
            Method::POST,
            &format!("api/v1/workflows/{id}/unarchive"),
            None,
        )
        .await
    }

    pub(super) async fn call<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<T, N8nError> {
        let response = self.api(method.clone(), path, body).await?;
        parse(path, refuse_failure(method, path, response).await?).await
    }

    pub(super) async fn api(
        &self,
        method: Method,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<reqwest::Response, N8nError> {
        let key = self.api_key.as_ref().ok_or_else(|| N8nError::Unexpected {
            path: path.to_owned(),
            reason: "the public API needs the server's API key".to_owned(),
        })?;
        let mut request = self
            .http
            .request(method, self.url(path))
            .header(API_KEY_HEADER, key.expose_secret())
            .header(header::ACCEPT, "application/json");
        if let Some(body) = body {
            request = request
                .header(header::CONTENT_TYPE, "application/json")
                .body(body.to_string());
        }
        Ok(request.send().await?)
    }
}

pub(super) async fn refuse_failure(
    method: Method,
    path: &str,
    response: reqwest::Response,
) -> Result<reqwest::Response, N8nError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = response.bytes().await.unwrap_or_default();
    let message = serde_json::from_slice::<Message>(&body)
        .ok()
        .and_then(|message| message.message)
        .unwrap_or_else(|| String::from_utf8_lossy(&body).chars().take(300).collect());
    Err(N8nError::Refused {
        method,
        path: path.split('?').next().unwrap_or(path).to_owned(),
        status,
        message,
    })
}

pub(super) async fn parse<T: DeserializeOwned>(
    path: &str,
    response: reqwest::Response,
) -> Result<T, N8nError> {
    let body = response.bytes().await?;
    serde_json::from_slice(&body).map_err(|error| N8nError::Unexpected {
        path: path.split('?').next().unwrap_or(path).to_owned(),
        reason: error.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workflow_ids_are_n8n_s_alphanumeric_ids() {
        assert!(N8nWorkflowId::parse("Au7bKCOK5h1uXPLz").is_ok());
        for id in ["", "a-b", "a/b", &"a".repeat(37)] {
            assert!(N8nWorkflowId::parse(id).is_err(), "{id}");
        }
    }

    #[test]
    fn a_workflow_reads_from_n8n_s_answer() {
        let workflow: N8nWorkflow = serde_json::from_value(serde_json::json!({
            "id": "Au7bKCOK5h1uXPLz",
            "name": "hello",
            "active": true,
            "versionId": "0190f3a2-7b1c-4d2e-8f3a-4b5c6d7e8f90",
            "activeVersionId": "0190f3a2-7b1c-4d2e-8f3a-4b5c6d7e8f90",
            "isArchived": false,
            "nodes": []
        }))
        .unwrap();
        assert!(workflow.is_active());
        let archived: N8nWorkflow = serde_json::from_value(serde_json::json!({
            "id": "Au7bKCOK5h1uXPLz",
            "versionId": "0190f3a2-7b1c-4d2e-8f3a-4b5c6d7e8f90",
            "activeVersionId": null,
            "isArchived": true
        }))
        .unwrap();
        assert!(!archived.is_active() && archived.is_archived);
    }
}
