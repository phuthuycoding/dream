//! Bearer-token gate applied to every route, including the WS upgrade.

use axum::{
    extract::{Request, State},
    http::{header, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::http::routes::AppState;

/// Reject with 401 unless the request carries the token either as
/// `Authorization: Bearer <token>` or, on WS-upgrade requests only, as a
/// `?token=` query parameter — the channel the browser WS client uses, since
/// it cannot set headers. Scoping keeps the secret out of plain REST URLs.
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
        || (is_ws_upgrade(request) && query_token(request) == Some(token))
}

/// True when the request asks for a websocket upgrade — the only route class
/// where `?token=` is honoured.
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
