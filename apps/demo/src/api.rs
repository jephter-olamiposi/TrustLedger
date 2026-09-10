//! HTTP API handlers for payments, webhooks, settlement batches, and reconciliation.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use axum::extract::{Path as AxumPath, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::{Json, Router};
use ledger_core::amount::{Amount, Scale};
use ledger_core::id::TransferId;
use ledger_core::transfer::Transfer;
use ledger_core::Ledger;
use serde::{Deserialize, Serialize};
use solana_program::pubkey::Pubkey;
use solana_settle::client::TransferReceipt;

use crate::dashboard::render_dashboard_html;
use crate::error::DemoError;
use crate::idempotency::IdempotencyStore;
use crate::payment::{AccountDirectory, Payment, PaymentState};
use crate::rail::{MockAchRail, RailSubmission, SettlementRail, SolanaUsdcRail};
use crate::reconciliation::{ReconciliationEngine, ReconciliationReport};
use crate::webhook::{WebhookDeduplicator, WebhookPayload, WebhookVerifier};

/// Shared state container for the demo HTTP server.
pub struct AppState {
    /// In-memory double-entry ledger.
    pub ledger: RwLock<Ledger>,
    /// In-memory payment repository.
    pub payments: RwLock<HashMap<u128, Payment>>,
    /// Idempotency coordinator.
    pub idempotency: IdempotencyStore,
    /// Webhook deduplicator.
    pub webhook_dedupe: WebhookDeduplicator,
    /// Active Solana USDC settlement rail.
    pub solana_rail: SolanaUsdcRail,
    /// Mock ACH settlement rail.
    pub ach_rail: MockAchRail,
    /// Shared HMAC secret for incoming webhooks.
    pub webhook_secret: Vec<u8>,
    /// Currency decimal scale.
    pub scale: Scale,
    /// Atomic payment ID generator.
    pub next_payment_id: AtomicU64,
    /// Atomic transfer ID generator.
    pub next_transfer_id: AtomicU64,
    /// Current settlement batch sequence counter.
    pub batch_seq: AtomicU64,
}

impl AppState {
    /// Create a new, initialized application state with sample accounts.
    ///
    /// # Errors
    ///
    /// Returns [`DemoError`] if account initialization or Solana rail setup fails.
    pub fn new(webhook_secret: Vec<u8>) -> Result<Self, DemoError> {
        let scale = Scale::usdc();
        let mut ledger = Ledger::new(scale);

        let merchants = vec![1, 2, 3];
        let customers = vec![101, 102, 103];

        // Seed accounts with 1,000,000 units ($1,000.00 USDC) initial deposit
        AccountDirectory::setup_accounts(
            &mut ledger,
            scale,
            &merchants,
            &customers,
            1_000_000,
            1_700_000_000,
        )?;

        let program_id = Pubkey::new_unique();
        let authority = Pubkey::new_unique();
        let solana_rail = SolanaUsdcRail::new(program_id, authority)?;

        Ok(Self {
            ledger: RwLock::new(ledger),
            payments: RwLock::new(HashMap::new()),
            idempotency: IdempotencyStore::new(),
            webhook_dedupe: WebhookDeduplicator::new(),
            solana_rail,
            ach_rail: MockAchRail::new(),
            webhook_secret,
            scale,
            next_payment_id: AtomicU64::new(5000),
            next_transfer_id: AtomicU64::new(20_000),
            batch_seq: AtomicU64::new(0),
        })
    }
}

/// Build the Axum router with all API and dashboard endpoints.
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(handle_dashboard))
        .route("/api/payments/authorize", post(handle_authorize))
        .route("/api/payments/:id/capture", post(handle_capture))
        .route("/api/payments/:id/void", post(handle_void))
        .route("/api/balances/:merchant_id", get(handle_get_balance))
        .route("/api/webhooks/card", post(handle_webhook))
        .route("/api/settlement/batch", post(handle_settlement_batch))
        .route("/api/reconciliation", get(handle_reconciliation))
        .route("/api/verify", post(handle_verify_receipt))
        .with_state(state)
}

/// Request payload to authorize a payment.
#[derive(Debug, Deserialize)]
pub struct AuthorizeRequest {
    /// Merchant ID receiving funds.
    pub merchant_id: u128,
    /// Customer ID paying.
    pub customer_id: u128,
    /// Payment amount in base decimal units.
    pub amount: u128,
    /// Platform processing fee in units.
    pub fee_amount: u128,
    /// Unique client idempotency key.
    pub idempotency_key: String,
}

