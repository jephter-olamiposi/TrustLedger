//! HTTP API handlers for payments, webhooks, settlement batches, and reconciliation.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

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
    /// In-memory double-entry ledger state.
    pub ledger: RwLock<Ledger>,
    /// In-memory payment repository.
    pub payments: RwLock<HashMap<u128, Payment>>,
    pub(crate) idempotency: IdempotencyStore,
    pub(crate) webhook_dedupe: WebhookDeduplicator,
    /// Active Solana USDC settlement rail adapter.
    pub solana_rail: SolanaUsdcRail,
    pub(crate) ach_rail: MockAchRail,
    pub(crate) webhook_secret: Vec<u8>,
    pub(crate) scale: Scale,
    /// Monotonic sequence generator for payment identifiers.
    pub next_payment_id: AtomicU64,
    /// Monotonic sequence generator for ledger transfer identifiers.
    pub next_transfer_id: AtomicU64,
    /// Monotonic sequence generator for ledger event timestamps.
    pub next_timestamp: AtomicU64,
    pub(crate) batch_seq: AtomicU64,
    /// Serialization lock ensuring settlement batching runs single-threaded.
    pub settlement_lock: Mutex<()>,
    pub(crate) metrics: observability::LedgerMetrics,
    pub(crate) simulated_drift: AtomicI64,
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

        AccountDirectory::setup_accounts(
            &mut ledger,
            scale,
            &merchants,
            &customers,
            1_000_000_000,
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
            next_timestamp: AtomicU64::new(1_700_000_001),
            batch_seq: AtomicU64::new(0),
            settlement_lock: Mutex::new(()),
            metrics: observability::LedgerMetrics::new(),
            simulated_drift: AtomicI64::new(0),
        })
    }

    /// Monotonically allocate the next timestamp for double-entry mutations.
    #[must_use]
    pub fn next_timestamp(&self) -> u64 {
        self.next_timestamp.fetch_add(1, Ordering::SeqCst)
    }

    /// Currency decimal scale configured for this ledger.
    #[must_use]
    pub const fn scale(&self) -> Scale {
        self.scale
    }
}

/// Build the Axum router with all API and dashboard endpoints.
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(handle_dashboard))
        .route("/metrics", get(handle_metrics))
        .route("/api/dashboard/data", get(handle_dashboard_data))
        .route("/api/payments/quick-seed", post(handle_quick_seed))
        .route("/api/payments/authorize", post(handle_authorize))
        .route("/api/payments/:id/capture", post(handle_capture))
        .route("/api/payments/:id/void", post(handle_void))
        .route("/api/payments/:id/verify", post(handle_payment_verify))
        .route("/api/balances/:merchant_id", get(handle_get_balance))
        .route("/api/customers/:id/deposit", post(handle_customer_deposit))
        .route("/api/webhooks/card", post(handle_webhook))
        .route("/api/settlement/batch", post(handle_settlement_batch))
        .route("/api/reconciliation", get(handle_reconciliation))
        .route(
            "/api/reconciliation/drift-simulate",
            post(handle_simulate_drift),
        )
        .route("/api/reconciliation/drift-heal", post(handle_heal_drift))
        .route("/api/simulator/run", post(handle_simulator_run))
        .route("/api/verify", post(handle_verify_receipt))
        .with_state(state)
}

#[derive(Debug, Deserialize)]
pub(crate) struct AuthorizeRequest {
    pub(crate) merchant_id: u128,
    pub(crate) customer_id: u128,
    pub(crate) amount: u128,
    pub(crate) fee_amount: u128,
    pub(crate) idempotency_key: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct PaymentResponse {
    pub(crate) payment_id: u128,
    pub(crate) state: PaymentState,
    pub(crate) amount: u128,
    pub(crate) pending_transfer_id: Option<u128>,
    pub(crate) posted_transfer_id: Option<u128>,
}

async fn handle_dashboard(
    State(state): State<Arc<AppState>>,
) -> Result<Html<String>, (StatusCode, String)> {
    let mut payments: Vec<Payment> = state
        .payments
        .read()
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "lock poisoned".to_string(),
            )
        })?
        .values()
        .cloned()
        .collect();
    payments.sort_by_key(|a| std::cmp::Reverse(a.id));
    let batch_seq = state.batch_seq.load(Ordering::SeqCst);
    let solana_authority = state.solana_rail.authority().to_string();
    let solana_pda = state.solana_rail.pda_address().to_string();
    let latest_root = state.solana_rail.latest_root_hex();

    Ok(Html(render_dashboard_html(
        &payments,
        batch_seq,
        &solana_authority,
        &solana_pda,
        latest_root.as_deref(),
    )))
}

