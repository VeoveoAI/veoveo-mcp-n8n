//! Forwarding a request the Hub delivered to the hooks to n8n's listener on loopback,
//! streaming n8n's answer.
use axum::{
    body::Body,
    http::{HeaderMap, HeaderName, Method, Response, StatusCode, header},
    response::IntoResponse,
};
use futures::TryStreamExt;
use url::Url;

/// Headers that belong to one connection, which a proxy never forwards (RFC 9110 §7.6.1),
/// and the request's own `Host`.
const HOP_BY_HOP: &[HeaderName] = &[
    header::CONNECTION,
    header::PROXY_AUTHENTICATE,
    header::PROXY_AUTHORIZATION,
    header::TE,
    header::TRAILER,
    header::TRANSFER_ENCODING,
    header::UPGRADE,
    header::HOST,
];

/// The prefix of the server's own headers, which nothing outside the server sets on a
/// request to n8n.
pub const SERVER_HEADER_PREFIX: &str = "x-veoveo-";

/// `headers` without the hop-by-hop ones, those the `Connection` header names, `keep-alive`
/// and the server's own.
pub fn forwardable(headers: &HeaderMap) -> HeaderMap {
    let named: Vec<String> = headers
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(|name| name.trim().to_ascii_lowercase())
        .collect();
    let mut forwarded = HeaderMap::new();
    for (name, value) in headers {
        let lower = name.as_str();
        if HOP_BY_HOP.contains(name)
            || lower == "keep-alive"
            || lower.starts_with(SERVER_HEADER_PREFIX)
            || named.iter().any(|named| named == lower)
        {
            continue;
        }
        forwarded.append(name.clone(), value.clone());
    }
    forwarded
}

/// Send `method` with `headers` and `body` to `target`, and answer with n8n's response,
/// streaming its body, whose headers `answer` adjusts.
pub async fn forward(
    http: &reqwest::Client,
    method: Method,
    target: Url,
    headers: HeaderMap,
    body: reqwest::Body,
    answer: impl FnOnce(&mut HeaderMap),
) -> Response<Body> {
    let sent = http
        .request(method, target)
        .headers(headers)
        .body(body)
        .send()
        .await;
    let response = match sent {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(%error, "n8n could not be reached");
            return (StatusCode::BAD_GATEWAY, "n8n could not be reached").into_response();
        }
    };
    let status = response.status();
    let mut headers = forwardable(response.headers());
    answer(&mut headers);
    let body = Body::from_stream(response.bytes_stream().map_err(std::io::Error::other));
    let mut forwarded = Response::new(body);
    *forwarded.status_mut() = status;
    *forwarded.headers_mut() = headers;
    forwarded
}

/// The URL on n8n's listener of `path` and `query`, relative to its root.
pub fn target(base: &Url, path: &str, query: Option<&str>) -> Option<Url> {
    let mut url = base.join(path.trim_start_matches('/')).ok()?;
    if url.origin() != base.origin() {
        return None;
    }
    url.set_query(query);
    Some(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn hop_by_hop_and_server_headers_stay_behind() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::HOST,
            HeaderValue::from_static("n8n.apps.veoveo.test"),
        );
        headers.insert(
            header::CONNECTION,
            HeaderValue::from_static("keep-alive, x-trace"),
        );
        headers.insert("keep-alive", HeaderValue::from_static("timeout=5"));
        headers.insert("x-trace", HeaderValue::from_static("1"));
        headers.insert("x-veoveo-dispatch", HeaderValue::from_static("run"));
        headers.insert(
            header::TRANSFER_ENCODING,
            HeaderValue::from_static("chunked"),
        );
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        headers.insert(
            "x-hub-signature-256",
            HeaderValue::from_static("sha256=abc"),
        );
        let forwarded = forwardable(&headers);
        let names: Vec<_> = forwarded.keys().map(HeaderName::as_str).collect();
        assert_eq!(names, ["content-type", "x-hub-signature-256"]);
    }

    #[test]
    fn targets_stay_on_n8n_s_listener() {
        let base = Url::parse("http://127.0.0.1:5678/").unwrap();
        assert_eq!(
            target(&base, "webhook/hello", Some("a=1"))
                .unwrap()
                .as_str(),
            "http://127.0.0.1:5678/webhook/hello?a=1"
        );
        assert_eq!(
            target(&base, "/rest/login", None).unwrap().as_str(),
            "http://127.0.0.1:5678/rest/login"
        );
        assert_eq!(
            target(&base, "//evil.example/x", None).unwrap().as_str(),
            "http://127.0.0.1:5678/evil.example/x"
        );
    }
}
