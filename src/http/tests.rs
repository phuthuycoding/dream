//! Integration tests for the HTTP API + token auth (TC-011, TC-014).
//!
//! A real axum server is spawned per test on a loopback ephemeral port and
//! driven with reqwest / tokio-tungstenite — `tower` is not a direct
//! dependency of this crate, so `tower::ServiceExt::oneshot` is unavailable
//! without a Cargo.toml change that belongs to the dev seat.
//!
//! Assumed surface (the dev pass reconciles — task-006-tests-report.md):
//!   - `dream::http::routes::build_router(data_dir: PathBuf, token: String)
//!      -> axum::Router` builds the full app (REST + static + WS + middleware)
//!   - WS attach lives at `GET /api/sessions/{id}/ws`, authed via `?token=`
//!   - GET /api/sessions returns a bare JSON array of serialized `Session`s
//!   - mutation endpoints return any 2xx on success; GET of a deleted
//!     session returns 404
//!   - `crate::auth::oauth::GithubOauth { client_id, client_secret,
//!     allowlist, authorize_url, token_url, api_url }` — the struct
//!     literal the auth tests already pin, endpoints injectable so GitHub
//!     can be pointed at a loopback stub
//!   - `build_router(data_dir, token, Option<Arc<GithubOauth>>)` — the
//!     oauth-aware signature; reconciled in `build_app_with_oauth`

#![cfg(test)]

use std::fs;
use std::path::Path;
use std::sync::Arc;

use reqwest::StatusCode;
use serde_json::{json, Value};
use tempfile::TempDir;

use crate::auth::oauth::GithubOauth;
use crate::model::{Session, Status};
use crate::state::registry::Registry;

const TOKEN: &str = "test-token-9f8e7d6c5b4a3210";

/// ASSUMED constructor — one place to fix when the real signature lands.
fn build_app(data_dir: &Path) -> axum::Router {
    crate::http::routes::build_router(data_dir.to_path_buf(), TOKEN.to_string(), None)
}

async fn spawn_server(data_dir: &Path) -> String {
    let app = build_app(data_dir);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback ephemeral port");
    let base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app.into_make_service())
            .await
            .expect("serve");
    });
    base
}

/// Seed a session through the TASK-002 Registry API; the server under test
/// loads it from the same data dir (registry-level sessions have no live PTY).
fn seed_session(data_dir: &Path, name: &str) -> Session {
    let mut registry = Registry::load(data_dir).expect("load seed registry");
    registry
        .create(name, "sh", data_dir, Some("echo seeded".to_string()))
        .expect("seed session")
}

fn session_on_disk(data_dir: &Path, id: &str) -> Session {
    let path = data_dir.join("sessions").join(id).join("session.json");
    let raw = fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("cannot read {}: {err}", path.display()));
    serde_json::from_str(&raw).expect("session.json must deserialize into Session")
}

