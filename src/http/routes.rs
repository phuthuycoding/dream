//! REST + WS + static routes for the session API.
//!
//! `build_router` is the single entry point: every `/api/` route — including
//! the WS upgrade — sits behind the bearer-token gate; the static fallback
//! serves unauthenticated so the SPA shell can bootstrap (BUG-E2E-001).

use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{
        ws::WebSocketUpgrade,
        Path, State,
    },
    http::StatusCode,
    middleware,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;

use crate::model::{Session, Status};
use crate::runtime::{RuntimeManager, DEFAULT_IDLE_ATTENTION};
use crate::state::registry::Registry;

use crate::http::{middleware::require_token, static_files};

/// Shared router state: where sessions live, the token that gates access,
/// and the lifecycle manager the mutation/attach routes delegate to.
#[derive(Clone)]
pub struct AppState {
    pub data_dir: PathBuf,
    pub token: String,
    pub runtime: Arc<RuntimeManager>,
}

/// Standalone entry point (tests): builds its own manager. The daemon
/// uses [`build_router_with`] so the router shares main's one
/// `RuntimeManager` — the manager that ran `resurrect_all` and the
/// enforcer must be the one the routes spawn/attach through, or two
/// monitor sets would track the same sessions.
pub fn build_router(data_dir: PathBuf, token: String) -> Router {
    let runtime = Arc::new(RuntimeManager::new(data_dir.clone(), DEFAULT_IDLE_ATTENTION));
    build_router_with(runtime, data_dir, token)
}

pub fn build_router_with(
    runtime: Arc<RuntimeManager>,
    data_dir: PathBuf,
    token: String,
) -> Router {
    let state = AppState {
        runtime,
        data_dir,
        token,
    };
    Router::new()
        .route("/api/sessions", get(list_sessions).post(create_session))
        .route(
            "/api/sessions/{id}",
            get(get_session).patch(patch_session).delete(delete_session),
        )
        .route("/api/sessions/{id}/kill", post(kill_session))
        .route("/api/sessions/{id}/respawn", post(respawn_session))
        .route("/api/sessions/{id}/clean", post(clean_session))
        .route("/api/sessions/{id}/ws", get(ws_attach))
        .route("/api/logs/clean", post(clean_all_logs))
        .fallback(static_files::serve)
        .layer(middleware::from_fn_with_state(state.clone(), require_token))
        .with_state(state)
}

type ApiResult<T> = Result<T, (StatusCode, String)>;

fn registry(state: &AppState) -> ApiResult<Registry> {
    Registry::load(&state.data_dir).map_err(internal)
}

fn internal(err: anyhow::Error) -> (StatusCode, String) {
    tracing::error!("api handler failed: {err:#}");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal error".to_string(),
    )
}

fn not_found(id: &str) -> (StatusCode, String) {
    (StatusCode::NOT_FOUND, format!("no session with id {id}"))
}

fn session_or_404(reg: &Registry, id: &str) -> ApiResult<Session> {
    reg.get(id).cloned().ok_or_else(|| not_found(id))
}

async fn list_sessions(State(state): State<AppState>) -> ApiResult<Json<Vec<Session>>> {
    Ok(Json(registry(&state)?.list().to_vec()))
}

async fn get_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Session>> {
    session_or_404(&registry(&state)?, &id).map(Json)
}

#[derive(Deserialize, Default)]
struct CreateSession {
    name: Option<String>,
    cmd: Option<String>,
    cwd: Option<PathBuf>,
    setup_script: Option<String>,
}