/// Response payload returned after payment operations.
#[derive(Debug, Serialize, Deserialize)]
pub struct PaymentResponse {
    /// Payment identifier.
    pub payment_id: u128,
    /// Lifecycle state.
    pub state: PaymentState,
    /// Gross amount.
    pub amount: u128,
    /// Associated pending hold ID.
    pub pending_transfer_id: Option<u128>,
    /// Associated posted settlement transfer ID.
    pub posted_transfer_id: Option<u128>,
}

async fn handle_dashboard(State(state): State<Arc<AppState>>) -> Html<String> {
    let payments: Vec<Payment> = state
        .payments
        .read()
        .map(|g| g.values().cloned().collect())
        .unwrap_or_default();
    let batch_seq = state.batch_seq.load(Ordering::SeqCst);

    Html(render_dashboard_html(&payments, batch_seq))
}

async fn handle_authorize(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AuthorizeRequest>,
) -> Result<Json<PaymentResponse>, (StatusCode, String)> {
    // 1. Idempotency check
    if let Some(cached_json) = state
        .idempotency
        .try_acquire(&req.idempotency_key)
        .map_err(|e| (StatusCode::CONFLICT, e.to_string()))?
    {
        let resp: PaymentResponse = serde_json::from_str(&cached_json)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        return Ok(Json(resp));
    }

    let payment_id = state.next_payment_id.fetch_add(1, Ordering::SeqCst) as u128;
    let transfer_id = state.next_transfer_id.fetch_add(1, Ordering::SeqCst) as u128;
    let now = 1_700_000_000;

    // 2. Reserve funds on ledger via pending hold
    let customer_acc = AccountDirectory::customer(req.customer_id);
    let merchant_acc = AccountDirectory::merchant(req.merchant_id);

    let hold = Transfer::new_pending(
        TransferId::new(transfer_id),
        customer_acc,
        merchant_acc,
        Amount::new(req.amount),
        now,
    )
    .map_err(|e| {
        state.idempotency.release_on_failure(&req.idempotency_key);
        (StatusCode::BAD_REQUEST, e.to_string())
    })?;

    {
        let mut ledger = state.ledger.write().map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "lock poisoned".to_string(),
            )
        })?;
        ledger.create_pending(hold).map_err(|e| {
            state.idempotency.release_on_failure(&req.idempotency_key);
            (StatusCode::PAYMENT_REQUIRED, e.to_string())
        })?;
    }

    let payment = Payment::new_authorized(
        payment_id,
        req.merchant_id,
        req.customer_id,
        req.amount,
        req.fee_amount,
        transfer_id,
        now,
    );

    let resp = PaymentResponse {
        payment_id,
        state: payment.state,
        amount: payment.amount,
        pending_transfer_id: payment.pending_transfer_id,
        posted_transfer_id: payment.posted_transfer_id,
    };

    {
        let mut payments = state.payments.write().map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "lock poisoned".to_string(),
            )
        })?;
        payments.insert(payment_id, payment);
    }

    let resp_json = serde_json::to_string(&resp)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    state.idempotency.complete(&req.idempotency_key, resp_json);

    Ok(Json(resp))
}

async fn handle_capture(
    State(state): State<Arc<AppState>>,
    AxumPath(payment_id): AxumPath<u128>,
) -> Result<Json<PaymentResponse>, (StatusCode, String)> {
    let mut payments = state.payments.write().map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "lock poisoned".to_string(),
        )
    })?;
    let payment = payments.get_mut(&payment_id).ok_or((
        StatusCode::NOT_FOUND,
        format!("payment {payment_id} not found"),
    ))?;

    let pending_id = payment.pending_transfer_id.ok_or((
        StatusCode::BAD_REQUEST,
        "payment does not have pending transfer".to_string(),
    ))?;

    let post_transfer_id = state.next_transfer_id.fetch_add(1, Ordering::SeqCst) as u128;
    let now = 1_700_000_100;

    // Post hold on ledger
    {
        let mut ledger = state.ledger.write().map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "lock poisoned".to_string(),
            )
        })?;
        ledger
            .post_pending(
                TransferId::new(pending_id),
                TransferId::new(post_transfer_id),
                Amount::new(payment.amount),
                now,
            )
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }

    payment
        .transition_to_captured(post_transfer_id, now)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    Ok(Json(PaymentResponse {
        payment_id,
        state: payment.state,
        amount: payment.amount,
        pending_transfer_id: payment.pending_transfer_id,
        posted_transfer_id: payment.posted_transfer_id,
    }))
}

