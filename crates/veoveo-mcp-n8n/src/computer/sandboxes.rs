//! The Assistant's sandboxes: the n8n sandbox service's HTTP API (`@n8n/sandbox-client`
//! 0.1.1) on loopback under `/n8n`, n8n's `N8N_SANDBOX_SERVICE_URL`. Each sandbox is a
//! directory of the person's home, `$HOME/n8n/.sandboxes/{id}`, which is the sandbox's
//! home; n8n's fork finds it there through `N8N_SANDBOX_SERVICE_HOMES`, and the Assistant
//! works in its `workspace`. Commands run as the person, with `bash -c` in their own
//! process group, the sandbox's home as `HOME`, the person's npm cache, and n8n's
//! environment without the Computer's credentials and configuration. The file calls stay
//! in the sandbox's directory by their paths. The program admits a request with the token
//! of this start, n8n's `N8N_SANDBOX_SERVICE_API_KEY`, which only n8n's environment holds.
//! A sandbox stays until n8n deletes it; an execution's events stay until n8n deletes it,
//! as the client does once it has read the exit.
use std::{
    collections::{BTreeMap, HashMap},
    ffi::OsString,
    io,
    path::{Component, Path, PathBuf},
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{
        DefaultBodyLimit, Path as UrlPath, Query, Request, State, rejection::QueryRejection,
    },
    http::{HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use nix::{
    sys::signal::{Signal, killpg},
    unistd::Pid,
};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::{io::AsyncReadExt, sync::watch};
use uuid::Uuid;

use crate::n8n::WITHHELD_PREFIXES;

/// The directory under a sandbox's home that the Assistant works in, as n8n's
/// `WORKSPACE_DIR`.
pub const WORKSPACE: &str = "workspace";
/// The most a file call's body may hold.
const MAX_BODY: usize = 64 << 20;
/// The most output one execution keeps; what follows is dropped, and said so once.
const MAX_OUTPUT: usize = 16 << 20;
/// A command's limit when n8n names none, as n8n's own default.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);
/// The header the client sends its API key in.
const API_KEY_HEADER: &str = "x-api-key";

#[derive(Clone)]
pub struct ComputerSandboxes {
    inner: Arc<Inner>,
}

struct Inner {
    /// `$HOME/n8n/.sandboxes`, holding each sandbox's home.
    homes: PathBuf,
    /// The person's npm cache, shared by every sandbox's installs.
    npm_cache: PathBuf,
    token: SecretString,
    /// The environment commands start from.
    environment: Vec<(OsString, OsString)>,
    sandboxes: Mutex<HashMap<Uuid, SandboxState>>,
}

#[derive(Default)]
struct SandboxState {
    ephemeral: bool,
    last_active: u64,
    executions: HashMap<String, Arc<Execution>>,
}

impl ComputerSandboxes {
    /// Sandboxes under `homes`, for the person whose home is `home`, admitted with `token`.
    /// Commands start from this program's environment without the variables n8n never
    /// receives: the withheld prefixes and `withheld`.
    pub fn new(homes: PathBuf, home: &Path, token: SecretString, withheld: &[&str]) -> Self {
        let environment = std::env::vars_os()
            .filter(|(name, _)| {
                name.to_str().is_some_and(|name| {
                    !WITHHELD_PREFIXES
                        .iter()
                        .any(|prefix| name.starts_with(prefix))
                        && !withheld.contains(&name)
                })
            })
            .collect();
        Self {
            inner: Arc::new(Inner {
                homes,
                npm_cache: home.join(".npm"),
                token,
                environment,
                sandboxes: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// The directory holding each sandbox's home, n8n's `N8N_SANDBOX_SERVICE_HOMES`.
    pub fn homes(&self) -> &Path {
        &self.inner.homes
    }

    /// The service's routes under `prefix`, such as `/n8n`.
    pub fn routes(self, prefix: &str) -> Router {
        let sandbox = format!("{prefix}/sandboxes/{{id}}");
        Router::new()
            .route(&format!("{prefix}/sandboxes"), post(create))
            .route(&sandbox, get(read_sandbox).delete(delete_sandbox))
            .route(&format!("{sandbox}/executions"), post(execute))
            .route(
                &format!("{sandbox}/executions/{{exec}}"),
                get(follow).delete(forget),
            )
            .route(
                &format!("{sandbox}/files"),
                get(list)
                    .put(write_file)
                    .post(append_file)
                    .delete(delete_file),
            )
            .route(&format!("{sandbox}/files/content"), get(read_file))
            .route(&format!("{sandbox}/files/copy"), post(copy))
            .route(&format!("{sandbox}/files/move"), post(rename))
            .route(&format!("{sandbox}/mkdir"), post(mkdir))
            .route(&format!("{sandbox}/stat"), get(stat))
            .route_layer(middleware::from_fn_with_state(self.clone(), admit))
            .layer(DefaultBodyLimit::max(MAX_BODY))
            .with_state(self)
    }

    fn home(&self, id: Uuid) -> PathBuf {
        self.inner.homes.join(id.to_string())
    }

    /// The home of the sandbox `id`, which must exist, marked active.
    async fn existing(&self, id: &str) -> Result<(Uuid, PathBuf), Refusal> {
        let id = sandbox_id(id)?;
        let home = self.home(id);
        if !tokio::fs::metadata(&home)
            .await
            .is_ok_and(|metadata| metadata.is_dir())
        {
            return Err(Refusal::new(StatusCode::NOT_FOUND, "sandbox not found"));
        }
        self.state(id, |state| state.last_active = now());
        Ok((id, home))
    }

    fn state<T>(&self, id: Uuid, change: impl FnOnce(&mut SandboxState) -> T) -> T {
        let mut sandboxes = self.inner.sandboxes.lock().expect("the sandboxes' lock");
        change(sandboxes.entry(id).or_default())
    }

    async fn record(&self, id: Uuid) -> Value {
        let home = self.home(id);
        let metadata = tokio::fs::metadata(&home).await.ok();
        let created = metadata
            .as_ref()
            .and_then(|metadata| metadata.created().or_else(|_| metadata.modified()).ok())
            .map_or_else(now, seconds);
        let (ephemeral, last_active) = self.state(id, |state| (state.ephemeral, state.last_active));
        json!({
            "id": id.to_string(),
            "status": "running",
            "created_at": created,
            "last_active_at": last_active.max(created),
            "ephemeral": ephemeral,
        })
    }
}

/// A refused request: its status and the client's `{"error"}` body.
#[derive(Debug)]
struct Refusal(StatusCode, String);

impl Refusal {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self(status, message.into())
    }
}

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error": self.1}))).into_response()
    }
}

impl From<io::Error> for Refusal {
    fn from(error: io::Error) -> Self {
        let status = match error.kind() {
            io::ErrorKind::NotFound => StatusCode::NOT_FOUND,
            io::ErrorKind::AlreadyExists | io::ErrorKind::DirectoryNotEmpty => StatusCode::CONFLICT,
            io::ErrorKind::PermissionDenied => StatusCode::FORBIDDEN,
            io::ErrorKind::IsADirectory
            | io::ErrorKind::NotADirectory
            | io::ErrorKind::InvalidInput => StatusCode::BAD_REQUEST,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        Self(status, error.to_string())
    }
}

fn now() -> u64 {
    seconds(SystemTime::now())
}

fn seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn rfc3339(time: io::Result<SystemTime>) -> String {
    chrono::DateTime::<chrono::Utc>::from(time.unwrap_or(UNIX_EPOCH))
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// A sandbox's id: a lowercase UUID, as the service accepts.
fn sandbox_id(id: &str) -> Result<Uuid, Refusal> {
    Uuid::parse_str(id)
        .ok()
        .filter(|parsed| parsed.hyphenated().to_string() == id)
        .ok_or_else(|| {
            Refusal::new(
                StatusCode::BAD_REQUEST,
                "a sandbox's id is a lowercase UUID",
            )
        })
}

/// `path` in the sandbox whose home is `home`: an absolute path in it, or a path relative to
/// it, without `.` and `..`.
fn inside(home: &Path, path: &str) -> Result<PathBuf, Refusal> {
    if path.is_empty() || path.contains('\0') {
        return Err(Refusal::new(StatusCode::BAD_REQUEST, "a path names a file"));
    }
    let joined = if path.starts_with('/') {
        PathBuf::from(path)
    } else {
        home.join(path)
    };
    let mut normal = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::RootDir => normal.push(Component::RootDir),
            Component::CurDir => {}
            Component::ParentDir => {
                normal.pop();
            }
            Component::Normal(part) => normal.push(part),
            Component::Prefix(_) => {
                return Err(Refusal::new(StatusCode::BAD_REQUEST, "a path names a file"));
            }
        }
    }
    if normal.starts_with(home) {
        Ok(normal)
    } else {
        Err(Refusal::new(
            StatusCode::FORBIDDEN,
            format!("{path} is outside the sandbox"),
        ))
    }
}

async fn admit(
    State(sandboxes): State<ComputerSandboxes>,
    request: Request,
    next: Next,
) -> Response {
    let presented = request
        .headers()
        .get(API_KEY_HEADER)
        .map(HeaderValue::as_bytes);
    if presented != Some(sandboxes.inner.token.expose_secret().as_bytes()) {
        tracing::warn!(
            class = "n8n.assistant_sandbox",
            admitted = false,
            "refused a sandbox request without the Computer's token"
        );
        return Refusal::new(
            StatusCode::UNAUTHORIZED,
            "the key is not this Computer's n8n's",
        )
        .into_response();
    }
    next.run(request).await
}

fn query<T>(query: Result<Query<T>, QueryRejection>) -> Result<T, Refusal> {
    query
        .map(|Query(query)| query)
        .map_err(|rejection| Refusal::new(StatusCode::BAD_REQUEST, rejection.body_text()))
}

fn body<T: for<'de> Deserialize<'de> + Default>(body: &Bytes) -> Result<T, Refusal> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(T::default());
    }
    serde_json::from_slice(body)
        .map_err(|error| Refusal::new(StatusCode::BAD_REQUEST, error.to_string()))
}

// ---------------------------------------------------------------- sandboxes --

#[derive(Default, Deserialize)]
struct CreateRequest {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    ephemeral: Option<bool>,
}

/// `POST /sandboxes`: the sandbox `id`, created with its workspace when it does not exist,
/// or a new one.
async fn create(
    State(sandboxes): State<ComputerSandboxes>,
    request: Bytes,
) -> Result<Response, Refusal> {
    let request: CreateRequest = body(&request)?;
    let id = match &request.id {
        Some(id) => sandbox_id(id)?,
        None => Uuid::now_v7(),
    };
    tokio::fs::create_dir_all(sandboxes.home(id).join(WORKSPACE)).await?;
    sandboxes.state(id, |state| {
        state.ephemeral = request.ephemeral.unwrap_or(false);
        state.last_active = now();
    });
    tracing::info!(class = "n8n.assistant_sandbox", sandbox = %id, "a sandbox of n8n's Assistant");
    Ok((StatusCode::CREATED, Json(sandboxes.record(id).await)).into_response())
}

async fn read_sandbox(
    State(sandboxes): State<ComputerSandboxes>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<Value>, Refusal> {
    let (id, _) = sandboxes.existing(&id).await?;
    Ok(Json(sandboxes.record(id).await))
}

/// `DELETE /sandboxes/{id}`: its executions killed and its directory removed.
async fn delete_sandbox(
    State(sandboxes): State<ComputerSandboxes>,
    UrlPath(id): UrlPath<String>,
) -> Result<StatusCode, Refusal> {
    let (id, home) = sandboxes.existing(&id).await?;
    let executions = sandboxes
        .inner
        .sandboxes
        .lock()
        .expect("the sandboxes' lock")
        .remove(&id)
        .map(|state| state.executions)
        .unwrap_or_default();
    for execution in executions.values() {
        execution.kill();
    }
    tokio::fs::remove_dir_all(home).await?;
    tracing::info!(class = "n8n.assistant_sandbox", sandbox = %id, "deleted a sandbox of n8n's Assistant");
    Ok(StatusCode::NO_CONTENT)
}

// --------------------------------------------------------------- executions --

/// One command's events, one NDJSON line each, whose index is its `seq`.
struct Execution {
    lines: Mutex<Vec<Bytes>>,
    output: Mutex<usize>,
    /// How many lines there are, and whether the last one ended the execution.
    changed: watch::Sender<(usize, bool)>,
    /// The command's process group while it runs.
    group: Mutex<Option<Pid>>,
    killed: AtomicBool,
}

impl Execution {
    fn new() -> Self {
        Self {
            lines: Mutex::new(Vec::new()),
            output: Mutex::new(0),
            changed: watch::Sender::new((0, false)),
            group: Mutex::new(None),
            killed: AtomicBool::new(false),
        }
    }

    fn push(&self, mut event: Value, last: bool) {
        let mut lines = self.lines.lock().expect("the execution's lock");
        if self.changed.borrow().1 {
            return;
        }
        event["seq"] = json!(lines.len());
        let mut line = event.to_string().into_bytes();
        line.push(b'\n');
        lines.push(Bytes::from(line));
        let count = lines.len();
        drop(lines);
        self.changed.send_replace((count, last));
    }

    fn output(&self, stream: &'static str, data: String) {
        let mut kept = self.output.lock().expect("the execution's lock");
        if *kept >= MAX_OUTPUT {
            return;
        }
        *kept += data.len();
        if *kept >= MAX_OUTPUT {
            drop(kept);
            self.push(json!({"type": stream, "data": data}), false);
            self.push(
                json!({"type": "stderr", "data": format!("\n[the sandbox keeps {MAX_OUTPUT} bytes of a command's output and dropped the rest]\n")}),
                false,
            );
            return;
        }
        drop(kept);
        self.push(json!({"type": stream, "data": data}), false);
    }

    /// Kill the command's process group, if it still runs.
    fn kill(&self) {
        self.killed.store(true, Ordering::SeqCst);
        if let Some(group) = *self.group.lock().expect("the execution's lock") {
            let _ = killpg(group, Signal::SIGKILL);
        }
    }

    /// The events after `after`, and, when `follow`, every later one until the last.
    fn stream(self: Arc<Self>, after: Option<u64>, follow: bool) -> Body {
        let cursor = after.map_or(0, |after| {
            usize::try_from(after)
                .unwrap_or(usize::MAX)
                .saturating_add(1)
        });
        let changes = self.changed.subscribe();
        let events = futures::stream::unfold(
            (self, changes, cursor, false),
            move |(execution, mut changes, cursor, ended)| async move {
                if ended {
                    return None;
                }
                loop {
                    let (count, last) = *changes.borrow_and_update();
                    if cursor < count {
                        let lines = execution.lines.lock().expect("the execution's lock");
                        let batch: Vec<u8> = lines[cursor..count].concat();
                        drop(lines);
                        let ended = !follow || last;
                        return Some((
                            Ok::<_, io::Error>(Bytes::from(batch)),
                            (execution, changes, count, ended),
                        ));
                    }
                    if last || !follow || changes.changed().await.is_err() {
                        return None;
                    }
                }
            },
        );
        Body::from_stream(events)
    }
}

#[derive(Default, Deserialize)]
struct ExecRequest {
    #[serde(default)]
    command: String,
    #[serde(default)]
    env: Option<BTreeMap<String, String>>,
    #[serde(default)]
    workdir: Option<String>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    exec_id: Option<String>,
}

fn ndjson(body: Body) -> Response {
    (
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/x-ndjson"),
        )],
        body,
    )
        .into_response()
}