async fn session_ids(client: &reqwest::Client, base: &str) -> Vec<String> {
    let resp = client
        .get(format!("{base}/api/sessions"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .expect("list request");
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = resp.json().await.expect("json body");
    body.as_array()
        .expect("GET /api/sessions must return a JSON array")
        .iter()
        .map(|session| {
            session["id"]
                .as_str()
                .expect("each session carries an id")
                .to_string()
        })
        .collect()
}

fn ws_upgrade_request(client: &reqwest::Client, url: &str) -> reqwest::RequestBuilder {
    client
        .get(url)
        .header("connection", "upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
}

// ---------------------------------------------------------------- TC-011

#[tokio::test]
async fn requests_without_a_token_are_rejected_with_401() {
    let data = TempDir::new().expect("tempdir");
    let session = seed_session(data.path(), "guarded");
    let base = spawn_server(data.path()).await;
    let client = reqwest::Client::new();

    // The token gate applies to `/api/*` and the WS upgrade only; the
    // static shell is public because browser subresource fetches
    // (`<script src>`, `<link>`) cannot carry auth headers (BUG-E2E-001).
    for url in [format!("{base}/"), format!("{base}/app.js")] {
        let resp = client.get(&url).send().await.expect("request");
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "GET {url} without a token must serve the public static shell"
        );
    }

    for (method, url) in [
        ("GET", format!("{base}/api/sessions")),
        ("POST", format!("{base}/api/sessions")),
        ("GET", format!("{base}/api/sessions/{}", session.id)),
        ("PATCH", format!("{base}/api/sessions/{}", session.id)),
        ("DELETE", format!("{base}/api/sessions/{}", session.id)),
        ("POST", format!("{base}/api/sessions/{}/kill", session.id)),
        (
            "POST",
            format!("{base}/api/sessions/{}/respawn", session.id),
        ),
        ("POST", format!("{base}/api/sessions/{}/clean", session.id)),
        ("POST", format!("{base}/api/logs/clean")),
    ] {
        let resp = client
            .request(method.parse().expect("http method"), &url)
            .send()
            .await
            .expect("request");
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "{method} {url} without a token must be 401"
        );
    }
}

#[tokio::test]
async fn requests_with_a_wrong_token_are_rejected_with_401() {
    let data = TempDir::new().expect("tempdir");
    let base = spawn_server(data.path()).await;
    let client = reqwest::Client::new();

    // A wrong bearer cannot block the public static shell.
    let resp = client
        .get(format!("{base}/"))
        .bearer_auth("wrong-token")
        .send()
        .await
        .expect("request");
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "GET / with a wrong bearer still serves the public shell"
    );

    let resp = client
        .get(format!("{base}/api/sessions"))
        .bearer_auth("wrong-token")
        .send()
        .await
        .expect("request");
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "GET /api/sessions with a wrong bearer token must be 401"
    );
}

#[tokio::test]
async fn websocket_upgrade_without_a_valid_token_is_rejected_with_401() {
    let data = TempDir::new().expect("tempdir");
    let session = seed_session(data.path(), "ws-guard");
    let base = spawn_server(data.path()).await;
    let client = reqwest::Client::new();

    // No token at all.
    let resp = ws_upgrade_request(&client, &format!("{base}/api/sessions/{}/ws", session.id))
        .send()
        .await
        .expect("ws upgrade request");
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "unauthenticated WS upgrade must be rejected before the handshake"
    );

    // Wrong token via the WS query channel.
    let resp = ws_upgrade_request(
        &client,
        &format!("{base}/api/sessions/{}/ws?token=wrong-token", session.id),
    )
    .send()
    .await
    .expect("ws upgrade request");
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // Wrong token via Bearer must not unlock the WS route either.
    let resp = ws_upgrade_request(&client, &format!("{base}/api/sessions/{}/ws", session.id))
        .bearer_auth("wrong-token")
        .send()
        .await
        .expect("ws upgrade request");
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn authenticated_requests_return_the_dashboard_and_session_list() {
    let data = TempDir::new().expect("tempdir");
    let session = seed_session(data.path(), "alpha");
    let base = spawn_server(data.path()).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{base}/"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .expect("dashboard request");
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "authenticated GET / must serve the dashboard shell"
    );

    let ids = session_ids(&client, &base).await;
    assert!(
        ids.contains(&session.id),
        "GET /api/sessions must list the seeded session, got {ids:?}"
    );
}

#[tokio::test]
async fn websocket_upgrade_with_a_valid_query_token_passes_auth() {
    let data = TempDir::new().expect("tempdir");
    let session = seed_session(data.path(), "ws-attach");
    let base = spawn_server(data.path()).await;

    let url = format!(
        "{}/api/sessions/{}/ws?token={TOKEN}",
        base.replacen("http://", "ws://", 1),
        session.id
    );
    // The auth contract is what this test pins: a valid `?token=` must not
    // be answered with 401. A full 101 additionally depends on TASK-007's
    // attach handler accepting a registry-only session (no live PTY).
    match tokio_tungstenite::connect_async(&url).await {
        Ok((_socket, resp)) => assert_eq!(resp.status(), StatusCode::SWITCHING_PROTOCOLS),
        Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => assert_ne!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "a valid ?token= must pass the auth middleware"
        ),
        Err(err) => panic!("unexpected WS connect outcome for {url}: {err}"),
    }
}

