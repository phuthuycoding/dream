//! Integration tests for the HTTP API + token auth (TC-011, TC-014).
//!
//! A real axum server is spawned per test on a loopback ephemeral port and
//! driven with reqwest / tokio-tungstenite — `tower` is not a direct
//! dependency of this crate, so `tower::ServiceExt::oneshot` is unavailable
//! without a Cargo.toml change that belongs to the dev seat.
//!
//! Assumed surface (the dev pass reconciles — task-006-tests-report.md):
//!   - `webterm::http::routes::build_router(data_dir: PathBuf, token: String)
//!      -> axum::Router` builds the full app (REST + static + WS + middleware)
//!   - WS attach lives at `GET /api/sessions/{id}/ws`, authed via `?token=`
//!   - GET /api/sessions returns a bare JSON array of serialized `Session`s
//!   - mutation endpoints return any 2xx on success; GET of a deleted
//!     session returns 404

#![cfg(test)]

use std::fs;
use std::path::Path;

use reqwest::StatusCode;
use serde_json::{json, Value};
use tempfile::TempDir;

use crate::model::{Session, Status};
use crate::state::registry::Registry;

const TOKEN: &str = "test-token-9f8e7d6c5b4a3210";

/// ASSUMED constructor — one place to fix when the real signature lands.
fn build_app(data_dir: &Path) -> axum::Router {
    crate::http::routes::build_router(data_dir.to_path_buf(), TOKEN.to_string())
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
