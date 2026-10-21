//! Notebook sync server (stub).

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().init();
    tracing::info!("notebook-server {}", notebook_core::version());
}
