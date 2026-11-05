//! REST + WS + static routes for the session API.
//!
//! `build_router` is the single entry point: every route — including the WS
//! upgrade and the static fallback — sits behind the bearer-token gate.

use std::path::PathBuf;

use axum::{
    extract::{
        ws::{WebSocket, WebSocketUpgrade},
        Path, State,
    },
    http::StatusCode,
    middleware,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;

use crate::model::Session;
use crate::replay::enforcer;
use crate::state::registry::Registry;

use crate::http::{middleware::require_token, static_files};

/// Shared router state: where sessions live and the token that gates access.
#[derive(Clone)]
pub struct AppState {
    pub data_dir: PathBuf,
    pub token: String,
}

pub fn build_router(data_dir: PathBuf, token: String) -> Router {
    let state = AppState { data_dir, token };
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
    (StatusCode::INTERNAL_SERVER_ERROR, format!("{err:#}"))
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

#[derive(Deserialize)]
struct CreateSession {
    name: Option<String>,
    cmd: Option<String>,
    cwd: Option<PathBuf>,
    setup_script: Option<String>,
}

async fn create_session(
    State(state): State<AppState>,
    body: Option<Json<CreateSession>>,
) -> ApiResult<(StatusCode, Json<Session>)> {
    let body = body.map(|Json(body)| body).unwrap_or(CreateSession {
        name: None,
        cmd: None,
        cwd: None,
        setup_script: None,
    });
    let cmd = body.cmd.unwrap_or_else(|| "sh".to_string());
    let name = body.name.unwrap_or_else(|| cmd.clone());
    let cwd = body.cwd.unwrap_or_else(|| PathBuf::from("."));
    // Spawning the PTY is the lifecycle layer's job (TASK-009); the API
    // records the session so it is listed and attachable immediately.
    let session = registry(&state)?
        .create(&name, &cmd, &cwd, body.setup_script)
        .map_err(internal)?;
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

/// Kill persists `Dead`; terminating a live PTY is wired in TASK-009.
async fn kill_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Session>> {
    let mut reg = registry(&state)?;
    session_or_404(&reg, &id)?;
    reg.mark_dead(&id).map_err(internal)?;
    session_or_404(&reg, &id).map(Json)
}

/// Respawn persists `Live`; spawning the replacement shell is TASK-009.
async fn respawn_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Session>> {
    let mut reg = registry(&state)?;
    session_or_404(&reg, &id)?;
    reg.respawn(&id).map_err(internal)?;
    session_or_404(&reg, &id).map(Json)
}

async fn delete_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let mut reg = registry(&state)?;
    session_or_404(&reg, &id)?;
    reg.delete(&id).map_err(internal)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Reset this session's replay log to a fresh header.
async fn clean_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    session_or_404(&registry(&state)?, &id)?;
    let dir = state.data_dir.join("sessions").join(&id);
    if dir.is_dir() {
        enforcer::clean_all(&dir).map_err(|err| internal(err.into()))?;
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Reset every session's replay log; a missing sessions dir means there is
/// nothing to clean, which is success rather than an error.
async fn clean_all_logs(State(state): State<AppState>) -> ApiResult<StatusCode> {
    let sessions = state.data_dir.join("sessions");
    if sessions.is_dir() {
        enforcer::clean_all(&sessions).map_err(|err| internal(err.into()))?;
    }
    Ok(StatusCode::NO_CONTENT)
}

/// WS attach point. Live PTY streaming is TASK-007; the upgrade is accepted
/// here so the auth contract is already enforced end-to-end.
async fn ws_attach(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    match registry(&state).and_then(|reg| session_or_404(&reg, &id)) {
        Ok(_) => ws.on_upgrade(attach).into_response(),
        Err(err) => err.into_response(),
    }
}

/// Placeholder attach: the 101 handshake already completed, so dropping the
/// socket ends the connection cleanly until TASK-007 streams a live PTY.
async fn attach(_socket: WebSocket) {}
