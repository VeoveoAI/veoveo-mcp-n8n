//! The owner's n8n session, which `veoveo-n8n-computer` keeps itself and attaches to each
//! editor request: one person, one Computer, so the editor acts as its owner. It signs in
//! with the password it keeps, renews the session from n8n's answers and signs in again
//! an hour before it expires or after n8n refused it. The browser never holds it.
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::http::{HeaderMap, HeaderValue, header};
use secrecy::SecretString;
use tokio::sync::Mutex;

use crate::n8n::{N8nClient, N8nError, OWNER_EMAIL, OwnerSession, SESSION_COOKIE_NAME};

/// A session expiring sooner than this is renewed before a request uses it.
const RENEW_BEFORE: Duration = Duration::from_secs(60 * 60);

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

    /// The session's cookie value, signing in as the owner when none is held or the one
    /// held expires within the hour.
    pub async fn cookie(&self) -> Result<SecretString, N8nError> {
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
    pub async fn renewed(&self, cookie: String) {
        *self.current.lock().await = Some(OwnerSession::from_cookie(cookie));
    }

    /// Forget a session n8n refused.
    pub async fn refused(&self) {
        *self.current.lock().await = None;
    }
}

/// The `Cookie` header of a forwarded request: the browser's cookies without n8n's
/// session, and the owner's session added.
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
