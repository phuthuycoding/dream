//! REST + WS + static routes for the session API.
//!
//! `build_router` is the single entry point: every `/api/` route — including
//! the WS upgrade — sits behind the bearer-token gate; the static fallback
//! serves unauthenticated so the SPA shell can bootstrap (BUG-E2E-001).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    body::Bytes,
    extract::{
        ws::WebSocketUpgrade,
        Path, Query, State,
    },
    http::{header, HeaderMap, HeaderValue, StatusCode},
    middleware,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;

use crate::auth::oauth::{self, GithubOauth};
use crate::auth::session::{SessionSigner, SESSION_COOKIE};
use crate::model::{Session, Status};
use crate::runtime::{RuntimeManager, DEFAULT_IDLE_ATTENTION};
use crate::state::registry::Registry;

use crate::http::{middleware::require_token, static_files};

/// Shared router state: where sessions live, the token that gates access,
/// the lifecycle manager the mutation/attach routes delegate to, and —
/// when the operator configured GitHub OAuth — the client that runs the
/// login flow plus the signer minting `dream_session` cookies.
#[derive(Clone)]
pub struct AppState {
    pub data_dir: PathBuf,
    pub token: String,
    pub runtime: Arc<RuntimeManager>,
    /// `None` keeps the bearer token as the only gate.
    pub oauth: Option<Arc<GithubOauth>>,
    /// Present exactly when `oauth` is — mints and verifies session cookies.
    pub sessions: Option<Arc<SessionSigner>>,
    /// `host:port` the daemon bound — the callback URI's authority when a
    /// request arrives without a usable Host header.
    pub bind_addr: String,
}

/// Standalone entry point (tests): builds its own manager. The daemon
/// uses [`build_router_with`] so the router shares main's one
/// `RuntimeManager` — the manager that ran `resurrect_all` and the
/// enforcer must be the one the routes spawn/attach through, or two
/// monitor sets would track the same sessions. `build_router_with`
/// returns `Result` — a bad session key must fail startup, not panic;
/// this standalone seam unwraps instead (tests run on a fresh data dir)
/// and passes the daemon's default bind as the fallback authority.
pub fn build_router(data_dir: PathBuf, token: String, oauth: Option<Arc<GithubOauth>>) -> Router {
    let runtime = Arc::new(RuntimeManager::new(data_dir.clone(), DEFAULT_IDLE_ATTENTION));
    build_router_with(runtime, data_dir, token, oauth, "127.0.0.1:4020".to_string())
        .expect("session key must load or generate under the data dir")
}