// ---------------------------------------------------------------- TC-014

#[tokio::test]
async fn kill_marks_the_session_dead_on_disk_and_keeps_it_listed() {
    let data = TempDir::new().expect("tempdir");
    let session = seed_session(data.path(), "victim");
    let base = spawn_server(data.path()).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{base}/api/sessions/{}/kill", session.id))
        .bearer_auth(TOKEN)
        .send()
        .await
        .expect("kill request");
    assert!(
        resp.status().is_success(),
        "kill must succeed, got {}",
        resp.status()
    );

    let on_disk = session_on_disk(data.path(), &session.id);
    assert_eq!(
        on_disk.status,
        Status::Dead,
        "kill must persist status Dead to session.json"
    );

    let resp = client
        .get(format!("{base}/api/sessions/{}", session.id))
        .bearer_auth(TOKEN)
        .send()
        .await
        .expect("get request");
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "a killed session stays listed (kill != delete)"
    );
    let body: Value = resp.json().await.expect("json body");
    assert_eq!(body["status"].as_str(), Some("Dead"));
    assert!(session_ids(&client, &base).await.contains(&session.id));
}

#[tokio::test]
async fn patch_persists_name_cwd_and_setup_script_to_session_json() {
    let data = TempDir::new().expect("tempdir");
    let new_cwd = TempDir::new().expect("cwd tempdir");
    let session = seed_session(data.path(), "before-rename");
    let base = spawn_server(data.path()).await;
    let client = reqwest::Client::new();

    let resp = client
        .patch(format!("{base}/api/sessions/{}", session.id))
        .bearer_auth(TOKEN)
        .json(&json!({
            "name": "after-rename",
            "cwd": new_cwd.path(),
            "setup_script": "export Z=1",
        }))
        .send()
        .await
        .expect("patch request");
    assert!(
        resp.status().is_success(),
        "patch must succeed, got {}",
        resp.status()
    );

    let on_disk = session_on_disk(data.path(), &session.id);
    assert_eq!(on_disk.name, "after-rename");
    assert_eq!(on_disk.cwd, new_cwd.path());
    assert_eq!(on_disk.setup_script.as_deref(), Some("export Z=1"));

    let resp = client
        .get(format!("{base}/api/sessions/{}", session.id))
        .bearer_auth(TOKEN)
        .send()
        .await
        .expect("get request");
    let body: Value = resp.json().await.expect("json body");
    assert_eq!(body["name"].as_str(), Some("after-rename"));
}

#[tokio::test]
async fn respawn_returns_a_dead_session_to_live() {
    let data = TempDir::new().expect("tempdir");
    let session = seed_session(data.path(), "phoenix");
    Registry::load(data.path())
        .and_then(|mut reg| reg.mark_dead(&session.id))
        .expect("seed a dead session");
    let base = spawn_server(data.path()).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{base}/api/sessions/{}/respawn", session.id))
        .bearer_auth(TOKEN)
        .send()
        .await
        .expect("respawn request");
    assert!(
        resp.status().is_success(),
        "respawn must succeed, got {}",
        resp.status()
    );

    let on_disk = session_on_disk(data.path(), &session.id);
    assert_eq!(
        on_disk.status,
        Status::Live,
        "respawn must persist status Live to session.json"
    );

    let resp = client
        .get(format!("{base}/api/sessions/{}", session.id))
        .bearer_auth(TOKEN)
        .send()
        .await
        .expect("get request");
    let body: Value = resp.json().await.expect("json body");
    assert_eq!(body["status"].as_str(), Some("Live"));
}

