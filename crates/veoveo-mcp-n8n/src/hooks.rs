//! The hooks: public deliveries under `/n8n/hooks/{path}`, passed to the root of n8n's
//! listener on loopback, where n8n serves its webhooks and forms. A webhook's own
//! signature, which n8n's trigger node checks, verifies the sender; the Hub routes the
//! delivery with its size and rate limits and strips its cookies.
use axum::{
    Router,
    body::Body,
    extract::{Path, State},
    http::{Request, StatusCode, header},
    response::{IntoResponse, Response},
    routing::any,
};
use url::Url;

use crate::proxy;

/// The hooks' prefix under the mount, with its body limit and rate.
pub const PATH: &str = "hooks/";
pub const MAX_BODY_BYTES: u64 = 1024 * 1024;
pub const REQUESTS_PER_MINUTE: u32 = 600;

/// n8n's endpoints that a delivery may reach: production and test webhooks, a waiting
/// execution's webhook, and forms. n8n's REST and public API are never reached.
pub const ENDPOINTS: &[&str] = &[
    "webhook",
    "webhook-test",
    "webhook-waiting",
    "form",
    "form-test",
    "form-waiting",
];

#[derive(Clone)]
struct Hooks {
    http: reqwest::Client,
    n8n: Url,
}

/// The hooks' routes, under the mount: `/hooks/{*path}`, delivered to n8n at `n8n`.
pub fn router(n8n: Url) -> Router {
    let http = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .expect("the hooks' HTTP client");
    Router::new()
        .route("/hooks/{*path}", any(deliver))
        .with_state(Hooks { http, n8n })
}

/// Whether `path`, after `/n8n/hooks/`, names one of n8n's webhook or form endpoints.
pub fn admitted(path: &str) -> bool {
    path.split('/')
        .next()
        .is_some_and(|endpoint| ENDPOINTS.contains(&endpoint))
        && !path
            .split('/')
            .any(|segment| segment == ".." || segment == ".")
}

async fn deliver(
    State(hooks): State<Hooks>,
    Path(path): Path<String>,
    request: Request<Body>,
) -> Response {
    let method = request.method().clone();
    if !admitted(&path) {
        tracing::info!(class = "n8n.webhook_delivery", %method, %path, status = 404, "refused a delivery outside n8n's webhooks");
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(target) = proxy::target(&hooks.n8n, &path, request.uri().query()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mut headers = proxy::forwardable(request.headers());
    headers.remove(header::COOKIE);
    let body = match axum::body::to_bytes(request.into_body(), MAX_BODY_BYTES as usize).await {
        Ok(body) => body,
        Err(_) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
    };
    let size = body.len();
    let response = proxy::forward(
        &hooks.http,
        method.clone(),
        target,
        headers,
        body.into(),
        |headers| {
            headers.remove(header::SET_COOKIE);
        },
    )
    .await;
    tracing::info!(
        class = "n8n.webhook_delivery", %method, %path, bytes = size,
        status = response.status().as_u16(), "delivered a webhook to n8n"
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deliveries_reach_only_n8n_s_webhooks_and_forms() {
        for path in [
            "webhook/hello-veoveo",
            "webhook-test/abc/def",
            "webhook-waiting/12",
            "form/4f9a",
        ] {
            assert!(admitted(path), "{path}");
        }
        for path in [
            "rest/login",
            "api/v1/workflows",
            "healthz",
            "",
            "webhook/../rest/login",
            "webhooks/x",
        ] {
            assert!(!admitted(path), "{path}");
        }
    }
}