/// `bind_addr` (`host:port`, the configured listen address) backs the
/// callback URI when a request's Host header is missing or malformed.
pub fn build_router_with(
    runtime: Arc<RuntimeManager>,
    data_dir: PathBuf,
    token: String,
    oauth: Option<Arc<GithubOauth>>,
    bind_addr: String,
) -> anyhow::Result<Router> {
    // The signer exists only to serve the oauth flow: minting the
    // callback's cookie and letting the middleware verify it. Its load
    // propagates like `AuthToken::load_or_generate` — an unreadable or
    // corrupt key file is a startup error, not a panic.
    let sessions = oauth
        .as_ref()
        .map(|_| SessionSigner::load_or_generate(&data_dir).map(Arc::new))
        .transpose()?;
    let state = AppState {
        runtime,
        data_dir,
        token,
        oauth,
        sessions,
        bind_addr,
    };
    Ok(Router::new()
        .route("/auth/mode", get(auth_mode))
        .route("/auth/github", get(github_login_start))
        .route("/auth/callback", get(github_login_callback))
        .route("/auth/logout", post(logout))
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
        .with_state(state))
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

// ---------------------------------------------------------------
// GitHub OAuth login flow.
// ---------------------------------------------------------------

/// How long a minted `dream_session` stays valid — the cookie's Max-Age
/// and the signed expiry are set to the same window.
const SESSION_TTL_SECS: u64 = 60 * 60 * 24 * 7;

/// `GET /auth/github` — start the login: mint a CSRF state, carry it in
/// the `dream_oauth_state` cookie and on the authorize redirect so the
/// callback can prove the browser that started the flow is the one
/// returning from GitHub.
async fn github_login_start(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(oauth) = &state.oauth else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let csrf = oauth::generate_state();
    let location = oauth.build_authorize_url(&callback_uri(&headers, &state.bind_addr), &csrf);
    // HttpOnly: no script ever reads the CSRF token. SameSite=Lax: the
    // cookie still rides the top-level navigation back from GitHub.
    // Max-Age=600 caps the window a started login can complete in.
    let state_cookie = format!(
        "{}={csrf}; HttpOnly{}; SameSite=Lax; Max-Age=600; Path=/",
        oauth::STATE_COOKIE,
        secure_attr(&headers)
    );
    redirect_with_cookies(&location, &[state_cookie])
}

#[derive(Deserialize)]
struct CallbackParams {
    code: Option<String>,
    state: Option<String>,
}

/// `GET /auth/callback` — GitHub sent the browser back with a `code` and
/// the echoed `state`. Verify the state against the cookie, exchange the
/// code for an access token, check the login (or an org membership)
/// against the allowlist, then mint `dream_session` and send the browser
/// to the dashboard. Denial is 403 with no cookie; a state mismatch is
/// 400. The consumed state cookie is cleared on every outcome.
async fn github_login_callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<CallbackParams>,
) -> Response {
    let (Some(oauth), Some(signer)) = (&state.oauth, &state.sessions) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let clear_state = clear_cookie(oauth::STATE_COOKIE, &headers);
    let reject = |status: StatusCode| match set_cookie_headers(std::slice::from_ref(&clear_state)) {
        Some(headers) => (status, headers).into_response(),
        None => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let (Some(code), Some(query_state)) = (params.code, params.state) else {
        return reject(StatusCode::BAD_REQUEST);
    };
    let Some(cookie_state) = crate::http::middleware::cookie_value(&headers, oauth::STATE_COOKIE)
    else {
        return reject(StatusCode::BAD_REQUEST);
    };
    if !constant_time_eq(&query_state, &cookie_state) {
        return reject(StatusCode::BAD_REQUEST);
    }

    let redirect_uri = callback_uri(&headers, &state.bind_addr);
    let access_token = match oauth.exchange_code(&code, &redirect_uri).await {
        Ok(token) => token,
        Err(err) => return internal_with_clear(err, &clear_state),
    };
    let login = match oauth.fetch_login(&access_token).await {
        Ok(login) => login,
        Err(err) => return internal_with_clear(err, &clear_state),
    };
    let orgs = match oauth.fetch_orgs(&access_token).await {
        Ok(orgs) => orgs,
        Err(err) => return internal_with_clear(err, &clear_state),
    };
    if !oauth::is_allowlisted(&oauth.allowlist, &login, &orgs) {
        return reject(StatusCode::FORBIDDEN);
    }

    let expires = now_unix() + SESSION_TTL_SECS;
    let session = format!(
        "{SESSION_COOKIE}={}; HttpOnly{}; SameSite=Lax; Path=/; Max-Age={SESSION_TTL_SECS}",
        signer.mint(&login, expires),
        secure_attr(&headers)
    );
    redirect_with_cookies("/", &[session, clear_state])
}

/// `POST /auth/logout` — expire `dream_session`. The middleware whitelists
/// it by path: a stale or absent credential must not lock a client out of
/// logging out. The residual CSRF surface is a forced logout — a nuisance
/// that exposes nothing — narrowed here by Fetch Metadata / `Origin`: a
/// browser POST declaring itself cross-site is refused, while clients
/// sending neither header (curl, other daemons) still pass.
async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !logout_same_site(&headers, &state.bind_addr) {
        return StatusCode::FORBIDDEN.into_response();
    }
    redirect_with_cookies("/", &[clear_cookie(SESSION_COOKIE, &headers)])
}

/// Browser evidence that a logout POST is ours. `Sec-Fetch-Site` decides
/// when present — every browser fetch carries it; an `Origin` matching
/// this request's own scheme+host is the fallback when it is absent.
/// Both missing means a non-browser client, which passes: the accepted
/// residual is a forced logout from a client that forges or omits these
/// headers, which no cookie-only endpoint can fully prevent.
fn logout_same_site(headers: &HeaderMap, bind_addr: &str) -> bool {
    if let Some(site) = headers
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok())
    {
        return site.eq_ignore_ascii_case("same-origin") || site.eq_ignore_ascii_case("same-site");
    }
    match headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    {
        Some(origin) => {
            origin == format!("{}://{}", request_scheme(headers), request_host(headers, bind_addr))
        }
        None => true,
    }
}

/// `GET /auth/mode` — public probe the gate fetches at bootstrap to decide
/// whether the "Sign in with GitHub" button shows. A GET outside `/api/`
/// already passes the middleware's static-shell exemption.
async fn auth_mode(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "github": state.oauth.is_some() }))
}