/// `POST /sandboxes/{id}/executions`: the command started, or, for an `exec_id` already
/// started, the same execution's events from the first.
async fn execute(
    State(sandboxes): State<ComputerSandboxes>,
    UrlPath(id): UrlPath<String>,
    request: Bytes,
) -> Result<Response, Refusal> {
    let (id, home) = sandboxes.existing(&id).await?;
    let request: ExecRequest = body(&request)?;
    if request.command.trim().is_empty() {
        return Err(Refusal::new(
            StatusCode::BAD_REQUEST,
            "an execution runs a command",
        ));
    }
    let exec_id = request
        .exec_id
        .clone()
        .unwrap_or_else(|| Uuid::now_v7().to_string());
    let workdir = match &request.workdir {
        Some(workdir) => inside(&home, workdir)?,
        None => home.join(WORKSPACE),
    };
    let (execution, started) = sandboxes.state(id, |state| match state.executions.get(&exec_id) {
        Some(execution) => (execution.clone(), false),
        None => {
            let execution = Arc::new(Execution::new());
            state.executions.insert(exec_id.clone(), execution.clone());
            (execution, true)
        }
    });
    if started {
        execution.push(json!({"type": "started", "exec_id": exec_id}), false);
        let mut command = tokio::process::Command::new("/bin/bash");
        command
            .arg("-c")
            .arg(&request.command)
            .current_dir(&workdir)
            .env_clear()
            .envs(sandboxes.inner.environment.iter().cloned())
            .env("HOME", &home)
            .env("npm_config_cache", &sandboxes.inner.npm_cache)
            .envs(request.env.iter().flatten())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .kill_on_drop(true);
        let timeout = request
            .timeout_ms
            .filter(|timeout| *timeout > 0)
            .map_or(DEFAULT_TIMEOUT, Duration::from_millis);
        tracing::info!(class = "n8n.assistant_sandbox", sandbox = %id, exec = exec_id, "a command of n8n's Assistant");
        tokio::spawn(run(execution.clone(), command, timeout));
    }
    Ok(ndjson(execution.stream(None, true)))
}