#[tokio::test]
async fn delete_removes_the_session_and_its_state_dir() {
    let data = TempDir::new().expect("tempdir");
    let session = seed_session(data.path(), "goner");
    let state_dir = data.path().join("sessions").join(&session.id);
    assert!(state_dir.is_dir(), "seed must create the state dir");
    let base = spawn_server(data.path()).await;
    let client = reqwest::Client::new();

    let resp = client
        .delete(format!("{base}/api/sessions/{}", session.id))
        .bearer_auth(TOKEN)
        .send()
        .await
        .expect("delete request");
    assert!(
        resp.status().is_success(),
        "delete must succeed, got {}",
        resp.status()
    );

    assert!(
        !state_dir.exists(),
        "delete must remove the whole per-session state dir"
    );
    assert!(
        !session_ids(&client, &base).await.contains(&session.id),
        "a deleted session must drop out of the list"
    );

    let resp = client
        .get(format!("{base}/api/sessions/{}", session.id))
        .bearer_auth(TOKEN)
        .send()
        .await
        .expect("get request");
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "fetching a deleted session must be 404"
    );
}

// ---------------------------------------------------------------
// GitHub OAuth login: `GET /auth/github` hands the browser a
// `dream_oauth_state` CSRF cookie and 302s to the authorize URL;
// `GET /auth/callback` verifies the state, exchanges the code and
// checks the allowlist against a stub GitHub before minting the
// `dream_session` cookie; `POST /auth/logout` expires it. The
// middleware accepts the session cookie next to the bearer token.
// ---------------------------------------------------------------

/// ASSUMED oauth-aware router — the dev pass reconciles this single call
/// when the real signature lands (`AppState` gains
/// `Option<Arc<GithubOauth>>`; today's `build_router` takes none).
fn build_app_with_oauth(data_dir: &Path, oauth: GithubOauth) -> axum::Router {
    crate::http::routes::build_router(
        data_dir.to_path_buf(),
        TOKEN.to_string(),
        Some(Arc::new(oauth)),
    )
}

async fn spawn_oauth_server(data_dir: &Path, oauth: GithubOauth) -> String {
    let app = build_app_with_oauth(data_dir, oauth);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback ephemeral port");
    let base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app.into_make_service())
            .await
            .expect("serve");
    });
    base
}

/// A stub GitHub on loopback serving a fixed user: the authorize page
/// (never fetched — the test asserts the 302 target), the token
/// exchange, `/user` and `/user/orgs` the callback handler calls.
async fn spawn_github_stub(login: &'static str, orgs: Vec<&'static str>) -> String {
    let app = axum::Router::new()
        .route(
            "/login/oauth/authorize",
            axum::routing::get(|| async { StatusCode::OK }),
        )
        .route(
            "/login/oauth/access_token",
            axum::routing::post(|| async {
                axum::Json(json!({
                    "access_token": "stub-access-token",
                    "token_type": "bearer",
                    "scope": "read:user read:org",
                }))
            }),
        )
        .route(
            "/user",
            axum::routing::get(move || async move { axum::Json(json!({ "login": login })) }),
        )
        .route(
            "/user/orgs",
            axum::routing::get(move || async move {
                axum::Json(json!(orgs
                    .iter()
                    .map(|org| json!({ "login": org }))
                    .collect::<Vec<_>>()))
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind stub github port");
    let base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app.into_make_service())
            .await
            .expect("serve stub github");
    });
    base
}

/// A `GithubOauth` whose three GitHub endpoints all point at the stub —
/// the struct literal the auth unit tests already pin.
fn stub_oauth(stub: &str, allowlist: &[&str]) -> GithubOauth {
    GithubOauth {
        client_id: "client-id-123".to_string(),
        client_secret: "client-secret-xyz".to_string(),
        allowlist: allowlist.iter().map(|entry| entry.to_string()).collect(),
        authorize_url: format!("{stub}/login/oauth/authorize"),
        token_url: format!("{stub}/login/oauth/access_token"),
        api_url: stub.to_string(),
    }
}

