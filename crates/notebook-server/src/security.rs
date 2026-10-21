//! Security response headers, applied to every response.

use axum::Router;
use axum::http::{HeaderName, HeaderValue, header};
use tower_http::set_header::SetResponseHeaderLayer;

/// Same-origin scripts, styles, and connections; no inline script or `eval`
/// (WebAssembly compilation excepted); no plugins, framing, or form posts.
pub const CONTENT_SECURITY_POLICY: &str = "default-src 'none'; \
    script-src 'self' 'wasm-unsafe-eval'; style-src 'self'; \
    img-src 'self' data: blob:; font-src 'self'; connect-src 'self'; \
    manifest-src 'self'; base-uri 'none'; form-action 'none'; \
    frame-ancestors 'none'";

pub const HSTS: &str = "max-age=31536000";

/// The headers every response carries, whatever the deployment.
pub fn headers() -> [(HeaderName, HeaderValue); 4] {
    [
        (
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(CONTENT_SECURITY_POLICY),
        ),
        (
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ),
        (header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY")),
        (
            header::REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        ),
    ]
}

/// Wraps `router` so every response gets the security headers, plus HSTS
/// when the server is reached over `https`. Apply as the outermost layer.
pub fn apply(mut router: Router, https: bool) -> Router {
    if https {
        router = router.layer(SetResponseHeaderLayer::overriding(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static(HSTS),
        ));
    }
    for (name, value) in headers() {
        router = router.layer(SetResponseHeaderLayer::overriding(name, value));
    }
    router
}
