//! End-to-end test suite proving payment lifecycle, webhook security, rails, and 3-way reconciliation.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use demo::api::AppState;
use demo::payment::{AccountDirectory, Payment, PaymentState};
use demo::rail::SettlementRail;
use demo::reconciliation::{ReconciliationEngine, ReconciliationStatus};
use demo::webhook::{WebhookPayload, WebhookVerifier};
use ledger_core::amount::Amount;
use ledger_core::id::TransferId;
use ledger_core::transfer::Transfer;

#[test]
fn test_payment_lifecycle_two_phase_holds() {
    let secret = b"test-secret-key-12345".to_vec();
    let state = AppState::new(secret).expect("app state init");

    let merchant_id = 1;
    let customer_id = 101;
    let initial_balance = 1_000_000_000u128; // $1,000.00 seeded

    let payment_id = state.next_payment_id.fetch_add(1, Ordering::SeqCst) as u128;
    let hold_id = state.next_transfer_id.fetch_add(1, Ordering::SeqCst) as u128;
    let now = 1_700_000_000;

    let hold = Transfer::new_pending(
        TransferId::new(hold_id),
        AccountDirectory::customer(customer_id),
        AccountDirectory::merchant(merchant_id),
        Amount::new(50_000),
        now,
    )
    .expect("valid hold");

    {
        let mut ledger = state.ledger.write().unwrap();
        ledger.create_pending(hold).expect("create pending");
    }

    let mut payment = Payment::new_authorized(
        payment_id,
        merchant_id,
        customer_id,
        50_000,
        500,
        hold_id,
        now,
    );

    {
        let ledger = state.ledger.read().unwrap();
        let cust_acc = ledger
            .get_account(AccountDirectory::customer(customer_id))
            .unwrap();
        assert_eq!(cust_acc.balance.credits_posted.as_u128(), initial_balance);
        assert_eq!(cust_acc.balance.debits_pending.as_u128(), 50_000);
    }

    let post_id = state.next_transfer_id.fetch_add(1, Ordering::SeqCst) as u128;
    {
        let mut ledger = state.ledger.write().unwrap();
        ledger
            .post_pending(
                TransferId::new(hold_id),
                TransferId::new(post_id),
                Amount::new(50_000),
                now + 10,
            )
            .expect("post hold");
    }
    payment
        .transition_to_captured(post_id, now + 10)
        .expect("transition to captured");
    assert_eq!(payment.state, PaymentState::Captured);

    {
        let ledger = state.ledger.read().unwrap();
        let merch_acc = ledger
            .get_account(AccountDirectory::merchant(merchant_id))
            .unwrap();
        assert_eq!(merch_acc.balance.credits_posted.as_u128(), 50_000);
    }

    let p2_id = state.next_payment_id.fetch_add(1, Ordering::SeqCst) as u128;
    let hold2_id = state.next_transfer_id.fetch_add(1, Ordering::SeqCst) as u128;

    let hold2 = Transfer::new_pending(
        TransferId::new(hold2_id),
        AccountDirectory::customer(customer_id),
        AccountDirectory::merchant(merchant_id),
        Amount::new(20_000),
        now + 20,
    )
    .expect("valid hold 2");

    {
        let mut ledger = state.ledger.write().unwrap();
        ledger.create_pending(hold2).expect("create pending 2");
        ledger
            .void_pending(TransferId::new(hold2_id), now + 30)
            .expect("void pending 2");
    }

    let mut p2 = Payment::new_authorized(
        p2_id,
        merchant_id,
        customer_id,
        20_000,
        200,
        hold2_id,
        now + 20,
    );
    p2.transition_to_voided(now + 30).expect("void payment");
    assert_eq!(p2.state, PaymentState::Voided);

    {
        let ledger = state.ledger.read().unwrap();
        ledger.verify_invariants().expect("invariants must hold");
    }
}