/// An empty body means "default session"; a non-empty body must be valid
/// JSON — `Option<Json<T>>` would silently swallow malformed input.
async fn create_session(
    State(state): State<AppState>,
    body: Bytes,
) -> ApiResult<(StatusCode, Json<Session>)> {
    let body = if body.is_empty() {
        CreateSession::default()
    } else {
        serde_json::from_slice(&body).map_err(|err| {
            (
                StatusCode::BAD_REQUEST,
                format!("invalid JSON body: {err}"),
            )
        })?
    };
    // `cmd` is a shell command line run via `sh -c` on spawn; the default
    // is the user's login shell.
    let cmd = body
        .cmd
        .unwrap_or_else(|| std::env::var("SHELL").unwrap_or_else(|_| "sh".to_string()));
    let name = body.name.unwrap_or_else(|| cmd.clone());
    let cwd = body.cwd.unwrap_or_else(|| PathBuf::from("."));
    let session = registry(&state)?
        .create(&name, &cmd, &cwd, body.setup_script)
        .map_err(internal)?;
    // The shell starts on create; a spawn failure leaves the session
    // listed (a later attach retries the spawn) but is still an error.
    state.runtime.spawn(&session).await.map_err(internal)?;
    Ok((StatusCode::CREATED, Json(session)))
}

#[derive(Deserialize)]
struct PatchSession {
    name: Option<String>,
    cwd: Option<PathBuf>,
    setup_script: Option<String>,
}

async fn patch_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(patch): Json<PatchSession>,
) -> ApiResult<Json<Session>> {
    let mut reg = registry(&state)?;
    session_or_404(&reg, &id)?;
    reg.update(&id, patch.name, patch.cwd, patch.setup_script)
        .map_err(internal)?;
    session_or_404(&reg, &id).map(Json)
}

/// Kill terminates the shell and persists `Dead`.
async fn kill_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Session>> {
    session_or_404(&registry(&state)?, &id)?;
    state.runtime.kill(&id).await.map_err(internal)?;
    session_or_404(&registry(&state)?, &id).map(Json)
}

/// Respawn starts a fresh shell (setup script re-runs) and persists `Live`.
async fn respawn_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Session>> {
    let session = session_or_404(&registry(&state)?, &id)?;
    state.runtime.respawn(&session).await.map_err(internal)?;
    session_or_404(&registry(&state)?, &id).map(Json)
}

async fn delete_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let mut reg = registry(&state)?;
    session_or_404(&reg, &id)?;
    // Retire the shell before removing its state dir so a deleted session
    // never leaves a running pty behind.
    state.runtime.kill(&id).await.map_err(internal)?;
    reg.delete(&id).map_err(internal)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Reset this session's replay log to a fresh header. The manager routes
/// the reset through the live runtime's log mutex when one exists — a
/// path-based rewrite would race the pump's appends.
async fn clean_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let session = session_or_404(&registry(&state)?, &id)?;
    state.runtime.clean_log(&session).await.map_err(internal)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Reset every session's replay log. Live runtimes are cleaned through
/// their own log mutex (same reason as `clean_session`); sessions with
/// no runtime get the enforcer's path reset. Per-session failures are
/// aggregated rather than aborting the pass.
async fn clean_all_logs(State(state): State<AppState>) -> ApiResult<StatusCode> {
    let sessions = registry(&state)?.list().to_vec();
    let mut failures = Vec::new();
    for session in &sessions {
        if let Err(err) = state.runtime.clean_log(session).await {
            tracing::warn!(session = %session.id, error = %err, "clean failed");
            failures.push(format!("{}: {err:#}", session.id));
        }
    }
    if failures.is_empty() {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(internal(anyhow::anyhow!(
            "{} log(s) failed to clean: {}",
            failures.len(),
            failures.join("; ")
        )))
    }
}

/// WS attach point. Live PTY streaming is TASK-007; the upgrade is accepted
/// here so the auth contract is already enforced end-to-end.
async fn ws_attach(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    match registry(&state).and_then(|reg| session_or_404(&reg, &id)) {
        Ok(session) => {
            // Go through the manager so its runtime is the one the attach
            // lands on and the attach clears the attention flag; dead
            // sessions skip straight to the replay path, and a spawn
            // failure here is retried by the attach itself.
            if session.status != Status::Dead {
                if let Err(err) = state.runtime.spawn_on_attach(&session).await {
                    tracing::warn!(session = %session.id, error = %err, "pre-attach spawn failed");
                }
            }
            ws.on_upgrade(|socket| crate::ws::attach::attach(socket, session))
                .into_response()
        }
        Err(err) => err.into_response(),
    }
}
