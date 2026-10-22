//! Application state, the router and its middleware stack, and the serve loop.

use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::get;
use axum::{Router, middleware};
use sqlx::SqlitePool;
use tokio::net::TcpListener;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;
use tracing::Span;

use crate::auth::{self, AuthState, RegistrationPolicy, Setup};
use crate::config::Config;
use crate::rate_limit::KeyedLimiter;
use crate::{client_ip, db, rate_limit, security, static_files};

/// How long in-flight requests get to finish after a shutdown signal.
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

/// How long closing the pool may take after serving stops.
const POOL_CLOSE_TIMEOUT: Duration = Duration::from_secs(2);

/// How long `/healthz` waits for the database.
const HEALTH_TIMEOUT: Duration = Duration::from_secs(2);

/// Shared state handed to every handler.
#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub config: Arc<Config>,
    /// The global per-client limiter applied to every request.
    pub limiter: Arc<KeyedLimiter>,
    pub auth: Arc<AuthState>,
}

impl AppState {
    /// `setup` is the OPAQUE server key, normally from
    /// [`auth::keyfile::load_or_create`]. HTTP registration is closed.
    pub fn new(pool: SqlitePool, config: Config, setup: Setup) -> Self {
        let limiter = Arc::new(KeyedLimiter::new(
            config.rate_limit_per_second,
            config.rate_limit_burst,
        ));
        AppState {
            pool,
            config: Arc::new(config),
            limiter,
            auth: Arc::new(AuthState::new(setup, RegistrationPolicy::Closed)),
        }
    }
}

/// The full application: every route behind the hardening stack.
pub fn router(state: AppState) -> Router {
    let routes = Router::new()
        .route("/healthz", get(healthz))
        .merge(auth::routes(&state))
        .with_state(state.clone())
        .fallback_service(static_files::router(state.config.static_dir.as_deref()));
    harden(routes, &state)
}

/// Wraps `routes` in the middleware stack. Outermost first: security headers,
/// tracing, client address, global rate limit, request timeout, body limit.
pub fn harden(routes: Router, state: &AppState) -> Router {
    let config = &state.config;
    let max_body = config.max_body_bytes.get();
    let trusted: Arc<[IpAddr]> = config.trusted_proxies.clone().into();

    // `Router::layer` wraps what is already there, so innermost goes first.
    let router = routes
        .layer(DefaultBodyLimit::max(max_body))
        .layer(RequestBodyLimitLayer::new(max_body))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(config.request_timeout_secs.get()),
        ))
        .layer(middleware::from_fn_with_state(
            state.limiter.clone(),
            rate_limit::middleware,
        ))
        .layer(middleware::from_fn_with_state(
            trusted,
            client_ip::middleware,
        ))
        // Logs method, path (never the query), status, and latency. Headers
        // and bodies are never recorded.
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(|request: &Request| {
                    tracing::info_span!(
                        "request",
                        method = %request.method(),
                        path = %request.uri().path(),
                    )
                })
                .on_request(())
                .on_response(|response: &Response, latency: Duration, _span: &Span| {
                    tracing::info!(
                        status = response.status().as_u16(),
                        latency_ms = latency.as_secs_f64() * 1000.0,
                        "response"
                    );
                })
                .on_body_chunk(())
                .on_eos(())
                .on_failure(()),
        );
    security::apply(router, config.is_https())
}

/// `200` when the database answers, `503` with an empty body otherwise.
async fn healthz(State(state): State<AppState>) -> StatusCode {
    let query = sqlx::query("SELECT 1").execute(&state.pool);
    match tokio::time::timeout(HEALTH_TIMEOUT, query).await {
        Ok(Ok(_)) => StatusCode::OK,
        Ok(Err(e)) => {
            tracing::warn!("health check failed: {e}");
            StatusCode::SERVICE_UNAVAILABLE
        }
        Err(_) => {
            tracing::warn!("health check timed out");
            StatusCode::SERVICE_UNAVAILABLE
        }
    }
}

