//! HTTP server for the TrustLedger demo app.

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::net::TcpListener;

use crate::api::{router, AppState};
use crate::error::DemoError;

/// Start the HTTP API and dashboard server on the given address.
///
/// # Errors
///
/// Returns [`std::io::Error`] if binding or serving fails.
pub async fn start_server(state: Arc<AppState>, addr: SocketAddr) -> Result<(), std::io::Error> {
    let app = router(state);
    let listener = TcpListener::bind(addr).await?;
    println!("TrustLedger Demo Server listening on http://{addr}");
    axum::serve(listener, app).await
}

/// Convenience function to create an initialized app state and start the server.
///
/// # Errors
///
/// Returns [`DemoError`] if state initialization or network binding fails.
pub async fn run_default_server(port: u16) -> Result<(), DemoError> {
    let secret = b"super-secret-webhook-key-2026".to_vec();
    let state = Arc::new(AppState::new(secret)?);
    let host = std::env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let ip = host
        .parse::<std::net::IpAddr>()
        .map_err(|e| DemoError::Server(format!("invalid HOST address '{host}': {e}")))?;
    let addr = SocketAddr::from((ip, port));

    start_server(state, addr)
        .await
        .map_err(|e| DemoError::Server(e.to_string()))
}
