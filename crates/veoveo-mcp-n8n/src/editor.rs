//! n8n's editor on the server's app origin. The Hub signs a person in, checks
//! `n8n.editor` and forwards each request with a forwarded token; the server forwards it
//! to n8n's listener on loopback as n8n's owner. It attaches the owner's n8n session, which
//! it keeps itself and renews before it expires, and strips n8n's cookies from the
//! answers, so the browser holds no n8n credential. It keeps n8n's `X-Frame-Options`:
//! nothing frames the editor, and another server's app origin, which is same-site, must
//! not frame it with the person's session. It records the person behind each request
//! that changes n8n.
use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    Router,
    body::Body,
    extract::State,
    http::{HeaderMap, HeaderValue, Method, Request, StatusCode, header},
    response::{IntoResponse, Response},
};
use secrecy::{ExposeSecret, SecretString};
use tokio::sync::Mutex;
use url::Url;
use veoveo_hub_sdk::HubInternalIdentity;

use crate::{
    contract::N8nPermission,
    n8n::{N8nClient, OWNER_EMAIL, OwnerSession, SESSION_COOKIE_NAME},
    proxy,
};

/// A session expiring sooner than this is renewed before a request uses it.
const RENEW_BEFORE: Duration = Duration::from_secs(60 * 60);

/// The owner's n8n session, kept by the server and never given to a browser.
pub struct EditorSession {
    n8n: N8nClient,
    password: SecretString,
    current: Mutex<Option<OwnerSession>>,
}

impl EditorSession {
    pub fn new(n8n: N8nClient, password: SecretString) -> Self {
        Self {
            n8n,
            password,
            current: Mutex::new(None),
        }
    }

    /// The session's cookie value, logging in as the owner when the server holds none or
    /// the one it holds expires within the hour.
    pub async fn cookie(&self) -> Result<SecretString, crate::n8n::N8nError> {
        let mut current = self.current.lock().await;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let fresh = current.as_ref().is_some_and(|session| {
            session
                .expires_at()
                .is_some_and(|expires| expires > now + RENEW_BEFORE.as_secs())
        });
        if !fresh {
            let session = self.n8n.login(OWNER_EMAIL, &self.password).await?;
            tracing::info!("signed in to n8n as its owner for the editor");
            *current = Some(session);
        }
        Ok(current
            .as_ref()
            .map(|session| session.cookie().clone())
            .expect("a session after sign-in"))
    }

    /// Keep the session n8n renewed in an answer.
    async fn renewed(&self, cookie: String) {
        *self.current.lock().await = Some(OwnerSession::from_cookie(cookie));
    }

    /// Forget a session n8n refused.
    async fn refused(&self) {
        *self.current.lock().await = None;
    }
}

#[derive(Clone)]
struct Editor {
    http: reqwest::Client,
    n8n: Url,
    session: Arc<EditorSession>,
    /// The app origin's host and port, the only one this proxy answers on.
    host: Option<String>,
}

/// The editor's routes: every path of the app origin, forwarded to n8n at `n8n` with
/// `session`. `host` is the app origin's host authority; a request for another host is
/// not the editor's.
pub fn router(n8n: Url, session: Arc<EditorSession>, host: Option<String>) -> Router {
    let http = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("the editor's HTTP client");
    Router::new().fallback(forward).with_state(Editor {
        http,
        n8n,
        session,
        host,
    })
}