async fn run(execution: Arc<Execution>, mut command: tokio::process::Command, timeout: Duration) {
    let started = Instant::now();
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            execution.push(
                json!({"type": "error", "error": format!("the command could not start: {error}")}),
                true,
            );
            return;
        }
    };
    let group = child
        .id()
        .and_then(|pid| i32::try_from(pid).ok())
        .map(Pid::from_raw);
    *execution.group.lock().expect("the execution's lock") = group;
    if execution.killed.load(Ordering::SeqCst)
        && let Some(group) = group
    {
        let _ = killpg(group, Signal::SIGKILL);
    }
    let stdout = child
        .stdout
        .take()
        .map(|out| tokio::spawn(pump(execution.clone(), out, "stdout")));
    let stderr = child
        .stderr
        .take()
        .map(|err| tokio::spawn(pump(execution.clone(), err, "stderr")));
    let mut timed_out = false;
    let status = tokio::select! {
        status = child.wait() => status,
        () = tokio::time::sleep(timeout) => {
            timed_out = true;
            if let Some(group) = group {
                let _ = killpg(group, Signal::SIGKILL);
            }
            child.wait().await
        }
    };
    // What the command left running in its group ends with it, and so do the pipes.
    if let Some(group) = group {
        let _ = killpg(group, Signal::SIGKILL);
    }
    *execution.group.lock().expect("the execution's lock") = None;
    for pump in [stdout, stderr].into_iter().flatten() {
        let _ = pump.await;
    }
    let killed = timed_out || execution.killed.load(Ordering::SeqCst);
    let exit_code = match &status {
        Ok(status) => status.code().unwrap_or_else(|| {
            use std::os::unix::process::ExitStatusExt;
            128 + status.signal().unwrap_or(0)
        }),
        Err(_) => -1,
    };
    if let Err(error) = &status {
        execution.output(
            "stderr",
            format!("waiting for the command failed: {error}\n"),
        );
    }
    execution.push(
        json!({
            "type": "exit",
            "exit_code": exit_code,
            "success": exit_code == 0 && !killed,
            "execution_time_ms": u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            "timed_out": timed_out,
            "killed": killed,
        }),
        true,
    );
}

