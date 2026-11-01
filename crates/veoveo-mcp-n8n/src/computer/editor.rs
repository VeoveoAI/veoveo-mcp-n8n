//! What the Hub forwards to the Computer's program `n8n`, `/apps/n8n/…` after the
//! Computer's base path: `open`, which clones a team on demand, applies it and opens a
//! workflow; `teams`, the teams the person reaches; and everything else, n8n's editor,
//! forwarded to n8n's listener without the mount as the owner, whose session the program
//! holds. The Hub already strips the browser's cookies and credentials both ways.
use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    extract::{Query, State},
    http::{HeaderValue, Request, StatusCode, header},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{any, get, post},
};
use secrecy::ExposeSecret;
use serde::Deserialize;
use url::Url;
use veoveo_hub_sdk::client::HubClient;

use super::{
    MOUNT,
    bridge::{self, ComputerBridge},
    hub,
    session::{EditorSession, renewed_session, with_session},
    teams::Checkouts,
};
use crate::{contract::WorkflowName, proxy};

#[derive(Clone)]
pub struct Editor {
    pub http: reqwest::Client,
    /// n8n's loopback listener.
    pub n8n: Url,
    pub session: Arc<EditorSession>,
    pub hub: HubClient,
    pub checkouts: Arc<Checkouts>,
    /// The path the browser reaches the editor under, ending in `/`.
    pub path: String,
}

/// The program's routes on its loopback port.
pub fn router(editor: Editor, bridge: ComputerBridge) -> Router {
    let mount = MOUNT.trim_end_matches('/');
    Router::new()
        .route(&format!("{mount}/open"), get(open))
        .route(&format!("{mount}/teams"), get(teams))
        .route(&format!("{mount}/"), any(forward))
        .route(&format!("{mount}/{{*rest}}"), any(forward))
        .with_state(editor)
        .route(
            &format!("/{}/bridge/calls", crate::server().as_str()),
            post(bridge::call).with_state(bridge),
        )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OpenQuery {
    repository: String,
    #[serde(default)]
    workflow: Option<WorkflowName>,
}

fn page(status: StatusCode, title: &str, text: &str) -> Response {
    let escape = |value: &str| {
        value
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    (
        status,
        Html(format!(
            "<!doctype html><meta charset=\"utf-8\"><title>{}</title><h1>{}</h1><p>{}</p>",
            escape(title),
            escape(title),
            escape(text)
        )),
    )
        .into_response()
}

/// `GET /apps/n8n/open?repository=&workflow=`: the team whose runner names the
/// repository, cloned when the Computer has no checkout of it, its files applied, then
/// the workflow in the editor, or the editor's workflows without one.
async fn open(
    State(editor): State<Editor>,
    query: Result<Query<OpenQuery>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let Ok(Query(query)) = query else {
        return page(
            StatusCode::BAD_REQUEST,
            "Nothing to open",
            "open names a repository and, optionally, a workflow.",
        );
    };
    let teams = match hub::teams(&editor.hub).await {
        Ok(teams) => teams,
        Err(error) => {
            return page(
                StatusCode::BAD_GATEWAY,
                "The Hub could not list your teams",
                &format!("{error:#}"),
            );
        }
    };
    let Some(team) = teams
        .into_iter()
        .find(|team| team.repository() == Some(query.repository.as_str()))
    else {
        return page(
            StatusCode::NOT_FOUND,
            "No team of yours serves that repository",
            "The Hub lists no n8n runner of that repository to you.",
        );
    };
    if let Err(error) = editor.checkouts.checkout(&team).await {
        return page(
            StatusCode::BAD_GATEWAY,
            "The team could not be cloned",
            &format!("{error:#}"),
        );
    }
    let applied = match editor.checkouts.apply(&team.link).await {
        Ok(applied) => applied,
        Err(error) => {
            return page(
                StatusCode::BAD_GATEWAY,
                "The team's workflows could not be applied",
                &format!("{error:#}"),
            );
        }
    };
    let target = match &query.workflow {
        None => format!("{}home/workflows", editor.path),
        Some(workflow) => match applied.iter().find(|(name, _)| name == workflow) {
            Some((_, id)) => format!("{}workflow/{id}", editor.path),
            None => {
                return page(
                    StatusCode::NOT_FOUND,
                    "No such workflow",
                    &format!("The team {} has no {}.", team.link, workflow.file_name()),
                );
            }
        },
    };
    Redirect::to(&target).into_response()
}

/// `GET /apps/n8n/teams`: each team the person reaches, its repository, whether they
/// write it and whether the Computer holds its checkout, with a link that opens it.
async fn teams(State(editor): State<Editor>) -> Response {
    let teams = match hub::teams(&editor.hub).await {
        Ok(teams) => teams,
        Err(error) => {
            return page(
                StatusCode::BAD_GATEWAY,
                "The Hub could not list your teams",
                &format!("{error:#}"),
            );
        }
    };
    let mut rows = String::new();
    for team in &teams {
        let repository = team.repository().unwrap_or_default();
        let checkout = editor.checkouts.checkout_of(&team.link);
        let open: String = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("repository", repository)
            .finish();
        rows.push_str(&format!(
            "<tr><td>{}</td><td><code>{}</code></td><td>{}</td><td>{}</td><td><a href=\"{}open?{}\">Open</a></td></tr>",
            team.link,
            repository,
            if team.status.writer { "writer" } else { "reader" },
            if checkout.join(".lore").exists() { checkout.display().to_string() } else { "not cloned".to_owned() },
            editor.path,
            open,
        ));
    }
    Html(format!(
        "<!doctype html><meta charset=\"utf-8\"><title>Your n8n teams</title><h1>Your n8n teams</h1>\
         <table><tr><th>Team</th><th>Repository</th><th>Grant</th><th>Checkout</th><th></th></tr>{rows}</table>\
         <p>A workflow you save in the editor is written back to its file in the team's checkout. Commit and push it with Lore.</p>"
    ))
    .into_response()
}

/// Everything else under the mount is n8n's editor, without the mount, as the owner.
async fn forward(State(editor): State<Editor>, request: Request<Body>) -> Response {
    let mount = MOUNT.trim_end_matches('/');
    let path = request
        .uri()
        .path()
        .strip_prefix(mount)
        .unwrap_or("/")
        .to_owned();
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
    let method = request.method().clone();
    let mut headers = proxy::forwardable(request.headers());
    headers.remove(header::AUTHORIZATION);
    match with_session(headers.get(header::COOKIE), session.expose_secret()) {
        Some(cookies) => {
            headers.insert(header::COOKIE, cookies);
        }
        None => return StatusCode::BAD_REQUEST.into_response(),
    }
    // The browser's request reaches n8n from its own origin, as n8n's push channel
    // checks; the Hub admitted the browser's origin already.
    if let Ok(origin) = HeaderValue::from_str(editor.n8n.origin().ascii_serialization().as_str()) {
        headers.insert(header::ORIGIN, origin);
    }
    let mut renewed = None;
    let body = proxy::streamed(request.into_body());
    let response = proxy::forward(&editor.http, method, target, headers, body, |answer| {
        renewed = renewed_session(answer);
        answer.remove(header::SET_COOKIE);
    })
    .await;
    if let Some(cookie) = renewed {
        editor.session.renewed(cookie).await;
    }
    if response.status() == StatusCode::UNAUTHORIZED && path.starts_with("/rest/") {
        editor.session.refused().await;
    }
    response
}