/// The login flow's 302s must be asserted, never followed — reqwest
/// follows redirects by default.
fn no_redirect_client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("client")
}

/// Every `Set-Cookie` header for one cookie name.
fn set_cookies_named(resp: &reqwest::Response, name: &str) -> Vec<String> {
    let prefix = format!("{name}=");
    resp.headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|value| value.to_str().ok().map(str::to_string))
        .filter(|header| header.trim_start().starts_with(&prefix))
        .collect()
}

/// The value inside a `name=value; attrs` Set-Cookie header.
fn cookie_value(header: &str) -> &str {
    header
        .split(';')
        .next()
        .and_then(|pair| pair.split_once('='))
        .map(|(_, value)| value)
        .unwrap_or("")
}

/// A Set-Cookie a client reads as "drop this cookie": an empty value or
/// an explicit `Max-Age=0`.
fn clears_cookie(header: &str) -> bool {
    cookie_value(header).is_empty() || header.to_ascii_lowercase().contains("max-age=0")
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&input[i + 1..i + 3], 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1;
    }
    String::from_utf8(out).expect("a percent-decoded param must be utf-8")
}

/// One percent-decoded parameter out of a URL's query string.
fn query_param(url: &str, key: &str) -> Option<String> {
    url.split_once('?')?.1.split('&').find_map(|pair| {
        pair.split_once('=')
            .filter(|(k, _)| *k == key)
            .map(|(_, v)| percent_decode(v))
    })
}

/// `GET /auth/github`, returning the CSRF state — the same value must
/// ride the redirect URL and the `dream_oauth_state` cookie.
async fn start_login(client: &reqwest::Client, base: &str) -> String {
    let resp = client
        .get(format!("{base}/auth/github"))
        .send()
        .await
        .expect("login start request");
    assert_eq!(
        resp.status(),
        StatusCode::FOUND,
        "GET /auth/github must 302 to the authorize url"
    );
    let location = resp
        .headers()
        .get("location")
        .and_then(|value| value.to_str().ok())
        .expect("the 302 must carry a Location header")
        .to_string();
    let state =
        query_param(&location, "state").expect("the authorize url must carry a state parameter");
    assert!(
        !state.is_empty(),
        "state is a CSRF token — it must be non-empty"
    );
    let state_cookie = set_cookies_named(&resp, "dream_oauth_state")
        .into_iter()
        .next()
        .expect("login start must Set-Cookie the oauth state");
    assert_eq!(
        cookie_value(&state_cookie),
        state,
        "the state cookie must hold the value round-tripping through github"
    );
    state
}

/// Drive the full browser login dance against the stub GitHub —
/// `GET /auth/github` then the callback exchange — and return the
/// minted `dream_session` cookie value.
async fn perform_login(client: &reqwest::Client, base: &str) -> String {
    let state = start_login(client, base).await;
    let resp = client
        .get(format!("{base}/auth/callback?code=stub-code&state={state}"))
        .header("cookie", format!("dream_oauth_state={state}"))
        .send()
        .await
        .expect("callback request");
    assert_eq!(
        resp.status(),
        StatusCode::FOUND,
        "a successful login callback must 302"
    );
    assert_eq!(
        resp.headers()
            .get("location")
            .and_then(|value| value.to_str().ok()),
        Some("/"),
        "a successful login redirects to the dashboard root"
    );
    let session = set_cookies_named(&resp, "dream_session")
        .into_iter()
        .next()
        .expect("a successful login must Set-Cookie dream_session");
    let attrs = session.to_ascii_lowercase();
    assert!(
        attrs.contains("httponly"),
        "the session cookie must be HttpOnly — no script ever reads it, got {session:?}"
    );
    assert!(
        attrs.contains("secure"),
        "the session cookie must carry the Secure attribute, got {session:?}"
    );
    let value = cookie_value(&session).to_string();
    assert!(
        !value.is_empty(),
        "the session cookie value must be non-empty"
    );
    value
}

