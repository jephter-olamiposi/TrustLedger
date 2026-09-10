//! Backpressure load-shedding test suite for TrustLedger ingest.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use ingest::proto::ledger_service_client::LedgerServiceClient;
use ingest::proto::ledger_service_server::LedgerServiceServer;
use ingest::proto::{AccountFlags, AccountType, CreateAccountRequest, GetAccountRequest};
use ingest::{Engine, EngineConfig, LedgerServer};
use ledger_core::amount::Scale;
use ledger_core::Ledger;
use tokio_stream::wrappers::TcpListenerStream;
use wal::{Wal, WalOptions};

#[tokio::test]
async fn backpressure_burst_sheds_load_with_resource_exhausted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let wal_path = dir.path().join("wal.log");
    let (wal, _) = Wal::open(&wal_path, WalOptions::default()).expect("open wal");
    let ledger = Ledger::new(Scale::usdc());
    let queue_capacity = 5;
    let config = EngineConfig {
        max_batch_size: 16,
        max_batch_delay: Duration::from_millis(50),
    };
    let (mut engine, queue) = Engine::new(ledger, wal, config, queue_capacity);
    tokio::spawn(async move {
        let _ = engine.run().await;
    });

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let addr = listener.local_addr().expect("local addr");
    let server = LedgerServer::new(queue, Scale::usdc());

    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(LedgerServiceServer::new(server))
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
            .expect("serve");
    });

    let server_url = format!("http://{addr}");

    {
        let mut client = LedgerServiceClient::connect(server_url.clone())
            .await
            .expect("connect client");
        client
            .create_account(CreateAccountRequest {
                id: 1,
                account_type: AccountType::Asset as i32,
                flags: Some(AccountFlags {
                    debits_must_not_exceed_credits: false,
                    credits_must_not_exceed_debits: false,
                    is_closed: false,
                }),
                scale: 6,
                timestamp: 1,
            })
            .await
            .expect("seed account created");
    }

    let burst_size = 100;
    let resource_exhausted_count = Arc::new(AtomicUsize::new(0));
    let success_count = Arc::new(AtomicUsize::new(0));

    let mut handles = Vec::with_capacity(burst_size);
    for _ in 0..burst_size {
        let url = server_url.clone();
        let exhausted = Arc::clone(&resource_exhausted_count);
        let success = Arc::clone(&success_count);

        handles.push(tokio::spawn(async move {
            if let Ok(mut client) = LedgerServiceClient::connect(url).await {
                match client.get_account(GetAccountRequest { id: 1 }).await {
                    Ok(_) => {
                        success.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(status) if status.code() == tonic::Code::ResourceExhausted => {
                        exhausted.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(_) => {}
                }
            }
        }));
    }

    for handle in handles {
        let _ = handle.await;
    }

    let exhausted_total = resource_exhausted_count.load(Ordering::Relaxed);
    let success_total = success_count.load(Ordering::Relaxed);

    assert!(
        exhausted_total > 0,
        "expected backpressure load-shedding to trigger, but got 0 ResourceExhausted (success: {success_total})"
    );

    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut post_burst_client = LedgerServiceClient::connect(server_url)
        .await
        .expect("connect after burst");
    let resp = post_burst_client
        .get_account(GetAccountRequest { id: 1 })
        .await
        .expect("server must remain operational after burst");
    assert_eq!(resp.into_inner().account.unwrap().id, 1);
}