async fn handle_authorize(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AuthorizeRequest>,
) -> Result<Json<PaymentResponse>, (StatusCode, String)> {
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
    let now = state.next_timestamp();

    let customer_acc = AccountDirectory::customer(req.customer_id);
    let merchant_acc = AccountDirectory::merchant(req.merchant_id);

    state.metrics.transfers_received.inc();
    let hold = Transfer::new_pending(
        TransferId::new(transfer_id),
        customer_acc,
        merchant_acc,
        Amount::new(req.amount),
        now,
    )
    .map_err(|e| {
        state.metrics.transfers_rejected.inc();
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
            state.metrics.transfers_rejected.inc();
            state.idempotency.release_on_failure(&req.idempotency_key);
            (StatusCode::PAYMENT_REQUIRED, e.to_string())
        })?;
    }

    state.metrics.transfers_committed.inc();
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
    let now = state.next_timestamp();

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

    let now = state.next_timestamp();

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
pub(crate) struct BalanceResponse {
    pub(crate) merchant_id: u128,
    pub(crate) posted_balance: u128,
    pub(crate) pending_balance: u128,
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

#[derive(Debug, Deserialize)]
pub(crate) struct DepositRequest {
    pub(crate) amount: Option<u128>,
}

#[derive(Debug, Serialize)]
pub(crate) struct DepositResponse {
    pub(crate) customer_id: u128,
    pub(crate) deposited_amount: u128,
    pub(crate) new_balance: u128,
    pub(crate) message: String,
}

async fn handle_customer_deposit(
    State(state): State<Arc<AppState>>,
    AxumPath(customer_id): AxumPath<u128>,
    Json(req): Json<DepositRequest>,
) -> Result<Json<DepositResponse>, (StatusCode, String)> {
    let deposit_amount = req.amount.unwrap_or(100_000_000); // $100.00 default
    let transfer_id = state.next_transfer_id.fetch_add(1, Ordering::SeqCst) as u128;
    let now = state.next_timestamp();

    let cust_acc = AccountDirectory::customer(customer_id);
    let deposit = Transfer::new_immediate(
        TransferId::new(transfer_id),
        AccountDirectory::VAULT_ASSET,
        cust_acc,
        Amount::new(deposit_amount),
        now,
    )
    .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    let new_balance = {
        let mut ledger = state.ledger.write().map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "lock poisoned".to_string(),
            )
        })?;
        ledger
            .create_transfer(deposit)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        let acc = ledger.get_account(cust_acc).map_err(|_| {
            (
                StatusCode::NOT_FOUND,
                format!("Customer #{customer_id} account not found"),
            )
        })?;
        acc.balance.credits_posted.as_u128()
    };

    Ok(Json(DepositResponse {
        customer_id,
        deposited_amount: deposit_amount,
        new_balance,
        message: format!(
            "Successfully added ${:.2} to Customer #{customer_id}. New balance: ${:.2}",
            deposit_amount as f64 / 1_000_000.0,
            new_balance as f64 / 1_000_000.0
        ),
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

    let now = 1_700_000_100;
    WebhookVerifier::verify_freshness(payload.timestamp, now, 300)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    state
        .webhook_dedupe
        .check_and_record(&payload.event_id)
        .map_err(|e| (StatusCode::CONFLICT, e.to_string()))?;

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

#[derive(Debug, Deserialize)]
pub(crate) struct BatchRequest {
    pub(crate) rail: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct BatchResponse {
    pub(crate) rail: String,
    pub(crate) batch_seq: u64,
    pub(crate) transfer_count: usize,
    pub(crate) total_amount: u128,
    pub(crate) reference: String,
    pub(crate) merkle_root: Option<String>,
}

async fn handle_settlement_batch(
    State(state): State<Arc<AppState>>,
    Json(req): Json<BatchRequest>,
) -> Result<Json<BatchResponse>, (StatusCode, String)> {
    let _settlement_guard = state.settlement_lock.lock().map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "lock poisoned".to_string(),
        )
    })?;

    let transfers: Vec<Transfer> = {
        let payments = state.payments.read().map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "lock poisoned".to_string(),
            )
        })?;
        let posted_ids: std::collections::HashSet<u128> = payments
            .values()
            .filter(|p| p.state == PaymentState::Captured && p.settlement_batch_seq.is_none())
            .filter_map(|p| p.posted_transfer_id)
            .collect();

        let ledger = state.ledger.read().map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "lock poisoned".to_string(),
            )
        })?;

        ledger
            .transfers()
            .filter(|t| posted_ids.contains(&t.id().as_u128()))
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
    let rail_batch_seq = if rail_choice == "Mock-ACH" {
        state
            .batch_seq
            .load(Ordering::SeqCst)
            .checked_add(1)
            .ok_or((
                StatusCode::INTERNAL_SERVER_ERROR,
                "settlement batch sequence exhausted".to_string(),
            ))?
    } else {
        state
            .solana_rail
            .current_settlement_root()
            .map_or(Ok(1), |root| {
                root.batch_seq.checked_add(1).ok_or((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Solana settlement batch sequence exhausted".to_string(),
                ))
            })?
    };
    let submission: RailSubmission = if rail_choice == "Mock-ACH" {
        state
            .ach_rail
            .submit_batch(rail_batch_seq, &transfers)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    } else {
        state
            .solana_rail
            .submit_batch(rail_batch_seq, &transfers)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    };

    state.metrics.settlement_batches_committed.inc();
    state
        .metrics
        .settlement_amount_total
        .inc_by(submission.total_amount as u64);

    let settled_transfer_ids: std::collections::HashSet<u128> = transfers
        .iter()
        .map(|transfer| transfer.id().as_u128())
        .collect();
    state
        .payments
        .write()
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "lock poisoned".to_string(),
            )
        })?
        .values_mut()
        .filter(|payment| {
            payment.state == PaymentState::Captured
                && payment.settlement_batch_seq.is_none()
                && payment
                    .posted_transfer_id
                    .is_some_and(|id| settled_transfer_ids.contains(&id))
        })
        .for_each(|payment| payment.settlement_batch_seq = Some(submission.batch_seq));
    state.batch_seq.fetch_add(1, Ordering::SeqCst);

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
    let chain_root = state.solana_rail.current_settlement_root();

    let mut report = ReconciliationEngine::audit(
        &payments,
        &ledger,
        &merchants,
        chain_root.as_ref(),
        1_700_000_200,
    )
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let drift_offset = state.simulated_drift.load(Ordering::SeqCst) as i128;
    if drift_offset != 0 {
        report.drift += drift_offset;
        report.status = crate::reconciliation::ReconciliationStatus::DriftDetected;
        report.incident = Some(crate::reconciliation::ReconciliationIncident {
            incident_id: format!("INC-SIMULATED-DRIFT-{}", report.timestamp),
            drift_amount: report.drift,
            details: format!(
                "Simulated bank discrepancy: {drift_offset} units (${:.2} USD) unallocated balance outside double-entry journal.",
                drift_offset as f64 / 1_000_000.0
            ),
            recommended_action: "Audit bank clearing records and click 'Heal Drift' to re-balance."
                .to_string(),
        });
    }

    state.metrics.reconciliation_audits_total.inc();
    let drift_i64 = report.drift.clamp(i64::MIN as i128, i64::MAX as i128) as i64;
    state.metrics.reconciliation_drift_gauge.set(drift_i64);
    if report.status == crate::reconciliation::ReconciliationStatus::DriftDetected {
        state.metrics.reconciliation_incidents_total.inc();
    }

    Ok(Json(report))
}

