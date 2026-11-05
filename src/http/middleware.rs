//! Bearer-token gate applied to every route, including the WS upgrade.

use axum::{
    extract::{Request, State},
    http::{header, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::http::routes::AppState;

/// Reject with 401 unless the request carries the token either as
/// `Authorization: Bearer <token>` or as a `?token=` query parameter — the
/// channel the browser WS client uses, since it cannot set headers.
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
    bearer_token(request) == Some(token) || query_token(request) == Some(token)
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