#[tokio::test]
async fn github_login_start_redirects_to_authorize_and_sets_the_state_cookie() {
    let stub = spawn_github_stub("alice", vec![]).await;
    let data = TempDir::new().expect("tempdir");
    let base = spawn_oauth_server(data.path(), stub_oauth(&stub, &["alice"])).await;
    let client = no_redirect_client();

    let resp = client
        .get(format!("{base}/auth/github"))
        .send()
        .await
        .expect("login start request");
    assert_eq!(resp.status(), StatusCode::FOUND);
    let location = resp
        .headers()
        .get("location")
        .and_then(|value| value.to_str().ok())
        .expect("the 302 must carry a Location header")
        .to_string();
    assert!(
        location.starts_with(&format!("{stub}/login/oauth/authorize")),
        "the redirect must target the configured authorize endpoint, got {location:?}"
    );
    assert_eq!(
        query_param(&location, "client_id").as_deref(),
        Some("client-id-123"),
        "github identifies the app by client_id"
    );
    assert_eq!(
        query_param(&location, "redirect_uri").as_deref(),
        Some(format!("{base}/auth/callback").as_str()),
        "the code must come back to this daemon's callback route"
    );
    assert!(
        query_param(&location, "state").is_some(),
        "the authorize url must carry the CSRF state"
    );

    let state_cookie = set_cookies_named(&resp, "dream_oauth_state")
        .into_iter()
        .next()
        .expect("login start must Set-Cookie the oauth state");
    let attrs = state_cookie.to_ascii_lowercase();
    assert!(
        attrs.contains("httponly"),
        "the state cookie must be HttpOnly, got {state_cookie:?}"
    );
    assert!(
        attrs.contains("samesite=lax"),
        "the state cookie must be SameSite=Lax, got {state_cookie:?}"
    );
    assert!(
        attrs.contains("max-age=600"),
        "the state cookie caps the login window at 600s, got {state_cookie:?}"
    );
}

#[tokio::test]
async fn github_callback_exchanges_the_code_and_mints_a_working_session_cookie() {
    let stub = spawn_github_stub("alice", vec![]).await;
    let data = TempDir::new().expect("tempdir");
    let base = spawn_oauth_server(data.path(), stub_oauth(&stub, &["alice"])).await;
    let client = no_redirect_client();

    let cookie = perform_login(&client, &base).await;

    // The minted cookie is not just set — it must authenticate.
    let resp = client
        .get(format!("{base}/api/sessions"))
        .header("cookie", format!("dream_session={cookie}"))
        .send()
        .await
        .expect("api request with session cookie");
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "the minted session cookie must authenticate /api requests"
    );
}

#[tokio::test]
async fn an_org_allowlist_entry_admits_any_member_of_that_org() {
    // "bob" is not a listed login, but the stub reports membership in
    // dream-team — the `org:` entry must admit him.
    let stub = spawn_github_stub("bob", vec!["dream-team"]).await;
    let data = TempDir::new().expect("tempdir");
    let base =
        spawn_oauth_server(data.path(), stub_oauth(&stub, &["alice", "org:dream-team"])).await;
    let client = no_redirect_client();

    let cookie = perform_login(&client, &base).await;

    let resp = client
        .get(format!("{base}/api/sessions"))
        .header("cookie", format!("dream_session={cookie}"))
        .send()
        .await
        .expect("api request with session cookie");
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "an org-member session cookie must authenticate /api requests"
    );
}