async fn handle_simulate_drift(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ReconciliationReport>, (StatusCode, String)> {
    state.simulated_drift.store(50_000_000, Ordering::SeqCst);
    handle_reconciliation(State(state)).await
}

async fn handle_heal_drift(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ReconciliationReport>, (StatusCode, String)> {
    state.simulated_drift.store(0, Ordering::SeqCst);
    handle_reconciliation(State(state)).await
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct AccountSummary {
    pub(crate) id: u128,
    pub(crate) label: String,
    pub(crate) account_type: String,
    pub(crate) credits_posted: u128,
    pub(crate) debits_posted: u128,
    pub(crate) credits_pending: u128,
    pub(crate) debits_pending: u128,
    pub(crate) net_balance: i128,
}

#[derive(Debug, Serialize)]
pub(crate) struct DashboardDataResponse {
    pub(crate) payments: Vec<Payment>,
    pub(crate) accounts: Vec<AccountSummary>,
    pub(crate) batch_seq: u64,
    pub(crate) solana_authority: String,
    pub(crate) solana_pda: String,
    pub(crate) latest_root_hex: Option<String>,
    pub(crate) reconciliation: ReconciliationReport,
    pub(crate) total_captured_volume: u128,
    pub(crate) total_transfers_count: usize,
}

async fn handle_dashboard_data(
    State(state): State<Arc<AppState>>,
) -> Result<Json<DashboardDataResponse>, (StatusCode, String)> {
    let mut payments: Vec<Payment> = state
        .payments
        .read()
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "lock poisoned".to_string(),
            )
        })?
        .values()
        .cloned()
        .collect();
    payments.sort_by_key(|a| std::cmp::Reverse(a.id));

    let batch_seq = state.batch_seq.load(Ordering::SeqCst);

    let (accounts, total_transfers_count) = {
        let ledger = state.ledger.read().map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "lock poisoned".to_string(),
            )
        })?;

        let mut list = Vec::new();
        let tracked_ids = [
            (
                AccountDirectory::VAULT_ASSET.as_u128(),
                "Bank Clearing Vault",
                "Asset",
            ),
            (
                AccountDirectory::FEE_REVENUE.as_u128(),
                "Platform Fee Revenue",
                "Revenue",
            ),
            (
                AccountDirectory::customer(101).as_u128(),
                "Customer #101 (Alice)",
                "Liability",
            ),
            (
                AccountDirectory::customer(102).as_u128(),
                "Customer #102 (Bob)",
                "Liability",
            ),
            (
                AccountDirectory::customer(103).as_u128(),
                "Customer #103 (Charlie)",
                "Liability",
            ),
            (
                AccountDirectory::merchant(1).as_u128(),
                "Merchant #1 (Acme Corp)",
                "Liability",
            ),
            (
                AccountDirectory::merchant(2).as_u128(),
                "Merchant #2 (Globex)",
                "Liability",
            ),
            (
                AccountDirectory::merchant(3).as_u128(),
                "Merchant #3 (Soylent)",
                "Liability",
            ),
        ];

        for (id, label, acc_type) in tracked_ids {
            if let Ok(acc) = ledger.get_account(ledger_core::id::AccountId::new(id)) {
                let cp = acc.balance.credits_posted.as_u128();
                let dp = acc.balance.debits_posted.as_u128();
                let ch = acc.balance.credits_pending.as_u128();
                let dh = acc.balance.debits_pending.as_u128();
                let net = acc
                    .balance
                    .net_settled(acc.account_type)
                    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

                list.push(AccountSummary {
                    id,
                    label: label.to_string(),
                    account_type: acc_type.to_string(),
                    credits_posted: cp,
                    debits_posted: dp,
                    credits_pending: ch,
                    debits_pending: dh,
                    net_balance: net,
                });
            }
        }

        let transfer_count = ledger.transfers().count();
        (list, transfer_count)
    };

    let mut total_captured: u128 = 0;
    for p in &payments {
        if p.state == PaymentState::Captured {
            total_captured = total_captured.checked_add(p.amount).ok_or((
                StatusCode::INTERNAL_SERVER_ERROR,
                "captured-volume overflow".to_string(),
            ))?;
        }
    }

    let chain_root = state.solana_rail.current_settlement_root();
    let merchants = vec![1, 2, 3];
    let ledger_guard = state.ledger.read().map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "lock poisoned".to_string(),
        )
    })?;

    let mut recon = ReconciliationEngine::audit(
        &payments,
        &ledger_guard,
        &merchants,
        chain_root.as_ref(),
        1_700_000_200,
    )
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let drift_offset = state.simulated_drift.load(Ordering::SeqCst) as i128;
    if drift_offset != 0 {
        recon.drift += drift_offset;
        recon.status = crate::reconciliation::ReconciliationStatus::DriftDetected;
        recon.incident = Some(crate::reconciliation::ReconciliationIncident {
            incident_id: format!("INC-SIMULATED-DRIFT-{}", recon.timestamp),
            drift_amount: recon.drift,
            details: format!(
                "Simulated bank discrepancy: {drift_offset} units (${:.2} USD) unallocated balance outside double-entry journal.",
                drift_offset as f64 / 1_000_000.0
            ),
            recommended_action: "Audit bank clearing records and click 'Heal Drift' to re-balance."
                .to_string(),
        });
    }

    let solana_authority = state.solana_rail.authority().to_string();
    let solana_pda = state.solana_rail.pda_address().to_string();
    let latest_root_hex = state.solana_rail.latest_root_hex();

    Ok(Json(DashboardDataResponse {
        payments,
        accounts,
        batch_seq,
        solana_authority,
        solana_pda,
        latest_root_hex,
        reconciliation: recon,
        total_captured_volume: total_captured,
        total_transfers_count,
    }))
}