/// `<scheme>://<host>/auth/callback` rebuilt per request — the scheme
/// comes from `X-Forwarded-Proto` (a TLS front like `tailscale serve`
/// signals https; the daemon itself speaks only cleartext HTTP) and the
/// host from the Host header, falling back to the configured bind
/// address when the header is missing or malformed.
fn callback_uri(headers: &HeaderMap, bind_addr: &str) -> String {
    format!(
        "{}://{}/auth/callback",
        request_scheme(headers),
        request_host(headers, bind_addr)
    )
}

/// `https` when a trusted front terminated TLS and said so via
/// `X-Forwarded-Proto`, else `http`. A direct connection is always
/// cleartext — treating it as https would mint `Secure` cookies the
/// browser then drops (Safari drops them even on localhost: a login
/// loop).
fn request_scheme(headers: &HeaderMap) -> &'static str {
    match headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
    {
        Some(proto) if proto.trim().eq_ignore_ascii_case("https") => "https",
        _ => "http",
    }
}

/// The request's Host authority, or `bind_addr` when the header is
/// missing or not well-formed — the value is embedded verbatim in the
/// callback URI, so anything that could reshape the URL falls back
/// instead.
fn request_host<'a>(headers: &'a HeaderMap, bind_addr: &'a str) -> &'a str {
    headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .filter(|host| is_well_formed_host(host))
        .unwrap_or(bind_addr)
}

/// Well-formed authority = `name[:port]` or `[v6][:port]`: alnum plus
/// `. - : [ ]` and nothing that could reshape the surrounding URL
/// (no `@`, `/`, `%`, whitespace).
fn is_well_formed_host(host: &str) -> bool {
    !host.is_empty()
        && !host.starts_with(':')
        && (!host.starts_with('[') || host.contains(']'))
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']'))
}

/// `; Secure` when the request's effective scheme is https, else "" —
/// mints and clears share this one signal so an attribute mismatch can
/// never leave a stale cookie behind.
fn secure_attr(headers: &HeaderMap) -> &'static str {
    if request_scheme(headers) == "https" {
        "; Secure"
    } else {
        ""
    }
}

/// A Set-Cookie a client reads as "drop this cookie": empty value +
/// Max-Age=0, matching the attributes the cookie was set with — a
/// `Secure` clear over plain http would be dropped exactly like an
/// over-secured mint, leaving the cookie behind.
fn clear_cookie(name: &str, headers: &HeaderMap) -> String {
    format!(
        "{name}=; HttpOnly{}; SameSite=Lax; Path=/; Max-Age=0",
        secure_attr(headers)
    )
}

/// Upstream (GitHub exchange/profile) failures are still an outcome of
/// the flow: 500, and the consumed state cookie is cleared like on every
/// other path so a retry starts a fresh login.
fn internal_with_clear(err: anyhow::Error, clear_state: &str) -> Response {
    let (status, body) = internal(err);
    match set_cookie_headers(&[clear_state.to_string()]) {
        Some(headers) => (status, headers, body).into_response(),
        None => (status, body).into_response(),
    }
}

/// Equality without `!=`'s early exit — the query state is
/// attacker-controlled, so the compare must not leak how much of the
/// CSRF token a guess got right.
fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    a.len() == b.len()
        && a.iter()
            .zip(b.iter())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// 302 with a Location plus one Set-Cookie per entry — axum's `Redirect`
/// has no FOUND constructor and no way to attach cookies.
fn redirect_with_cookies(location: &str, cookies: &[String]) -> Response {
    let mut headers = match set_cookie_headers(cookies) {
        Some(headers) => headers,
        None => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    match HeaderValue::from_str(location) {
        Ok(value) => {
            headers.insert(header::LOCATION, value);
            (StatusCode::FOUND, headers).into_response()
        }
        Err(err) => {
            tracing::error!("cannot build Location header value: {err}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// One `Set-Cookie` header per entry. `None` when a value cannot become
/// a header — built internally, so a failure is a bug to surface as
/// 500, not a response to ship half-empty.
fn set_cookie_headers(cookies: &[String]) -> Option<HeaderMap> {
    let mut headers = HeaderMap::new();
    for cookie in cookies {
        match HeaderValue::from_str(cookie) {
            Ok(value) => {
                headers.append(header::SET_COOKIE, value);
            }
            Err(err) => {
                tracing::error!("cannot build Set-Cookie header value: {err}");
                return None;
            }
        }
    }
    Some(headers)
}