async fn handle_void(
    State(state): State<Arc<AppState>>,
    AxumPath(payment_id): AxumPath<u128>,
) -> Result<Json<PaymentResponse>, (StatusCode, String)> {
    let mut payments = state.payments.write().map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "lock poisoned".to_string(),
        )
    })?;
    let payment = payments.get_mut(&payment_id).ok_or((
        StatusCode::NOT_FOUND,
        format!("payment {payment_id} not found"),
    ))?;

    let pending_id = payment.pending_transfer_id.ok_or((
        StatusCode::BAD_REQUEST,
        "payment does not have pending transfer".to_string(),
    ))?;

    let now = 1_700_000_100;

    // Void hold on ledger
    {
        let mut ledger = state.ledger.write().map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "lock poisoned".to_string(),
            )
        })?;
        ledger
            .void_pending(TransferId::new(pending_id), now)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }

    payment
        .transition_to_voided(now)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    Ok(Json(PaymentResponse {
        payment_id,
        state: payment.state,
        amount: payment.amount,
        pending_transfer_id: payment.pending_transfer_id,
        posted_transfer_id: payment.posted_transfer_id,
    }))
}

/// Balance representation for merchant account.
#[derive(Debug, Serialize)]
pub struct BalanceResponse {
    /// Merchant ID.
    pub merchant_id: u128,
    /// Posted settled balance.
    pub posted_balance: u128,
    /// Pending reserved balance.
    pub pending_balance: u128,
}

async fn handle_get_balance(
    State(state): State<Arc<AppState>>,
    AxumPath(merchant_id): AxumPath<u128>,
) -> Result<Json<BalanceResponse>, (StatusCode, String)> {
    let acc_id = AccountDirectory::merchant(merchant_id);
    let ledger = state.ledger.read().map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "lock poisoned".to_string(),
        )
    })?;
    let acc = ledger.get_account(acc_id).map_err(|_| {
        (
            StatusCode::NOT_FOUND,
            "merchant account not found".to_string(),
        )
    })?;

    Ok(Json(BalanceResponse {
        merchant_id,
        posted_balance: acc.balance.credits_posted.as_u128(),
        pending_balance: acc.balance.credits_pending.as_u128(),
    }))
}

async fn handle_webhook(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: String,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let signature = headers
        .get("x-webhook-signature")
        .and_then(|v| v.to_str().ok())
        .ok_or((
            StatusCode::UNAUTHORIZED,
            "missing signature header".to_string(),
        ))?;

    // 1. Verify HMAC
    WebhookVerifier::verify_signature(&state.webhook_secret, body.as_bytes(), signature).map_err(
        |_| {
            (
                StatusCode::UNAUTHORIZED,
                "invalid HMAC signature".to_string(),
            )
        },
    )?;

    let payload: WebhookPayload =
        serde_json::from_str(&body).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    // 2. Freshness check (5 min tolerance)
    let now = 1_700_000_100;
    WebhookVerifier::verify_freshness(payload.timestamp, now, 300)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    // 3. Deduplication check
    state
        .webhook_dedupe
        .check_and_record(&payload.event_id)
        .map_err(|e| (StatusCode::CONFLICT, e.to_string()))?;

    // Process event
    match payload.event_type.as_str() {
        "charge.authorized" => {
            let transfer_id = state.next_transfer_id.fetch_add(1, Ordering::SeqCst) as u128;
            let customer_acc = AccountDirectory::customer(payload.customer_id);
            let merchant_acc = AccountDirectory::merchant(payload.merchant_id);

            let hold = Transfer::new_pending(
                TransferId::new(transfer_id),
                customer_acc,
                merchant_acc,
                Amount::new(payload.amount),
                payload.timestamp,
            )
            .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

            {
                let mut ledger = state.ledger.write().map_err(|_| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "lock poisoned".to_string(),
                    )
                })?;
                ledger
                    .create_pending(hold)
                    .map_err(|e| (StatusCode::PAYMENT_REQUIRED, e.to_string()))?;
            }

            let payment = Payment::new_authorized(
                payload.payment_id,
                payload.merchant_id,
                payload.customer_id,
                payload.amount,
                payload.fee_amount,
                transfer_id,
                payload.timestamp,
            );

            state
                .payments
                .write()
                .map_err(|_| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "lock poisoned".to_string(),
                    )
                })?
                .insert(payload.payment_id, payment);
        }
        "charge.captured" => {
            let mut payments = state.payments.write().map_err(|_| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "lock poisoned".to_string(),
                )
            })?;
            if let Some(payment) = payments.get_mut(&payload.payment_id) {
                if let Some(pending_id) = payment.pending_transfer_id {
                    let post_id = state.next_transfer_id.fetch_add(1, Ordering::SeqCst) as u128;
                    {
                        let mut ledger = state.ledger.write().map_err(|_| {
                            (
                                StatusCode::INTERNAL_SERVER_ERROR,
                                "lock poisoned".to_string(),
                            )
                        })?;
                        ledger
                            .post_pending(
                                TransferId::new(pending_id),
                                TransferId::new(post_id),
                                Amount::new(payment.amount),
                                payload.timestamp,
                            )
                            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
                    }
                    payment
                        .transition_to_captured(post_id, payload.timestamp)
                        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
                }
            }
        }
        _ => return Err((StatusCode::BAD_REQUEST, "unknown event type".to_string())),
    }

    Ok((StatusCode::OK, "Webhook processed successfully"))
}

