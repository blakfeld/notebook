//! Router-level tests: the full middleware stack driven in-process.

use std::net::SocketAddr;
use std::num::{NonZeroU32, NonZeroU64};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Request, Response, StatusCode, header};
use axum::routing::{get, post};
use http_body_util::BodyExt;
use notebook_server::app::{self, AppState};
use notebook_server::config::Config;
use notebook_server::{auth, db, security};
use tempfile::TempDir;
use tower::ServiceExt;

fn config(public_url: &str) -> Config {
    let url = public_url.to_owned();
    Config::from_sources(None, move |k| {
        (k == "NOTEBOOK_PUBLIC_URL").then(|| url.clone())
    })
    .unwrap()
}

async fn state(config: Config) -> (AppState, TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let pool = db::connect(&dir.path().join("db")).await.unwrap();
    db::migrate(&pool).await.unwrap();
    (AppState::new(pool, config, auth::keyfile::generate()), dir)
}

async fn send_from(router: &Router, peer: &str, request: Request<Body>) -> Response<Body> {
    let peer: SocketAddr = peer.parse().unwrap();
    router
        .clone()
        .layer(MockConnectInfo(peer))
        .oneshot(request)
        .await
        .unwrap()
}

async fn send(router: &Router, request: Request<Body>) -> Response<Body> {
    send_from(router, "192.0.2.1:5000", request).await
}

fn get_req(path: &str) -> Request<Body> {
    Request::get(path).body(Body::empty()).unwrap()
}

async fn body_bytes(response: Response<Body>) -> Bytes {
    response.into_body().collect().await.unwrap().to_bytes()
}

// --- health ---

#[tokio::test]
async fn healthz_ok() {
    let (state, _dir) = state(config("http://localhost")).await;
    let response = send(&app::router(state), get_req("/healthz")).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn healthz_unavailable_when_pool_closed() {
    let (state, _dir) = state(config("http://localhost")).await;
    state.pool.close().await;
    let response = send(&app::router(state), get_req("/healthz")).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(body_bytes(response).await.is_empty());
}

// --- static files ---

struct Site {
    _root: TempDir,
    router: Router,
    _db: TempDir,
}

async fn site() -> Site {
    let root = tempfile::tempdir().unwrap();
    let static_dir = root.path().join("static");
    std::fs::create_dir_all(static_dir.join("assets")).unwrap();
    std::fs::write(static_dir.join("index.html"), "<!doctype html>hello").unwrap();
    std::fs::write(static_dir.join("assets/app-3f9a2c.js"), "console.log(1)").unwrap();
    std::fs::write(root.path().join("secret.txt"), "TOP-SECRET").unwrap();

    let mut config = config("http://localhost");
    config.static_dir = Some(static_dir);
    let (state, db) = state(config).await;
    Site {
        _root: root,
        router: app::router(state),
        _db: db,
    }
}

#[tokio::test]
async fn index_served_as_html_without_caching() {
    let site = site().await;
    for path in ["/", "/index.html"] {
        let response = send(&site.router, get_req(path)).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        let content_type = response.headers()[header::CONTENT_TYPE].to_str().unwrap();
        assert!(content_type.starts_with("text/html"), "{content_type}");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-cache");
        assert_eq!(&body_bytes(response).await[..], b"<!doctype html>hello");
    }
}

#[tokio::test]
async fn hashed_assets_are_immutable() {
    let site = site().await;
    let response = send(&site.router, get_req("/assets/app-3f9a2c.js")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CACHE_CONTROL],
        "public, max-age=31536000, immutable"
    );

    // A missing asset must not be cached forever.
    let response = send(&site.router, get_req("/assets/missing.js")).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-cache");
}