/// Read `reader` to its end into `stream` events, as UTF-8 text.
async fn pump(
    execution: Arc<Execution>,
    mut reader: impl tokio::io::AsyncRead + Unpin,
    stream: &'static str,
) {
    let mut buffer = vec![0; 16 << 10];
    let mut pending = Vec::new();
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                pending.extend_from_slice(&buffer[..read]);
                let text = take_text(&mut pending);
                if !text.is_empty() {
                    execution.output(stream, text);
                }
            }
        }
    }
    if !pending.is_empty() {
        execution.output(stream, String::from_utf8_lossy(&pending).into_owned());
    }
}

/// The text at the start of `pending`, invalid bytes replaced, leaving an incomplete last
/// character for the next read.
fn take_text(pending: &mut Vec<u8>) -> String {
    let mut text = String::new();
    let mut start = 0;
    loop {
        match std::str::from_utf8(&pending[start..]) {
            Ok(valid) => {
                text.push_str(valid);
                start = pending.len();
                break;
            }
            Err(error) => {
                let valid = start + error.valid_up_to();
                text.push_str(
                    std::str::from_utf8(&pending[start..valid]).expect("the valid prefix"),
                );
                match error.error_len() {
                    Some(invalid) => {
                        text.push(char::REPLACEMENT_CHARACTER);
                        start = valid + invalid;
                    }
                    None => {
                        start = valid;
                        break;
                    }
                }
            }
        }
    }
    pending.drain(..start);
    text
}