/// Request payload to trigger a settlement batch on a chosen rail.
#[derive(Debug, Deserialize)]
pub struct BatchRequest {
    /// Rail to settle on: `Solana-USDC` (default) or `Mock-ACH`.
    pub rail: Option<String>,
}

/// Settlement batch submission summary response.
#[derive(Debug, Serialize)]
pub struct BatchResponse {
    /// Name of the settlement rail used.
    pub rail: String,
    /// Batch sequence number assigned to this settlement.
    pub batch_seq: u64,
    /// Number of transfers included in this batch.
    pub transfer_count: usize,
    /// Sum of all transfer amounts settled in this batch.
    pub total_amount: u128,
    /// External transaction or batch reference identifier.
    pub reference: String,
    /// Hex-encoded Merkle root if settled on Solana MMR rail.
    pub merkle_root: Option<String>,
}

async fn handle_settlement_batch(
    State(state): State<Arc<AppState>>,
    Json(req): Json<BatchRequest>,
) -> Result<Json<BatchResponse>, (StatusCode, String)> {
    let next_seq = state.batch_seq.fetch_add(1, Ordering::SeqCst) + 1;

    // Collect posted transfers from ledger
    let transfers: Vec<Transfer> = {
        let ledger = state.ledger.read().map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "lock poisoned".to_string(),
            )
        })?;
        ledger
            .transfers()
            .filter(|t| t.state() == ledger_core::transfer::TransferState::Posted)
            .cloned()
            .collect()
    };

    if transfers.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "no transfers to settle".to_string(),
        ));
    }

    let rail_choice = req.rail.unwrap_or_else(|| "Solana-USDC".to_string());
    let submission: RailSubmission = if rail_choice == "Mock-ACH" {
        state
            .ach_rail
            .submit_batch(next_seq, &transfers)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    } else {
        state
            .solana_rail
            .submit_batch(next_seq, &transfers)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    };

    Ok(Json(BatchResponse {
        rail: submission.rail_name,
        batch_seq: submission.batch_seq,
        transfer_count: submission.transfer_count,
        total_amount: submission.total_amount,
        reference: submission.reference,
        merkle_root: submission.merkle_root,
    }))
}

async fn handle_reconciliation(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ReconciliationReport>, (StatusCode, String)> {
    let payments_guard = state.payments.read().map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "lock poisoned".to_string(),
        )
    })?;
    let payments: Vec<Payment> = payments_guard.values().cloned().collect();
    let ledger = state.ledger.read().map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "lock poisoned".to_string(),
        )
    })?;
    let merchants = vec![1, 2, 3];

    let report = ReconciliationEngine::audit(&payments, &ledger, &merchants, None, 1_700_000_200)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(report))
}

/// Request payload to verify a transfer receipt against its Merkle proof.
#[derive(Debug, Deserialize)]
pub struct VerifyRequest {
    /// Transfer receipt containing cryptographic proof and metadata.
    pub receipt: TransferReceipt,
}

/// Result of a cryptographic transfer receipt verification.
#[derive(Debug, Serialize)]
pub struct VerifyResponse {
    /// True if cryptographic proof matches the root, false otherwise.
    pub verified: bool,
    /// Transfer ID verified.
    pub transfer_id: u128,
    /// Settlement batch sequence number.
    pub batch_seq: u64,
    /// Hex-encoded Merkle root verified against.
    pub merkle_root: String,
    /// Human-readable verification explanation.
    pub message: String,
}

async fn handle_verify_receipt(
    Json(req): Json<VerifyRequest>,
) -> Result<Json<VerifyResponse>, (StatusCode, String)> {
    match req.receipt.verify() {
        Ok(()) => Ok(Json(VerifyResponse {
            verified: true,
            transfer_id: req.receipt.transfer_id,
            batch_seq: req.receipt.batch_seq,
            merkle_root: req.receipt.merkle_root,
            message: "Cryptographic proof matches on-chain Merkle root. Transfer is immutable."
                .to_string(),
        })),
        Err(e) => Err((
            StatusCode::BAD_REQUEST,
            format!("Proof verification failed: {e}"),
        )),
    }
}
