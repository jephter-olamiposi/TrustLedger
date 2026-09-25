//! Binary entrypoint to run the interactive TrustLedger demo application.

use std::env;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let port = env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);

    let host = env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    println!("Starting TrustLedger demo server on http://{host}:{port}");
    println!("Interactive Dashboard: http://localhost:{port}/");
    println!("Press Ctrl+C to terminate.");

    demo::run_default_server(port).await?;
    Ok(())
}
