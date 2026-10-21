//! Static hosting of the browser client bundle.

use std::path::Path;

use axum::Router;
use axum::extract::Request;
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::Response;
use tower_http::services::ServeDir;

const IMMUTABLE: &str = "public, max-age=31536000, immutable";
const NO_CACHE: &str = "no-cache";

/// A fallback router serving `dir`, or answering `404` when there is none.
pub fn router(dir: Option<&Path>) -> Router {
    match dir {
        Some(dir) => Router::new()
            .fallback_service(ServeDir::new(dir).append_index_html_on_directories(true))
            .layer(middleware::from_fn(cache_control)),
        None => Router::new().fallback(|| async { StatusCode::NOT_FOUND }),
    }
}

/// Hashed files under `/assets/` are cached forever; everything else,
/// including `index.html` and error responses, revalidates.
async fn cache_control(request: Request, next: Next) -> Response {
    let hashed = request.uri().path().starts_with("/assets/");
    let mut response = next.run(request).await;
    let cacheable = response.status().is_success() || response.status() == StatusCode::NOT_MODIFIED;
    let value = if hashed && cacheable {
        IMMUTABLE
    } else {
        NO_CACHE
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static(value));
    response
}