#[derive(Deserialize)]
struct FollowQuery {
    #[serde(default)]
    follow: Option<bool>,
    #[serde(default)]
    after: Option<u64>,
}

/// `GET /sandboxes/{id}/executions/{exec}?follow=&after=`: the execution's events after
/// `after`, and, when following, every later one until the last.
async fn follow(
    State(sandboxes): State<ComputerSandboxes>,
    UrlPath((id, exec)): UrlPath<(String, String)>,
    query_: Result<Query<FollowQuery>, QueryRejection>,
) -> Result<Response, Refusal> {
    let request = query(query_)?;
    let (id, _) = sandboxes.existing(&id).await?;
    let execution = sandboxes
        .state(id, |state| state.executions.get(&exec).cloned())
        .ok_or_else(|| Refusal::new(StatusCode::NOT_FOUND, "execution not found"))?;
    Ok(ndjson(
        execution.stream(request.after, request.follow.unwrap_or(false)),
    ))
}

/// `DELETE /sandboxes/{id}/executions/{exec}`: the command killed if it runs, and its
/// events forgotten.
async fn forget(
    State(sandboxes): State<ComputerSandboxes>,
    UrlPath((id, exec)): UrlPath<(String, String)>,
) -> Result<StatusCode, Refusal> {
    let (id, _) = sandboxes.existing(&id).await?;
    let execution = sandboxes
        .state(id, |state| state.executions.remove(&exec))
        .ok_or_else(|| Refusal::new(StatusCode::NOT_FOUND, "execution not found"))?;
    execution.kill();
    Ok(StatusCode::NO_CONTENT)
}

