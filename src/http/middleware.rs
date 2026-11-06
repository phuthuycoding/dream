//! Bearer-token / session-cookie gate on `/api/*` and the WS upgrade;
//! the static shell and the auth flow's own endpoints are public.

use axum::{
    extract::{Request, State},
    http::{header, HeaderMap, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::auth::session::SESSION_COOKIE;
use crate::http::routes::AppState;

/// The static asset shell is not secret: browsers cannot attach `Bearer`
/// or `?token=` to the `<script src>` / `<link>` subresource fetches that
/// boot the SPA (BUG-E2E-001), so GETs outside `/api/` pass unauthenticated
/// — a `?token=` share link still works, it is simply unnecessary.
/// `POST /auth/logout` is exempt too: it only expires a cookie, so a
/// stale or absent credential must not gate it.
/// Everything else — all of `/api/*` plus non-GET requests — requires a
/// signed `dream_session` cookie, `Authorization: Bearer <token>`, or
/// `?token=` on the WS attach route whose browser client cannot set
/// headers.
pub async fn require_token(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    if is_static_get(&request)
        || is_logout(&request)
        || has_session(&request, &state)
        || is_authorized(&request, &state.token)
    {
        next.run(request).await
    } else {
        StatusCode::UNAUTHORIZED.into_response()
    }
}

/// GETs outside `/api/` resolve to the public static shell.
fn is_static_get(request: &Request) -> bool {
    request.method() == Method::GET && !request.uri().path().starts_with("/api/")
}

/// `POST /auth/logout` only expires `dream_session` — no CSRF surface.
fn is_logout(request: &Request) -> bool {
    request.method() == Method::POST && request.uri().path() == "/auth/logout"
}

/// A signed `dream_session` cookie satisfies the gate the same way the
/// bearer does — a logged-in browser sends it to `/api/*` and the WS
/// upgrade automatically. Absent oauth there is no signer and nothing
/// to verify against.
fn has_session(request: &Request, state: &AppState) -> bool {
    let Some(signer) = &state.sessions else {
        return false;
    };
    let Some(value) = cookie_value(request.headers(), SESSION_COOKIE) else {
        return false;
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    signer.verify(&value, now).is_some()
}

/// One `name=value` pair out of a request's Cookie header.
pub fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    let prefix = format!("{name}=");
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .map(str::trim)
        .find_map(|pair| pair.strip_prefix(&prefix).map(str::to_string))
}

fn is_authorized(request: &Request, token: &str) -> bool {
    bearer_token(request) == Some(token)
        || (is_ws_upgrade(request)
            && is_ws_attach_path(request.uri().path())
            && query_token(request) == Some(token))
}

/// The only path where `?token=` is honoured: `GET /api/sessions/{id}/ws`.
/// Without the path check, any `/api/*` GET carrying a forged
/// `Upgrade: websocket` header would authenticate on the query token
/// alone — the upgrade header is just a string any client can send.
fn is_ws_attach_path(path: &str) -> bool {
    path.strip_prefix("/api/sessions/")
        .and_then(|rest| rest.strip_suffix("/ws"))
        .is_some_and(|id| !id.is_empty() && !id.contains('/'))
}

/// True when the request asks for a websocket upgrade.
fn is_ws_upgrade(request: &Request) -> bool {
    request
        .headers()
        .get(header::UPGRADE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("websocket"))
}

fn bearer_token(request: &Request) -> Option<&str> {
    request
        .headers()
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

fn query_token(request: &Request) -> Option<&str> {
    request
        .uri()
        .query()?
        .split('&')
        .find_map(|pair| pair.strip_prefix("token="))
}