#[tokio::test]
async fn missing_file_is_404() {
    let site = site().await;
    let response = send(&site.router, get_req("/nope.txt")).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn path_traversal_never_escapes() {
    let site = site().await;
    for path in [
        "/../secret.txt",
        "/assets/../../secret.txt",
        "/%2e%2e/secret.txt",
        "/%2E%2E%2Fsecret.txt",
        "/assets/..%2f..%2fsecret.txt",
        "/..%5csecret.txt",
    ] {
        let response = send(&site.router, get_req(path)).await;
        let status = response.status();
        let body = body_bytes(response).await;
        assert_ne!(status, StatusCode::OK, "{path}");
        assert!(
            !String::from_utf8_lossy(&body).contains("TOP-SECRET"),
            "{path}"
        );
    }
}

#[tokio::test]
async fn no_static_dir_is_404_and_health_still_works() {
    let (state, _dir) = state(config("http://localhost")).await;
    let router = app::router(state);
    assert_eq!(
        send(&router, get_req("/")).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(&router, get_req("/healthz")).await.status(),
        StatusCode::OK
    );
}

// --- body limit and timeout ---

fn small_limits(mut config: Config) -> Config {
    config.max_body_bytes = 1024.try_into().unwrap();
    config.request_timeout_secs = NonZeroU64::new(1).unwrap();
    config
}

/// Test routes behind the real hardening stack.
fn test_routes(state: &AppState) -> Router {
    let routes = Router::new()
        .route(
            "/upload",
            post(|body: Bytes| async move { body.len().to_string() }),
        )
        .route(
            "/slow",
            get(|| async {
                tokio::time::sleep(Duration::from_secs(60)).await;
                "done"
            }),
        );
    app::harden(routes, state)
}

#[tokio::test]
async fn declared_oversize_body_is_413() {
    let (state, _dir) = state(small_limits(config("http://localhost"))).await;
    let router = test_routes(&state);
    // The declared length alone is enough: the body is never read.
    let request = Request::post("/upload")
        .header(header::CONTENT_LENGTH, "1000000")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        send(&router, request).await.status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
}

#[tokio::test]
async fn streamed_oversize_body_is_413() {
    let (state, _dir) = state(small_limits(config("http://localhost"))).await;
    let router = test_routes(&state);

    let request = Request::post("/upload")
        .body(Body::from(vec![b'x'; 512]))
        .unwrap();
    let response = send(&router, request).await;
    assert_eq!(response.status(), StatusCode::OK);

    // No Content-Length header: the limit is discovered while reading.
    let request = Request::post("/upload")
        .body(Body::from(vec![b'x'; 4096]))
        .unwrap();
    assert_eq!(
        send(&router, request).await.status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
}

#[tokio::test]
async fn slow_handler_times_out_with_408() {
    let (state, _dir) = state(small_limits(config("http://localhost"))).await;
    let router = test_routes(&state);
    tokio::time::pause();
    let response = send(&router, get_req("/slow")).await;
    assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
}

// --- rate limiting ---

fn tight_rate(mut config: Config) -> Config {
    config.rate_limit_per_second = NonZeroU32::new(1).unwrap();
    config.rate_limit_burst = NonZeroU32::new(2).unwrap();
    config
}

fn counting_routes(state: &AppState) -> (Router, Arc<AtomicUsize>) {
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    let routes = Router::new().route(
        "/count",
        get(move || {
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                "ok"
            }
        }),
    );
    (app::harden(routes, state), hits)
}

#[tokio::test]
async fn burst_exhaustion_gets_429_without_reaching_handler() {
    let (state, _dir) = state(tight_rate(config("http://localhost"))).await;
    let (router, hits) = counting_routes(&state);

    for _ in 0..2 {
        assert_eq!(
            send(&router, get_req("/count")).await.status(),
            StatusCode::OK
        );
    }
    let response = send(&router, get_req("/count")).await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let retry: u64 = response.headers()[header::RETRY_AFTER]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!(retry >= 1);
    assert_eq!(hits.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn clients_are_limited_independently() {
    let (state, _dir) = state(tight_rate(config("http://localhost"))).await;
    let (router, _) = counting_routes(&state);
    for _ in 0..3 {
        send_from(&router, "192.0.2.1:1", get_req("/count")).await;
    }
    let limited = send_from(&router, "192.0.2.1:2", get_req("/count")).await;
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    let other = send_from(&router, "192.0.2.2:1", get_req("/count")).await;
    assert_eq!(other.status(), StatusCode::OK);
}

#[tokio::test]
async fn ipv6_clients_share_a_64_limit() {
    let (state, _dir) = state(tight_rate(config("http://localhost"))).await;
    let (router, _) = counting_routes(&state);
    let a = "[2001:db8:1:2::1]:1";
    let b = "[2001:db8:1:2:ffff::9]:1";
    assert_eq!(
        send_from(&router, a, get_req("/count")).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        send_from(&router, b, get_req("/count")).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        send_from(&router, b, get_req("/count")).await.status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    let elsewhere = "[2001:db8:1:3::1]:1";
    assert_eq!(
        send_from(&router, elsewhere, get_req("/count"))
            .await
            .status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn trusted_proxy_forwarded_clients_are_limited_separately() {
    let mut config = tight_rate(config("http://localhost"));
    config.trusted_proxies = vec!["127.0.0.1".parse().unwrap()];
    let (state, _dir) = state(config).await;
    let (router, _) = counting_routes(&state);
    let via_proxy = |client: &str| {
        Request::get("/count")
            .header("x-forwarded-for", client)
            .body(Body::empty())
            .unwrap()
    };
    for _ in 0..2 {
        send_from(&router, "127.0.0.1:1", via_proxy("203.0.113.7")).await;
    }
    let limited = send_from(&router, "127.0.0.1:1", via_proxy("203.0.113.7")).await;
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    let other = send_from(&router, "127.0.0.1:1", via_proxy("203.0.113.8")).await;
    assert_eq!(other.status(), StatusCode::OK);
}

// --- security headers ---

fn assert_security_headers(response: &Response<Body>, https: bool) {
    for (name, value) in security::headers() {
        assert_eq!(
            response.headers().get(&name),
            Some(&value),
            "{name} on {}",
            response.status()
        );
    }
    let hsts = response.headers().get(header::STRICT_TRANSPORT_SECURITY);
    if https {
        assert_eq!(hsts.unwrap(), security::HSTS);
    } else {
        assert!(hsts.is_none(), "unexpected HSTS on {}", response.status());
    }
}

async fn every_kind_of_response(public_url: &str) -> Vec<Response<Body>> {
    let config = tight_rate(small_limits(config(public_url)));
    let (state, _dir) = state(config).await;
    let full = app::router(state.clone());
    let test = test_routes(&state);

    let ok = send_from(&full, "192.0.2.10:1", get_req("/healthz")).await;
    let not_found = send_from(&full, "192.0.2.11:1", get_req("/missing")).await;
    let too_large = send_from(
        &test,
        "192.0.2.12:1",
        Request::post("/upload")
            .header(header::CONTENT_LENGTH, "999999")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    for _ in 0..2 {
        send_from(&full, "192.0.2.13:1", get_req("/healthz")).await;
    }
    let limited = send_from(&full, "192.0.2.13:1", get_req("/healthz")).await;

    let responses = vec![ok, not_found, too_large, limited];
    let statuses: Vec<u16> = responses.iter().map(|r| r.status().as_u16()).collect();
    assert_eq!(statuses, [200, 404, 413, 429]);
    responses
}

#[tokio::test]
async fn security_headers_on_every_response_with_hsts_for_https() {
    for response in every_kind_of_response("https://notes.example.com").await {
        assert_security_headers(&response, true);
    }
}

#[tokio::test]
async fn no_hsts_for_http() {
    for response in every_kind_of_response("http://localhost:8080").await {
        assert_security_headers(&response, false);
    }
}