/// The `serve` subcommand: migrate, bind, and serve until signalled.
pub async fn serve(config: Config) -> Result<(), Box<dyn std::error::Error>> {
    let pool = db::connect(&config.database_path).await?;
    db::migrate(&pool).await?;
    let setup = auth::keyfile::load_or_create(&pool, &config.opaque_key_path).await?;

    if config.trusted_proxies.is_empty() && config.bind.ip().is_loopback() {
        tracing::warn!(
            "bound to loopback with no trusted_proxies: if a reverse proxy is in front, \
             every client shares the proxy's address and rate limit"
        );
    }

    let listener = TcpListener::bind(config.bind).await?;
    tracing::info!(
        "notebook-server {} listening on {}",
        notebook_core::version(),
        listener.local_addr()?
    );

    let state = AppState::new(pool.clone(), config, setup);
    let _retain = rate_limit::spawn_retain_task(&state.limiter);
    let _auth_tasks = state.auth.spawn_tasks(&pool);
    let result = run(listener, router(state), shutdown_signal()).await;
    // Connection tasks outlive `run` after the grace period and may still hold
    // pool connections, which `close` waits for; don't let them block exit.
    if tokio::time::timeout(POOL_CLOSE_TIMEOUT, pool.close())
        .await
        .is_err()
    {
        tracing::warn!("database connections still in use; exiting without waiting");
    }
    result?;
    tracing::info!("shut down");
    Ok(())
}

/// Serves `router` on `listener` until `shutdown` resolves, then lets
/// in-flight requests finish for up to [`SHUTDOWN_GRACE`].
pub async fn run(
    listener: TcpListener,
    router: Router,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    let (signalled_tx, signalled_rx) = tokio::sync::oneshot::channel::<()>();
    let server = axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        shutdown.await;
        tracing::info!("shutdown signal received; draining connections");
        let _ = signalled_tx.send(());
    });
    let grace = async move {
        if signalled_rx.await.is_err() {
            std::future::pending::<()>().await;
        }
        tokio::time::sleep(SHUTDOWN_GRACE).await;
    };
    tokio::select! {
        result = server => result,
        () = grace => {
            // Remaining connection tasks are cancelled when the runtime shuts down.
            tracing::warn!("grace period elapsed; abandoning open connections");
            Ok(())
        }
    }
}

/// Resolves on `SIGINT` (Ctrl-C) or, on Unix, `SIGTERM`.
pub async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::error!("cannot listen for SIGINT: {e}");
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(e) => {
                tracing::error!("cannot listen for SIGTERM: {e}");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
}

#[cfg(test)]
mod tests {
    use axum::body::{Body, Bytes};
    use axum::extract::connect_info::MockConnectInfo;
    use axum::http::{Request, header};
    use axum::routing::post;
    use tower::ServiceExt;

    use super::*;
    use crate::config::LogFormat;
    use crate::logging::{self, test_writer::Captured};

    #[tokio::test]
    async fn request_log_omits_query_headers_and_body() {
        let config = Config::from_sources(None, |k| {
            (k == "NOTEBOOK_PUBLIC_URL").then(|| "http://localhost".to_owned())
        })
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let pool = db::connect(&dir.path().join("db")).await.unwrap();
        let state = AppState::new(pool, config, auth::keyfile::generate());
        let routes = Router::new().route(
            "/x",
            post(|body: Bytes| async move { body.len().to_string() }),
        );
        let router =
            harden(routes, &state).layer(MockConnectInfo(SocketAddr::from(([192, 0, 2, 1], 1))));

        for format in [LogFormat::Text, LogFormat::Json] {
            let out = Captured::default();
            let _guard = tracing::subscriber::set_default(logging::subscriber(
                "trace",
                format,
                false,
                out.clone(),
            ));
            let request = Request::post("/x?token=abc")
                .header(header::AUTHORIZATION, "Bearer hunter2-credential")
                .header(header::COOKIE, "session=cookie-secret")
                .body(Body::from("BODY-MARKER-7731"))
                .unwrap();
            let response = router.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);

            let log = out.contents();
            assert!(log.contains("POST"), "{log}");
            assert!(log.contains("/x"), "{log}");
            assert!(log.contains("200"), "{log}");
            assert!(log.contains("latency_ms"), "{log}");
            for secret in ["abc", "token", "hunter2", "cookie-secret", "BODY-MARKER"] {
                assert!(!log.contains(secret), "{secret} leaked: {log}");
            }
        }
    }
}
