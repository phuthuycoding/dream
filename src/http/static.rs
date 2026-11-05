//! Static dashboard assets embedded into the binary via rust-embed.

use axum::{
    body::Body,
    http::{header, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "web/"]
struct Assets;

/// Serve an embedded asset; `/` resolves to `index.html`.
pub async fn serve(uri: Uri) -> Response {
    let path = match uri.path().trim_start_matches('/') {
        "" => "index.html",
        path => path,
    };
    match Assets::get(path) {
        Some(file) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, mime.as_ref())
                .body(Body::from(file.data.into_owned()))
                .expect("static response is always constructible")
        }
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}
