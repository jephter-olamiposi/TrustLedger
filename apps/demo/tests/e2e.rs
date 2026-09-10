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

    // Merchant 1, Customer 101
    let merchant_id = 1;
    let customer_id = 101;
    let initial_balance = 1_000_000u128; // $1,000.00 seeded

    // 1. Authorize $50.00 (50,000 units)
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

    // Verify hold reserved customer funds
    {
        let ledger = state.ledger.read().unwrap();
        let cust_acc = ledger
            .get_account(AccountDirectory::customer(customer_id))
            .unwrap();
        // Credits posted is 1,000,000, debits pending is 50,000
        assert_eq!(cust_acc.balance.credits_posted.as_u128(), initial_balance);
        assert_eq!(cust_acc.balance.debits_pending.as_u128(), 50_000);
    }

    // 2. Capture the payment
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

    // Verify merchant received funds
    {
        let ledger = state.ledger.read().unwrap();
        let merch_acc = ledger
            .get_account(AccountDirectory::merchant(merchant_id))
            .unwrap();
        assert_eq!(merch_acc.balance.credits_posted.as_u128(), 50_000);
    }

    // 3. Authorize and Void a second payment ($20.00)
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

    // Verify ledger invariants strictly hold
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
    let valid_signature = WebhookVerifier::compute_signature(secret, &raw_bytes);

    // 1. Valid signature passes
    assert!(WebhookVerifier::verify_signature(secret, &raw_bytes, &valid_signature).is_ok());

    // 2. Tampered payload rejects
    let mut tampered_bytes = raw_bytes.clone();
    tampered_bytes[10] ^= 0x01;
    assert!(WebhookVerifier::verify_signature(secret, &tampered_bytes, &valid_signature).is_err());

    // 3. Forged signature rejects
    let bad_sig = "a".repeat(64);
    assert!(WebhookVerifier::verify_signature(secret, &raw_bytes, &bad_sig).is_err());

    // 4. Freshness check: expired event rejects
    let stale_event_time = now - 600; // 10 minutes ago (tolerance 300s)
    assert!(WebhookVerifier::verify_freshness(stale_event_time, now, 300).is_err());

    // 5. Fresh event within window passes
    let fresh_event_time = now - 60; // 1 minute ago
    assert!(WebhookVerifier::verify_freshness(fresh_event_time, now, 300).is_ok());

    // 6. Deduplication check
    let dedupe = demo::webhook::WebhookDeduplicator::new();
    assert!(dedupe.check_and_record("evt_unique_1").is_ok());
    // Duplicate call with same event_id must fail
    assert!(dedupe.check_and_record("evt_unique_1").is_err());
}

#[test]
fn test_multi_rail_solana_settlement_and_receipt() {
    let secret = b"rail-test-secret".to_vec();
    let state = AppState::new(secret).expect("app state");

    // Perform a transfer
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

    // Settle on Solana USDC rail
    let submission = state
        .solana_rail
        .submit_batch(1, std::slice::from_ref(&t))
        .expect("submit batch to solana");

    assert_eq!(submission.rail_name, "Solana-USDC");
    assert_eq!(submission.batch_seq, 1);
    assert_eq!(submission.transfer_count, 1);
    assert_eq!(submission.total_amount, 80_000);
    assert!(submission.merkle_root.is_some());

    // Generate and independently verify transfer receipt
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

    // 1. Setup matching state in App and Ledger
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

    // Audit happy path: drift must be 0
    let report = {
        let ledger = state.ledger.read().unwrap();
        ReconciliationEngine::audit(&payments, &ledger, &merchants, None, now + 10).unwrap()
    };

    assert_eq!(report.status, ReconciliationStatus::Clean);
    assert_eq!(report.drift, 0);
    assert_eq!(report.app_settled_total, 45_000);
    assert_eq!(report.ledger_settled_total, 45_000);
    assert!(report.incident.is_none());

    // 2. Simulate Drift: App payment claims $55,000, but ledger only recorded $45,000
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

    // Check dashboard HTML endpoint
    let client = reqwest_or_hyper(addr).await;
    assert!(client.contains("TrustLedger Settlement Engine"));
}

async fn reqwest_or_hyper(addr: std::net::SocketAddr) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(addr).await.expect("connect");
    let req = "GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n";
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
