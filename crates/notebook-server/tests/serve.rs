//! The serve loop over a real socket: graceful shutdown with a request in
//! flight.

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::routing::get;
use notebook_server::app::{self, AppState};
use notebook_server::config::Config;
use notebook_server::{auth, db};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Notify, oneshot};

#[tokio::test]
async fn in_flight_request_completes_after_shutdown_signal() {
    let config = Config::from_sources(None, |k| {
        (k == "NOTEBOOK_PUBLIC_URL").then(|| "http://localhost".to_owned())
    })
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let pool = db::connect(&dir.path().join("db")).await.unwrap();
    db::migrate(&pool).await.unwrap();
    let state = AppState::new(pool, config, auth::keyfile::generate());

    let started = Arc::new(Notify::new());
    let notify = started.clone();
    let routes = Router::new().route(
        "/slow",
        get(move || {
            let notify = notify.clone();
            async move {
                notify.notify_one();
                tokio::time::sleep(Duration::from_millis(300)).await;
                "finished"
            }
        }),
    );
    let router = app::harden(routes, &state);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let server = tokio::spawn(app::run(listener, router, async {
        let _ = shutdown_rx.await;
    }));

    let client = tokio::spawn(async move {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream
            .write_all(b"GET /slow HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();
        response
    });

    started.notified().await;
    shutdown_tx.send(()).unwrap();

    let response = client.await.unwrap();
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.ends_with("finished"), "{response}");

    let result = tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("server did not stop after its last request")
        .unwrap();
    assert!(result.is_ok(), "{result:?}");

    // New connections are refused once the server has stopped.
    assert!(TcpStream::connect(addr).await.is_err());
}