/// Whether `method` can change n8n.
fn changes(method: &Method) -> bool {
    !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

/// The browser's `Cookie` header without n8n's session, and the owner's session added.
pub fn with_session(cookies: Option<&HeaderValue>, session: &str) -> Option<HeaderValue> {
    let mut pairs: Vec<String> = cookies
        .and_then(|value| value.to_str().ok())
        .into_iter()
        .flat_map(|value| value.split(';'))
        .map(str::trim)
        .filter(|pair| !pair.is_empty())
        .filter(|pair| {
            pair.split('=')
                .next()
                .is_none_or(|name| name.trim() != SESSION_COOKIE_NAME)
        })
        .map(str::to_owned)
        .collect();
    pairs.push(format!("{SESSION_COOKIE_NAME}={session}"));
    HeaderValue::from_str(&pairs.join("; ")).ok()
}

/// The value of an `n8n-auth` cookie an answer sets, when it sets one.
pub fn renewed_session(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .filter_map(|cookie| cookie.split(';').next())
        .find_map(|pair| {
            let (name, value) = pair.split_once('=')?;
            (name.trim() == SESSION_COOKIE_NAME && !value.is_empty()).then(|| value.to_owned())
        })
}

fn host_of(request: &Request<Body>) -> Option<String> {
    request
        .headers()
        .get(header::HOST)
        .and_then(|host| host.to_str().ok())
        .map(str::to_ascii_lowercase)
}

async fn forward(State(editor): State<Editor>, request: Request<Body>) -> Response {
    // The SDK also serves authenticated routes under the mount; the editor is the app
    // origin's alone.
    if editor.host.is_none() || host_of(&request) != editor.host {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(identity) = request.extensions().get::<HubInternalIdentity>().cloned() else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if !identity.permissions.contains(&N8nPermission::Editor.name()) {
        return (StatusCode::FORBIDDEN, "the editor needs n8n.editor").into_response();
    }
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let Some(target) = proxy::target(&editor.n8n, &path, request.uri().query()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let session = match editor.session.cookie().await {
        Ok(session) => session,
        Err(error) => {
            tracing::error!(%error, "signing in to n8n as its owner failed");
            return (StatusCode::BAD_GATEWAY, "n8n's owner could not sign in").into_response();
        }
    };
    let mut headers = proxy::forwardable(request.headers());
    // The Hub's forwarded token is the server's, never n8n's.
    headers.remove(header::AUTHORIZATION);
    match with_session(headers.get(header::COOKIE), session.expose_secret()) {
        Some(cookies) => {
            headers.insert(header::COOKIE, cookies);
        }
        None => return StatusCode::BAD_REQUEST.into_response(),
    }
    let mut renewed = None;
    let body = proxy::streamed(request.into_body());
    let response = proxy::forward(
        &editor.http,
        method.clone(),
        target,
        headers,
        body,
        |answer| {
            renewed = renewed_session(answer);
            answer.remove(header::SET_COOKIE);
        },
    )
    .await;
    if let Some(cookie) = renewed {
        editor.session.renewed(cookie).await;
    }
    let status = response.status();
    if status == StatusCode::UNAUTHORIZED && path.starts_with("/rest/") {
        editor.session.refused().await;
    }
    if changes(&method) {
        tracing::info!(
            class = "n8n.editor_request",
            person = identity.actor.id.as_str(),
            subject = identity.actor.subject.as_str(),
            %method, %path, status = status.as_u16(),
            "an editor request that changes n8n"
        );
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_browser_never_sends_or_receives_n8n_s_session() {
        let browser = HeaderValue::from_static("theme=dark; n8n-auth=forged; other=1");
        assert_eq!(
            with_session(Some(&browser), "owner").unwrap(),
            "theme=dark; other=1; n8n-auth=owner"
        );
        assert_eq!(with_session(None, "owner").unwrap(), "n8n-auth=owner");
        let mut answer = HeaderMap::new();
        answer.append(
            header::SET_COOKIE,
            HeaderValue::from_static("n8n-auth=renewed; Max-Age=604800; HttpOnly; Secure"),
        );
        assert_eq!(renewed_session(&answer).as_deref(), Some("renewed"));
        assert!(renewed_session(&HeaderMap::new()).is_none());
        assert!(changes(&Method::PATCH));
        assert!(!changes(&Method::GET));
    }

    #[test]
    fn a_session_knows_when_it_expires() {
        // {"id":"u","exp":2000000000}
        let session = OwnerSession::from_cookie(
            "eyJhbGciOiJIUzI1NiJ9.eyJpZCI6InUiLCJleHAiOjIwMDAwMDAwMDB9.c2ln",
        );
        assert_eq!(session.expires_at(), Some(2_000_000_000));
        assert_eq!(OwnerSession::from_cookie("opaque").expires_at(), None);
    }
}