#[derive(Debug, Serialize)]
pub(crate) struct QuickSeedResponse {
    pub(crate) seeded_count: usize,
    pub(crate) message: String,
}

async fn handle_quick_seed(
    State(state): State<Arc<AppState>>,
) -> Result<Json<QuickSeedResponse>, (StatusCode, String)> {
    let seeds = [
        (101, 1, 150_000_000u128, 1_500_000u128, "captured"),
        (102, 2, 85_500_000u128, 855_000u128, "captured"),
        (103, 1, 42_000_000u128, 500_000u128, "authorized"),
        (101, 3, 20_000_000u128, 200_000u128, "voided"),
    ];

    // Seed the customer liabilities from the vault before placing holds.
    {
        let mut ledger = state.ledger.write().map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "lock poisoned".to_string(),
            )
        })?;

        for &c_id in &[101, 102, 103] {
            let deposit_now = state.next_timestamp();
            let deposit_id = state.next_transfer_id.fetch_add(1, Ordering::SeqCst) as u128;
            let deposit = Transfer::new_immediate(
                TransferId::new(deposit_id),
                AccountDirectory::VAULT_ASSET,
                AccountDirectory::customer(c_id),
                Amount::new(500_000_000),
                deposit_now,
            )
            .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
            ledger
                .create_transfer(deposit)
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        }
    }

    let mut seeded = 0;
    for (cust, merch, amt, fee, target_state) in seeds {
        let hold_ts = state.next_timestamp();
        let p_id = state.next_payment_id.fetch_add(1, Ordering::SeqCst) as u128;
        let hold_id = state.next_transfer_id.fetch_add(1, Ordering::SeqCst) as u128;

        let cust_acc = AccountDirectory::customer(cust);
        let merch_acc = AccountDirectory::merchant(merch);

        let hold = Transfer::new_pending(
            TransferId::new(hold_id),
            cust_acc,
            merch_acc,
            Amount::new(amt),
            hold_ts,
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

        let mut payment = Payment::new_authorized(p_id, merch, cust, amt, fee, hold_id, hold_ts);

        if target_state == "captured" {
            let post_ts = state.next_timestamp();
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
                        TransferId::new(hold_id),
                        TransferId::new(post_id),
                        Amount::new(amt),
                        post_ts,
                    )
                    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            }
            payment
                .transition_to_captured(post_id, post_ts)
                .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
        } else if target_state == "voided" {
            let void_ts = state.next_timestamp();
            {
                let mut ledger = state.ledger.write().map_err(|_| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "lock poisoned".to_string(),
                    )
                })?;
                ledger
                    .void_pending(TransferId::new(hold_id), void_ts)
                    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            }
            payment
                .transition_to_voided(void_ts)
                .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
        }

        state
            .payments
            .write()
            .map_err(|_| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "lock poisoned".to_string(),
                )
            })?
            .insert(p_id, payment);

        seeded += 1;
    }

    Ok(Json(QuickSeedResponse {
        seeded_count: seeded,
        message: format!("Successfully seeded {seeded} sample payments."),
    }))
}