#[test]
fn test_webhook_hmac_tamper_and_replay_protection() {
    let secret = b"fintech-security-secret-42";
    let now = 1_700_000_000;

    let payload = WebhookPayload {
        event_id: "evt_card_charge_001".to_string(),
        event_type: "charge.authorized".to_string(),
        payment_id: 8888,
        merchant_id: 2,
        customer_id: 102,
        amount: 15_000,
        fee_amount: 150,
        timestamp: now,
    };

    let raw_bytes = serde_json::to_vec(&payload).expect("serialize payload");
    let valid_signature =
        WebhookVerifier::compute_signature(secret, &raw_bytes).expect("compute signature");

    assert!(WebhookVerifier::verify_signature(secret, &raw_bytes, &valid_signature).is_ok());

    let mut tampered_bytes = raw_bytes.clone();
    tampered_bytes[10] ^= 0x01;
    assert!(WebhookVerifier::verify_signature(secret, &tampered_bytes, &valid_signature).is_err());

    let bad_sig = "a".repeat(64);
    assert!(WebhookVerifier::verify_signature(secret, &raw_bytes, &bad_sig).is_err());

    let stale_event_time = now - 600; // 10 minutes ago (tolerance 300s)
    assert!(WebhookVerifier::verify_freshness(stale_event_time, now, 300).is_err());

    let fresh_event_time = now - 60; // 1 minute ago
    assert!(WebhookVerifier::verify_freshness(fresh_event_time, now, 300).is_ok());

    let dedupe = demo::webhook::WebhookDeduplicator::new();
    assert!(dedupe.check_and_record("evt_unique_1").is_ok());
    assert!(dedupe.check_and_record("evt_unique_1").is_err());
}

#[test]
fn test_multi_rail_solana_settlement_and_receipt() {
    let secret = b"rail-test-secret".to_vec();
    let state = AppState::new(secret).expect("app state");

    let t = Transfer::new_immediate(
        TransferId::new(99_001),
        AccountDirectory::customer(101),
        AccountDirectory::merchant(1),
        Amount::new(80_000),
        1_700_000_000,
    )
    .expect("transfer");

    {
        let mut ledger = state.ledger.write().unwrap();
        ledger.create_transfer(t.clone()).expect("post transfer");
    }

    let submission = state
        .solana_rail
        .submit_batch(1, std::slice::from_ref(&t))
        .expect("submit batch to solana");

    assert_eq!(submission.rail_name, "Solana-USDC");
    assert_eq!(submission.batch_seq, 1);
    assert_eq!(submission.transfer_count, 1);
    assert_eq!(submission.total_amount, 80_000);
    assert!(submission.merkle_root.is_some());

    let receipt = state
        .solana_rail
        .generate_receipt(1, 0, &t)
        .expect("generate receipt");

    assert_eq!(receipt.transfer_id, 99_001);
    assert_eq!(receipt.amount, 80_000);
    assert!(receipt.verify().is_ok());
}

#[test]
fn test_three_way_reconciliation_zero_drift_and_incident_detection() {
    let secret = b"recon-secret".to_vec();
    let state = AppState::new(secret).expect("app state");

    let merchant_id = 1;
    let customer_id = 101;
    let payment_id = 5001;
    let hold_id = 6001;
    let post_id = 7001;
    let now = 1_700_000_000;

    let hold = Transfer::new_pending(
        TransferId::new(hold_id),
        AccountDirectory::customer(customer_id),
        AccountDirectory::merchant(merchant_id),
        Amount::new(45_000),
        now,
    )
    .unwrap();

    {
        let mut ledger = state.ledger.write().unwrap();
        ledger.create_pending(hold).unwrap();
        ledger
            .post_pending(
                TransferId::new(hold_id),
                TransferId::new(post_id),
                Amount::new(45_000),
                now + 1,
            )
            .unwrap();
    }

    let mut payment = Payment::new_authorized(
        payment_id,
        merchant_id,
        customer_id,
        45_000,
        0,
        hold_id,
        now,
    );
    payment.transition_to_captured(post_id, now + 1).unwrap();

    let payments = vec![payment.clone()];
    let merchants = vec![1, 2, 3];

    let report = {
        let ledger = state.ledger.read().unwrap();
        ReconciliationEngine::audit(&payments, &ledger, &merchants, None, now + 10).unwrap()
    };

    assert_eq!(report.status, ReconciliationStatus::Clean);
    assert_eq!(report.drift, 0);
    assert_eq!(report.app_settled_total, 45_000);
    assert_eq!(report.ledger_settled_total, 45_000);
    assert!(report.incident.is_none());

    let mut tampered_payment = payment;
    tampered_payment.amount = 55_000;
    let tampered_payments = vec![tampered_payment];

    let drift_report = {
        let ledger = state.ledger.read().unwrap();
        ReconciliationEngine::audit(&tampered_payments, &ledger, &merchants, None, now + 20)
            .unwrap()
    };

    assert_eq!(drift_report.status, ReconciliationStatus::DriftDetected);
    assert_eq!(drift_report.drift, -10_000); // ledger is $10k less than app!
    assert!(drift_report.incident.is_some());
    let incident = drift_report.incident.unwrap();
    assert_eq!(incident.drift_amount, -10_000);
}