#[tokio::test]
async fn a_login_matching_no_allowlist_entry_is_denied_without_a_cookie() {
    let stub = spawn_github_stub("mallory", vec!["other-org"]).await;
    let data = TempDir::new().expect("tempdir");
    let base =
        spawn_oauth_server(data.path(), stub_oauth(&stub, &["alice", "org:dream-team"])).await;
    let client = no_redirect_client();

    // A real browser completes the state handshake; only the allowlist
    // check may deny "mallory".
    let state = start_login(&client, &base).await;
    let resp = client
        .get(format!("{base}/auth/callback?code=stub-code&state={state}"))
        .header("cookie", format!("dream_oauth_state={state}"))
        .send()
        .await
        .expect("callback request");

    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a login matching no allowlist entry must be denied"
    );
    assert!(
        set_cookies_named(&resp, "dream_session").is_empty(),
        "a denied login must not receive a session cookie"
    );

    let resp = client
        .get(format!("{base}/api/sessions"))
        .send()
        .await
        .expect("api request");
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "the denied login authenticates nothing"
    );
}

#[tokio::test]
async fn a_callback_with_a_mismatched_state_is_rejected() {
    let stub = spawn_github_stub("alice", vec![]).await;
    let data = TempDir::new().expect("tempdir");
    let base = spawn_oauth_server(data.path(), stub_oauth(&stub, &["alice"])).await;
    let client = no_redirect_client();

    let state = start_login(&client, &base).await;
    let resp = client
        .get(format!(
            "{base}/auth/callback?code=stub-code&state=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        ))
        .header("cookie", format!("dream_oauth_state={state}"))
        .send()
        .await
        .expect("callback request");

    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "a query state that does not match the cookie must be rejected"
    );
    assert!(
        set_cookies_named(&resp, "dream_session").is_empty(),
        "a CSRF rejection must not issue a session cookie"
    );
    let cleared = set_cookies_named(&resp, "dream_oauth_state");
    assert!(
        cleared.iter().any(|header| clears_cookie(header)),
        "the consumed state cookie must be cleared, got {cleared:?}"
    );
}

#[tokio::test]
async fn a_callback_with_missing_state_material_is_rejected() {
    let stub = spawn_github_stub("alice", vec![]).await;
    let data = TempDir::new().expect("tempdir");
    let base = spawn_oauth_server(data.path(), stub_oauth(&stub, &["alice"])).await;
    let client = no_redirect_client();

    // Query state present but no state cookie — nothing to compare
    // against, so there is nothing to trust.
    let resp = client
        .get(format!(
            "{base}/auth/callback?code=stub-code&state=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        ))
        .send()
        .await
        .expect("callback request");
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "a callback without the state cookie must be rejected"
    );
    assert!(
        set_cookies_named(&resp, "dream_session").is_empty(),
        "a CSRF rejection must not issue a session cookie"
    );

    // State cookie present but no query state — symmetric rejection.
    let state = start_login(&client, &base).await;
    let resp = client
        .get(format!("{base}/auth/callback?code=stub-code"))
        .header("cookie", format!("dream_oauth_state={state}"))
        .send()
        .await
        .expect("callback request");
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "a callback without a query state must be rejected"
    );
    assert!(
        set_cookies_named(&resp, "dream_session").is_empty(),
        "a CSRF rejection must not issue a session cookie"
    );
    let cleared = set_cookies_named(&resp, "dream_oauth_state");
    assert!(
        cleared.iter().any(|header| clears_cookie(header)),
        "the consumed state cookie must be cleared, got {cleared:?}"
    );
}

#[tokio::test]
async fn a_minted_session_cookie_authenticates_api_and_websocket_requests() {
    let stub = spawn_github_stub("alice", vec![]).await;
    let data = TempDir::new().expect("tempdir");
    let session = seed_session(data.path(), "cookie-ws");
    let base = spawn_oauth_server(data.path(), stub_oauth(&stub, &["alice"])).await;
    let client = no_redirect_client();

    let cookie = perform_login(&client, &base).await;

    let resp = client
        .get(format!("{base}/api/sessions"))
        .header("cookie", format!("dream_session={cookie}"))
        .send()
        .await
        .expect("api request");
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "the session cookie must authenticate /api requests"
    );

    // A well-formed but wrongly-signed cookie is not "a cookie is
    // present" auth — verify must actually run.
    let resp = client
        .get(format!("{base}/api/sessions"))
        .header(
            "cookie",
            "dream_session=mallory|9999999999|0000000000000000000000000000000000000000000000000000000000000000",
        )
        .send()
        .await
        .expect("api request");
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "a forged session cookie must be rejected"
    );

    // The logged-in browser attaches without a token: the same-origin
    // WS upgrade carries the cookie automatically.
    let resp = ws_upgrade_request(&client, &format!("{base}/api/sessions/{}/ws", session.id))
        .header("cookie", format!("dream_session={cookie}"))
        .send()
        .await
        .expect("ws upgrade request");
    assert_eq!(
        resp.status(),
        StatusCode::SWITCHING_PROTOCOLS,
        "the session cookie must authenticate the WS attach"
    );
}

