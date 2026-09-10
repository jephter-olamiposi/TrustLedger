//! Binary entrypoint to run the interactive TrustLedger demo application.

use std::env;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let port = env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);

    println!("============================================================");
    println!("     TrustLedger Reference Client & Demonstration Server    ");
    println!("============================================================");
    println!("Starting demo server on http://127.0.0.1:{port}");
    println!("Interactive Dashboard: http://127.0.0.1:{port}/");
    println!("Press Ctrl+C to terminate.");

    demo::run_default_server(port).await?;
    Ok(())
}