#[derive(Debug, Serialize)]
pub(crate) struct PaymentVerifyResponse {
    pub(crate) verified: bool,
    pub(crate) payment_id: u128,
    pub(crate) transfer_id: Option<u128>,
    pub(crate) amount: u128,
    pub(crate) batch_seq: Option<u64>,
    pub(crate) leaf_hash: Option<String>,
    pub(crate) merkle_root: Option<String>,
    pub(crate) proof_siblings: Vec<String>,
    pub(crate) solana_pda: String,
    pub(crate) message: String,
}

async fn handle_payment_verify(
    State(state): State<Arc<AppState>>,
    AxumPath(payment_id): AxumPath<u128>,
) -> Result<Json<PaymentVerifyResponse>, (StatusCode, String)> {
    let payment = {
        let payments = state.payments.read().map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "lock poisoned".to_string(),
            )
        })?;
        payments.get(&payment_id).cloned().ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                format!("Payment #{payment_id} not found"),
            )
        })?
    };

    let pda_str = state.solana_rail.pda_address().to_string();

    let posted_id = match payment.posted_transfer_id {
        Some(id) => id,
        None => {
            return Ok(Json(PaymentVerifyResponse {
                verified: false,
                payment_id,
                transfer_id: None,
                amount: payment.amount,
                batch_seq: None,
                leaf_hash: None,
                merkle_root: None,
                proof_siblings: Vec::new(),
                solana_pda: pda_str,
                message: format!(
                    "Payment is in '{:?}' state. Only 'Captured' payments are submitted to settlement rails.",
                    payment.state
                ),
            }));
        }
    };

    match state.solana_rail.receipt_for_transfer(posted_id) {
        Ok(receipt) => match receipt.verify() {
            Ok(()) => {
                state.metrics.merkle_proofs_verified.inc();
                let siblings = receipt
                    .proof
                    .siblings
                    .iter()
                    .map(|h| h.to_hex())
                    .collect();

                Ok(Json(PaymentVerifyResponse {
                    verified: true,
                    payment_id,
                    transfer_id: Some(posted_id),
                    amount: payment.amount,
                    batch_seq: Some(receipt.batch_seq),
                    leaf_hash: Some(receipt.leaf_hash),
                    merkle_root: Some(receipt.merkle_root),
                    proof_siblings: siblings,
                    solana_pda: pda_str,
                    message: "Cryptographic proof matches Solana PDA Merkle root. Transfer is immutable and mathematically proven."
                        .to_string(),
                }))
            }
            Err(e) => Ok(Json(PaymentVerifyResponse {
                verified: false,
                payment_id,
                transfer_id: Some(posted_id),
                amount: payment.amount,
                batch_seq: Some(receipt.batch_seq),
                leaf_hash: Some(receipt.leaf_hash),
                merkle_root: Some(receipt.merkle_root),
                proof_siblings: Vec::new(),
                solana_pda: pda_str,
                message: format!("Cryptographic proof verification failed: {e}"),
            })),
        },
        Err(_) => Ok(Json(PaymentVerifyResponse {
            verified: false,
            payment_id,
            transfer_id: Some(posted_id),
            amount: payment.amount,
            batch_seq: None,
            leaf_hash: None,
            merkle_root: state.solana_rail.latest_root_hex(),
            proof_siblings: Vec::new(),
            solana_pda: pda_str,
            message: "Transfer is posted in local double-entry journal, but has not yet been committed to a Solana settlement batch. Click 'Commit Settlement Batch' above first!".to_string(),
        })),
    }
}