// -------------------------------------------------------------------- files --

#[derive(Deserialize)]
struct PathQuery {
    path: String,
    #[serde(default)]
    overwrite: Option<bool>,
    #[serde(default)]
    recursive: Option<bool>,
    #[serde(default)]
    force: Option<bool>,
}

async fn read_file(
    State(sandboxes): State<ComputerSandboxes>,
    UrlPath(id): UrlPath<String>,
    query_: Result<Query<PathQuery>, QueryRejection>,
) -> Result<Response, Refusal> {
    let request = query(query_)?;
    let (_, home) = sandboxes.existing(&id).await?;
    let path = inside(&home, &request.path)?;
    if tokio::fs::metadata(&path).await?.is_dir() {
        return Err(Refusal::new(
            StatusCode::BAD_REQUEST,
            format!("{} is a directory", request.path),
        ));
    }
    let content = tokio::fs::read(&path).await?;
    Ok((
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/octet-stream"),
        )],
        content,
    )
        .into_response())
}

/// `PUT /sandboxes/{id}/files?path=&overwrite=`: the body as the file, its directories
/// created.
async fn write_file(
    State(sandboxes): State<ComputerSandboxes>,
    UrlPath(id): UrlPath<String>,
    query_: Result<Query<PathQuery>, QueryRejection>,
    content: Bytes,
) -> Result<StatusCode, Refusal> {
    let request = query(query_)?;
    let (_, home) = sandboxes.existing(&id).await?;
    let path = inside(&home, &request.path)?;
    if !request.overwrite.unwrap_or(true) && tokio::fs::symlink_metadata(&path).await.is_ok() {
        return Err(Refusal::new(
            StatusCode::CONFLICT,
            format!("{} exists", request.path),
        ));
    }
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(&path, content).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /sandboxes/{id}/files?path=`: the body appended to the file, which it creates.
async fn append_file(
    State(sandboxes): State<ComputerSandboxes>,
    UrlPath(id): UrlPath<String>,
    query_: Result<Query<PathQuery>, QueryRejection>,
    content: Bytes,
) -> Result<StatusCode, Refusal> {
    use tokio::io::AsyncWriteExt;
    let request = query(query_)?;
    let (_, home) = sandboxes.existing(&id).await?;
    let path = inside(&home, &request.path)?;
    let mut file = tokio::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&path)
        .await?;
    file.write_all(&content).await?;
    file.flush().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /sandboxes/{id}/files?path=&recursive=&force=`: a file, or a directory, with
/// what it holds when `recursive`; a missing one is no error when `force`.
async fn delete_file(
    State(sandboxes): State<ComputerSandboxes>,
    UrlPath(id): UrlPath<String>,
    query_: Result<Query<PathQuery>, QueryRejection>,
) -> Result<StatusCode, Refusal> {
    let request = query(query_)?;
    let (_, home) = sandboxes.existing(&id).await?;
    let path = inside(&home, &request.path)?;
    if path == home {
        return Err(Refusal::new(
            StatusCode::BAD_REQUEST,
            "a sandbox keeps its home",
        ));
    }
    let metadata = match tokio::fs::symlink_metadata(&path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound && request.force.unwrap_or(false) => {
            return Ok(StatusCode::NO_CONTENT);
        }
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_dir() {
        tokio::fs::remove_file(&path).await?;
    } else if request.recursive.unwrap_or(false) {
        tokio::fs::remove_dir_all(&path).await?;
    } else {
        tokio::fs::remove_dir(&path).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Default, Deserialize)]
struct CopyRequest {
    #[serde(default)]
    src: String,
    #[serde(default)]
    dest: String,
    #[serde(default)]
    recursive: bool,
    #[serde(default)]
    overwrite: bool,
}

/// The source and destination of a copy or a move in the sandbox, refused when the
/// destination exists and is not to be overwritten.
async fn source_and_destination(
    sandboxes: &ComputerSandboxes,
    id: &str,
    request: &CopyRequest,
) -> Result<(PathBuf, PathBuf), Refusal> {
    let (_, home) = sandboxes.existing(id).await?;
    let source = inside(&home, &request.src)?;
    let destination = inside(&home, &request.dest)?;
    tokio::fs::symlink_metadata(&source).await?;
    if tokio::fs::symlink_metadata(&destination).await.is_ok() && !request.overwrite {
        return Err(Refusal::new(
            StatusCode::CONFLICT,
            format!("{} exists", request.dest),
        ));
    }
    if let Some(parent) = destination.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    Ok((source, destination))
}

async fn copy(
    State(sandboxes): State<ComputerSandboxes>,
    UrlPath(id): UrlPath<String>,
    request: Bytes,
) -> Result<StatusCode, Refusal> {
    let request: CopyRequest = body(&request)?;
    let (source, destination) = source_and_destination(&sandboxes, &id, &request).await?;
    if tokio::fs::metadata(&source).await?.is_dir() {
        if !request.recursive {
            return Err(Refusal::new(
                StatusCode::BAD_REQUEST,
                format!("{} is a directory", request.src),
            ));
        }
        tokio::task::spawn_blocking(move || copy_tree(&source, &destination))
            .await
            .map_err(|error| {
                Refusal::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
            })??;
    } else {
        tokio::fs::copy(&source, &destination).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

fn copy_tree(source: &Path, destination: &Path) -> io::Result<()> {
    std::fs::create_dir_all(destination)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let target = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

async fn rename(
    State(sandboxes): State<ComputerSandboxes>,
    UrlPath(id): UrlPath<String>,
    request: Bytes,
) -> Result<StatusCode, Refusal> {
    let request: CopyRequest = body(&request)?;
    let (source, destination) = source_and_destination(&sandboxes, &id, &request).await?;
    if let Ok(existing) = tokio::fs::symlink_metadata(&destination).await {
        if existing.is_dir() {
            tokio::fs::remove_dir_all(&destination).await?;
        } else {
            tokio::fs::remove_file(&destination).await?;
        }
    }
    tokio::fs::rename(&source, &destination).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn mkdir(
    State(sandboxes): State<ComputerSandboxes>,
    UrlPath(id): UrlPath<String>,
    query_: Result<Query<PathQuery>, QueryRejection>,
) -> Result<StatusCode, Refusal> {
    let request = query(query_)?;
    let (_, home) = sandboxes.existing(&id).await?;
    let path = inside(&home, &request.path)?;
    if request.recursive.unwrap_or(false) {
        tokio::fs::create_dir_all(&path).await?;
    } else {
        tokio::fs::create_dir(&path).await?;
    }
    Ok(StatusCode::CREATED)
}

#[derive(Deserialize)]
struct ListQuery {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    recursive: Option<bool>,
    #[serde(default)]
    extension: Option<String>,
}

/// `GET /sandboxes/{id}/files?path=&recursive=&extension=`: the directory's entries, by
/// their paths relative to it, files only of the extension when one is named.
async fn list(
    State(sandboxes): State<ComputerSandboxes>,
    UrlPath(id): UrlPath<String>,
    query_: Result<Query<ListQuery>, QueryRejection>,
) -> Result<Json<Value>, Refusal> {
    let request = query(query_)?;
    let (_, home) = sandboxes.existing(&id).await?;
    let directory = match &request.path {
        Some(path) => inside(&home, path)?,
        None => home.clone(),
    };
    let recursive = request.recursive.unwrap_or(false);
    let extension = request
        .extension
        .filter(|extension| !extension.is_empty())
        .map(|extension| {
            if extension.starts_with('.') {
                extension
            } else {
                format!(".{extension}")
            }
        });
    let entries = tokio::task::spawn_blocking(move || {
        let mut entries = Vec::new();
        list_tree(&directory, Path::new(""), recursive, &mut entries)?;
        Ok::<_, io::Error>(entries)
    })
    .await
    .map_err(|error| Refusal::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))??;
    let entries: Vec<Value> = entries
        .into_iter()
        .filter(|(name, is_dir, _)| {
            extension
                .as_ref()
                .is_none_or(|extension| !is_dir && name.ends_with(extension.as_str()))
        })
        .map(|(name, is_dir, metadata)| {
            json!({
                "name": name,
                "size": metadata.len(),
                "is_dir": is_dir,
                "type": if is_dir { "directory" } else { "file" },
                "mod_time": rfc3339(metadata.modified()),
            })
        })
        .collect();
    Ok(Json(Value::Array(entries)))
}

fn list_tree(
    directory: &Path,
    relative: &Path,
    recursive: bool,
    entries: &mut Vec<(String, bool, std::fs::Metadata)>,
) -> io::Result<()> {
    let mut found: Vec<_> = std::fs::read_dir(directory)?.collect::<io::Result<_>>()?;
    found.sort_by_key(std::fs::DirEntry::file_name);
    for entry in found {
        let metadata = entry.metadata()?;
        let name = relative.join(entry.file_name());
        let is_dir = metadata.is_dir();
        entries.push((name.to_string_lossy().into_owned(), is_dir, metadata));
        if recursive && is_dir {
            list_tree(&entry.path(), &name, true, entries)?;
        }
    }
    Ok(())
}

async fn stat(
    State(sandboxes): State<ComputerSandboxes>,
    UrlPath(id): UrlPath<String>,
    query_: Result<Query<PathQuery>, QueryRejection>,
) -> Result<Json<Value>, Refusal> {
    let request = query(query_)?;
    let (_, home) = sandboxes.existing(&id).await?;
    let path = inside(&home, &request.path)?;
    let metadata = tokio::fs::metadata(&path).await?;
    Ok(Json(json!({
        "name": path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default(),
        "path": path.display().to_string(),
        "type": if metadata.is_dir() { "directory" } else { "file" },
        "size": metadata.len(),
        "created_at": rfc3339(metadata.created().or_else(|_| metadata.modified())),
        "modified_at": rfc3339(metadata.modified()),
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_stay_in_the_sandbox() {
        let home = Path::new("/sandbox/n8n/.sandboxes/0194a2b0-0000-7000-8000-000000000001");
        assert_eq!(
            inside(
                home,
                "/sandbox/n8n/.sandboxes/0194a2b0-0000-7000-8000-000000000001/workspace/src/a.ts"
            )
            .unwrap(),
            home.join("workspace/src/a.ts")
        );
        assert_eq!(
            inside(home, "workspace/./b").unwrap(),
            home.join("workspace/b")
        );
        assert_eq!(
            inside(home, "workspace/../workspace").unwrap(),
            home.join("workspace")
        );
        for outside in [
            "/sandbox/.veoveo/n8n/owner-password",
            "../0194a2b0-0000-7000-8000-000000000002/workspace",
            "/sandbox/n8n/.sandboxes/0194a2b0-0000-7000-8000-0000000000011",
            "/home/user/workspace",
            "workspace/../../..",
        ] {
            assert_eq!(
                inside(home, outside).unwrap_err().0,
                StatusCode::FORBIDDEN,
                "{outside}"
            );
        }
        assert_eq!(inside(home, "").unwrap_err().0, StatusCode::BAD_REQUEST);
        assert_eq!(inside(home, "a\0b").unwrap_err().0, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn a_sandbox_is_named_by_a_lowercase_uuid() {
        assert!(sandbox_id("0194a2b0-0000-7000-8000-000000000001").is_ok());
        for id in [
            "0194A2B0-0000-7000-8000-000000000001",
            "..",
            "0194a2b000007000800000000000001",
            "x",
        ] {
            assert!(sandbox_id(id).is_err(), "{id}");
        }
    }

    #[test]
    fn output_is_text_across_reads() {
        let mut pending = "añ".as_bytes().to_vec();
        let last = pending.pop().unwrap();
        assert_eq!(take_text(&mut pending), "a");
        assert_eq!(pending.len(), 1);
        pending.push(last);
        pending.extend_from_slice(&[0xff, b'z']);
        assert_eq!(take_text(&mut pending), "ñ\u{fffd}z");
        assert!(pending.is_empty());
    }
}
