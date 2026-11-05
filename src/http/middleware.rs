//! Bearer-token gate on `/api/*` and the WS upgrade; the static shell is public.

use axum::{
    extract::{Request, State},
    http::{header, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::http::routes::AppState;

/// The static asset shell is not secret: browsers cannot attach `Bearer`
/// or `?token=` to the `<script src>` / `<link>` subresource fetches that
/// boot the SPA (BUG-E2E-001), so GETs outside `/api/` pass unauthenticated
/// — a `?token=` share link still works, it is simply unnecessary.
/// Everything else — all of `/api/*` plus non-GET requests — requires
/// `Authorization: Bearer <token>`, or `?token=` on the WS upgrade whose
/// browser client cannot set headers.
pub async fn require_token(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    if is_static_get(&request) || is_authorized(&request, &state.token) {
        next.run(request).await
    } else {
        StatusCode::UNAUTHORIZED.into_response()
    }
}

/// GETs outside `/api/` resolve to the public static shell.
fn is_static_get(request: &Request) -> bool {
    request.method() == Method::GET && !request.uri().path().starts_with("/api/")
}

fn is_authorized(request: &Request, token: &str) -> bool {
    bearer_token(request) == Some(token)
        || (is_ws_upgrade(request) && query_token(request) == Some(token))
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