/// Request payload to run deterministic simulation testing.
#[derive(Debug, Deserialize)]
pub(crate) struct SimulationRunRequest {
    pub(crate) scenario: String,
    pub(crate) seed: Option<u64>,
}

#[derive(Debug, Serialize)]
pub(crate) struct SimulationRunResponse {
    pub(crate) scenario: String,
    pub(crate) seed: u64,
    pub(crate) total_ticks: u64,
    pub(crate) ops_proposed: u64,
    pub(crate) ops_committed: u64,
    pub(crate) packets_delivered: u64,
    pub(crate) packets_dropped: u64,
    pub(crate) oracle_verdict: String,
    pub(crate) status: String,
}

async fn handle_simulator_run(
    Json(req): Json<SimulationRunRequest>,
) -> Result<Json<SimulationRunResponse>, (StatusCode, String)> {
    let scenario_type = match req.scenario.as_str() {
        "NetworkPartition" => simulator::ScenarioType::NetworkPartition,
        "CrashTornWrite" => simulator::ScenarioType::CrashTornWrite,
        "ChaosSoak" => simulator::ScenarioType::ChaosSoak,
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("Unknown scenario: {}", req.scenario),
            ))
        }
    };

    let seed = req.seed.unwrap_or(42);
    let ticks = 150;

    let report = simulator::ScenarioRunner::run(scenario_type, seed, ticks).map_err(|v| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Oracle invariant violation: {v}"),
        )
    })?;

    Ok(Json(SimulationRunResponse {
        scenario: req.scenario,
        seed: report.seed,
        total_ticks: report.total_ticks,
        ops_proposed: report.ops_proposed,
        ops_committed: report.ops_committed,
        packets_delivered: report.packets_delivered,
        packets_dropped: report.packets_dropped,
        oracle_verdict: "PASSED: Total wealth conservation, zero balance leakage, and state machine agreement verified across all simulated states.".to_string(),
        status: "PASSED".to_string(),
    }))
}