#[tokio::test]
async fn test_http_api_smoke_and_dashboard() {
    let secret = b"http-test-secret".to_vec();
    let state = Arc::new(AppState::new(secret).expect("app state"));
    let app = demo::api::router(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local addr");

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let client = reqwest_or_hyper(addr, "/").await;
    assert!(client.contains("TrustLedger Settlement Engine"));

    let metrics_resp = reqwest_or_hyper(addr, "/metrics").await;
    assert!(metrics_resp.contains("HTTP/1.1 200 OK"));
    assert!(metrics_resp.contains("trustledger_transfers_received_total"));
    assert!(metrics_resp.contains("trustledger_reconciliation_drift_gauge"));
}

#[tokio::test]
async fn test_interactive_dashboard_api_endpoints() {
    let secret = b"interactive-ui-test-secret".to_vec();
    let state = Arc::new(AppState::new(secret).expect("app state"));
    let app = demo::api::router(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local addr");

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let dash_resp = reqwest_or_hyper(addr, "/api/dashboard/data").await;
    assert!(dash_resp.contains("HTTP/1.1 200 OK"));
    assert!(dash_resp.contains("\"accounts\":["));
    assert!(dash_resp.contains("\"solana_pda\":"));

    let seed_resp = post_json(addr, "/api/payments/quick-seed", "{}").await;
    assert!(seed_resp.contains("HTTP/1.1 200 OK"));
    assert!(seed_resp.contains("\"seeded_count\":4"));

    let batch_resp = post_json(addr, "/api/settlement/batch", r#"{"rail":"Solana-USDC"}"#).await;
    assert!(batch_resp.contains("HTTP/1.1 200 OK"));
    assert!(batch_resp.contains("\"batch_seq\":1"));
    assert!(batch_resp.contains("\"merkle_root\":"));

    let verify_resp = post_json(addr, "/api/payments/5000/verify", "{}").await;
    assert!(verify_resp.contains("HTTP/1.1 200 OK"));
    assert!(verify_resp.contains("\"verified\":true"));
    assert!(verify_resp.contains("Cryptographic proof matches Solana PDA Merkle root"));

    let duplicate_batch =
        post_json(addr, "/api/settlement/batch", r#"{"rail":"Solana-USDC"}"#).await;
    assert!(duplicate_batch.contains("HTTP/1.1 400 Bad Request"));

    let drift_resp = post_json(addr, "/api/reconciliation/drift-simulate", "{}").await;
    assert!(drift_resp.contains("HTTP/1.1 200 OK"));
    assert!(drift_resp.contains("\"status\":\"DriftDetected\""));
    assert!(drift_resp.contains("\"drift\":50000000"));

    let heal_resp = post_json(addr, "/api/reconciliation/drift-heal", "{}").await;
    assert!(heal_resp.contains("HTTP/1.1 200 OK"));
    assert!(heal_resp.contains("\"status\":\"Clean\""));
    assert!(heal_resp.contains("\"drift\":0"));

    let sim_resp = post_json(
        addr,
        "/api/simulator/run",
        r#"{"scenario":"NetworkPartition","seed":42}"#,
    )
    .await;
    assert!(sim_resp.contains("HTTP/1.1 200 OK"));
    assert!(sim_resp.contains("\"status\":\"PASSED\""));
    assert!(sim_resp.contains("PASSED: Wealth conservation"));
}

async fn reqwest_or_hyper(addr: std::net::SocketAddr, path: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(addr).await.expect("connect");
    let req = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    stream
        .write_all(req.as_bytes())
        .await
        .expect("write request");

    let mut resp = String::new();
    stream
        .read_to_string(&mut resp)
        .await
        .expect("read response");
    resp
}

async fn post_json(addr: std::net::SocketAddr, path: &str, body: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(addr).await.expect("connect");
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(req.as_bytes()).await.expect("write post");

    let mut resp = String::new();
    stream.read_to_string(&mut resp).await.expect("read post");
    resp
}
