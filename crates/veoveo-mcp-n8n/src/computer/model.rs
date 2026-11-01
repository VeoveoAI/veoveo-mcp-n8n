//! The Assistant's model, on loopback at `POST /v1/openai/chat/completions`, n8n's
//! `N8N_INSTANCE_AI_MODEL_URL` being `http://127.0.0.1:{port}/v1/openai`: n8n's `custom/`
//! models with a URL call only `{url}/chat/completions`. The program admits a request with
//! the token of this start, which only n8n's environment holds, and forwards it to the
//! provider's endpoint with the Computer's model credential, the placeholder the proxy
//! replaces with the owner's key. The request goes whole, with its length, for the proxy
//! that inspects it; the answer streams.
use axum::{
    Json,
    body::Body,
    extract::State,
    http::{HeaderMap, HeaderValue, Request, Response, StatusCode, header},
    response::IntoResponse,
};
use futures::TryStreamExt;
use secrecy::{ExposeSecret, SecretString};
use serde_json::json;
use url::Url;

use crate::proxy;

/// The path n8n's model requests take, the provider's own.
pub const MODEL_PATH: &str = "/v1/openai/chat/completions";
/// The most a model request may hold: a conversation's history and the tools' schemas.
const MAX_REQUEST: usize = 32 << 20;

#[derive(Clone)]
pub struct ComputerModel {
    /// A client through the Computer's proxy.
    pub http: reqwest::Client,
    /// The provider's chat completions endpoint.
    pub endpoint: Url,
    /// The Computer's model credential placeholder.
    pub key: SecretString,
    /// The token of this start, n8n's `N8N_INSTANCE_AI_MODEL_API_KEY`.
    pub token: SecretString,
}

/// Whether `headers` carry `Authorization: Bearer {token}`.
pub(super) fn bears(headers: &HeaderMap, token: &SecretString) -> bool {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|presented| presented.as_bytes() == token.expose_secret().as_bytes())
}

/// A chat completion of n8n's Assistant.
pub async fn chat(State(model): State<ComputerModel>, request: Request<Body>) -> Response<Body> {
    if !bears(request.headers(), &model.token) {
        tracing::warn!(
            class = "n8n.assistant_model",
            admitted = false,
            "refused a model request without the Computer's token"
        );
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": {"message": "the token is not this Computer's n8n's"}})),
        )
            .into_response();
    }
    let mut headers = HeaderMap::new();
    for name in [header::CONTENT_TYPE, header::ACCEPT] {
        if let Some(value) = request.headers().get(&name) {
            headers.insert(name, value.clone());
        }
    }
    let Ok(bearer) = HeaderValue::from_str(&format!("Bearer {}", model.key.expose_secret())) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    headers.insert(header::AUTHORIZATION, bearer);
    let Ok(body) = axum::body::to_bytes(request.into_body(), MAX_REQUEST).await else {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(json!({"error": {"message": "the model request is too large or broke off"}})),
        )
            .into_response();
    };
    let sent = model
        .http
        .post(model.endpoint.clone())
        .headers(headers)
        .body(body)
        .send()
        .await;
    let response = match sent {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(class = "n8n.assistant_model", %error, "the model could not be reached");
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": {"message": "the model could not be reached"}})),
            )
                .into_response();
        }
    };
    let status = response.status();
    tracing::info!(
        class = "n8n.assistant_model",
        admitted = true,
        status = status.as_u16(),
        "a model request of n8n's Assistant"
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