#[derive(Debug, Deserialize)]
pub(crate) struct VerifyRequest {
    pub(crate) receipt: TransferReceipt,
}

#[derive(Debug, Serialize)]
pub(crate) struct VerifyResponse {
    pub(crate) verified: bool,
    pub(crate) transfer_id: u128,
    pub(crate) batch_seq: u64,
    pub(crate) merkle_root: String,
    pub(crate) message: String,
}

async fn handle_verify_receipt(
    State(state): State<Arc<AppState>>,
    Json(req): Json<VerifyRequest>,
) -> Result<Json<VerifyResponse>, (StatusCode, String)> {
    match req.receipt.verify() {
        Ok(()) => {
            state.metrics.merkle_proofs_verified.inc();
            Ok(Json(VerifyResponse {
                verified: true,
                transfer_id: req.receipt.transfer_id,
                batch_seq: req.receipt.batch_seq,
                merkle_root: req.receipt.merkle_root,
                message: "Cryptographic proof matches on-chain Merkle root. Transfer is immutable."
                    .to_string(),
            }))
        }
        Err(e) => Err((
            StatusCode::BAD_REQUEST,
            format!("Proof verification failed: {e}"),
        )),
    }
}

async fn handle_metrics(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        state.metrics.render_prometheus(),
    )
}
