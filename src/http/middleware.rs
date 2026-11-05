//! Bearer-token gate applied to every route, including the WS upgrade.

use axum::{
    extract::{Request, State},
    http::{header, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::http::routes::AppState;

/// Reject with 401 unless the request carries the token either as
/// `Authorization: Bearer <token>` or as a `?token=` query parameter on the
/// two route classes that cannot send headers: WS-upgrade requests (the
/// browser WS client) and GETs to non-`/api/` routes (the share-link that
/// bootstraps index.html, FR-007). `/api/*` stays Bearer-only so the secret
/// never lands in REST URLs.
pub async fn require_token(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    if is_authorized(&request, &state.token) {
        next.run(request).await
    } else {
        StatusCode::UNAUTHORIZED.into_response()
    }
}

fn is_authorized(request: &Request, token: &str) -> bool {
    bearer_token(request) == Some(token)
        || (accepts_query_token(request) && query_token(request) == Some(token))
}

/// `?token=` is honoured only on WS upgrades and on GETs outside `/api/`.
fn accepts_query_token(request: &Request) -> bool {
    is_ws_upgrade(request)
        || (request.method() == Method::GET && !request.uri().path().starts_with("/api/"))
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