#[tokio::test]
async fn bearer_and_query_token_auth_still_gate_api_with_oauth_configured() {
    let stub = spawn_github_stub("alice", vec![]).await;
    let data = TempDir::new().expect("tempdir");
    let session = seed_session(data.path(), "ws-token");
    let base = spawn_oauth_server(data.path(), stub_oauth(&stub, &["alice"])).await;
    let client = no_redirect_client();

    // Bearer is the fallback path — it keeps working next to cookies.
    let resp = client
        .get(format!("{base}/api/sessions"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .expect("api request");
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "the bearer token must still authenticate /api requests"
    );

    let resp = client
        .get(format!("{base}/api/sessions"))
        .send()
        .await
        .expect("api request");
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "no credential must still mean 401"
    );

    // The `?token=` channel on the WS attach path is unchanged — the
    // token-paste fallback keeps working for browsers without a session.
    let url = format!(
        "{}/api/sessions/{}/ws?token={TOKEN}",
        base.replacen("http://", "ws://", 1),
        session.id
    );
    match tokio_tungstenite::connect_async(&url).await {
        Ok((_socket, resp)) => assert_eq!(resp.status(), StatusCode::SWITCHING_PROTOCOLS),
        Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => assert_ne!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "a valid ?token= must pass the auth middleware"
        ),
        Err(err) => panic!("unexpected WS connect outcome for {url}: {err}"),
    }
}

#[tokio::test]
async fn logout_expires_the_session_cookie_without_requiring_auth() {
    let stub = spawn_github_stub("alice", vec![]).await;
    let data = TempDir::new().expect("tempdir");
    let base = spawn_oauth_server(data.path(), stub_oauth(&stub, &["alice"])).await;
    let client = no_redirect_client();

    let cookie = perform_login(&client, &base).await;

    // Logout is a POST outside /api — it only clears a cookie, so the
    // middleware exempts it; a stale/absent credential must not gate it.
    for extra_cookie in [None, Some(format!("dream_session={cookie}"))] {
        let request = client.post(format!("{base}/auth/logout"));
        let request = match &extra_cookie {
            Some(cookie) => request.header("cookie", cookie),
            None => request,
        };
        let resp = request.send().await.expect("logout request");
        assert!(
            resp.status().is_success() || resp.status().is_redirection(),
            "POST /auth/logout must be reachable unauthenticated, got {}",
            resp.status()
        );
        let cleared = set_cookies_named(&resp, "dream_session");
        assert!(
            cleared.iter().any(|header| clears_cookie(header)),
            "logout must expire the session cookie, got {cleared:?}"
        );
    }

    // Once the browser drops the expired cookie the API is gated again.
    let resp = client
        .get(format!("{base}/api/sessions"))
        .send()
        .await
        .expect("api request");
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "a logged-out client has no credential left to send"
    );
}
