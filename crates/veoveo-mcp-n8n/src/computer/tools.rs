//! The person's tools for n8n's Assistant, on loopback at `POST /n8n/mcp/{token}`, n8n's
//! `N8N_INSTANCE_AI_MCP_SERVERS` being `veoveo=http://127.0.0.1:{port}/n8n/mcp/{token}`:
//! n8n names such a server by its URL alone, with no headers, so the token of this start
//! is the path's last segment. The program forwards each request to the Hub's MCP endpoint
//! of the Computers profile with the Computer's Hub credential, the placeholder the proxy
//! replaces with the owner's token, and streams the answer back. The Hub answers every
//! request itself, `server/discover` among them, and decides each call as the owner's.
use axum::{
    Json,
    body::Body,
    extract::{Path, State},
    http::{HeaderValue, Request, Response, StatusCode, header},
    response::IntoResponse,
};
use futures::TryStreamExt;
use secrecy::{ExposeSecret, SecretString};
use serde_json::json;
use url::Url;

use crate::proxy;

/// The most an MCP request may hold: a tool call's arguments.
const MAX_REQUEST: usize = 16 << 20;

#[derive(Clone)]
pub struct ComputerTools {
    /// A client through the Computer's proxy.
    pub http: reqwest::Client,
    /// The Hub's MCP endpoint of the Computers profile.
    pub endpoint: Url,
    /// The Computer's Hub credential placeholder.
    pub bearer: SecretString,
    /// The token of this start, the last segment of the Assistant's MCP URL.
    pub token: SecretString,
}

/// The Assistant's MCP URL on the program's loopback `port`, which holds `token`.
pub fn tools_url(port: u16, token: &SecretString) -> String {
    format!(
        "http://127.0.0.1:{port}/{}/mcp/{}",
        crate::server().as_str(),
        token.expose_secret()
    )
}

/// An MCP request of n8n's Assistant, forwarded to the Hub: its headers but the
/// connection's own, its credentials and its cookies, and its body whole, with its
/// length; the answer streams back without cookies.
pub async fn forward(
    State(tools): State<ComputerTools>,
    Path(token): Path<String>,
    request: Request<Body>,
) -> Response<Body> {
    if token.as_bytes() != tools.token.expose_secret().as_bytes() {
        tracing::warn!(
            class = "n8n.assistant_tools",
            admitted = false,
            "refused an MCP request without the Computer's token"
        );
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "the token is not this Computer's n8n's"})),
        )
            .into_response();
    }
    let mut headers = proxy::forwardable(request.headers());
    for name in [
        header::AUTHORIZATION,
        header::COOKIE,
        header::CONTENT_LENGTH,
    ] {
        headers.remove(name);
    }
    let Ok(bearer) = HeaderValue::from_str(&format!("Bearer {}", tools.bearer.expose_secret()))
    else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    headers.insert(header::AUTHORIZATION, bearer);
    let Ok(body) = axum::body::to_bytes(request.into_body(), MAX_REQUEST).await else {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(json!({"error": "the MCP request is too large or broke off"})),
        )
            .into_response();
    };
    let sent = tools
        .http
        .post(tools.endpoint.clone())
        .headers(headers)
        .body(body)
        .send()
        .await;
    let response = match sent {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(class = "n8n.assistant_tools", %error, "the Hub could not be reached");
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": "the Hub could not be reached"})),
            )
                .into_response();
        }
    };
    let status = response.status();
    tracing::info!(
        class = "n8n.assistant_tools",
        admitted = true,
        status = status.as_u16(),
        "an MCP request of n8n's Assistant"
    );
    let mut headers = proxy::forwardable(response.headers());
    headers.remove(header::SET_COOKIE);
    let mut forwarded = Response::new(Body::from_stream(
        response.bytes_stream().map_err(std::io::Error::other),
    ));
    *forwarded.status_mut() = status;
    *forwarded.headers_mut() = headers;
    forwarded
}
