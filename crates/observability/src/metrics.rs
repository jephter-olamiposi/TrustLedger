//! Pre-defined operational telemetry metrics for TrustLedger services.

use std::sync::Arc;

use crate::counter::Counter;
use crate::gauge::Gauge;
use crate::histogram::{Histogram, DEFAULT_BATCH_BUCKETS, DEFAULT_LATENCY_BUCKETS};
use crate::registry::MetricsRegistry;

/// Unified operational metrics collection spanning all TrustLedger subsystems.
#[derive(Clone)]
pub struct LedgerMetrics {
    /// Total transfer commands received by the ingress gateway.
    pub transfers_received: Arc<Counter>,
    /// Total transfer commands committed durably to the ledger.
    pub transfers_committed: Arc<Counter>,
    /// Total transfer commands rejected due to validation or invariant violations.
    pub transfers_rejected: Arc<Counter>,
    /// Ingress batch size distribution.
    pub batch_size: Arc<Histogram>,
    /// Micro-batch processing duration in seconds.
    pub batch_duration_seconds: Arc<Histogram>,

    /// Write-ahead log fsync duration in seconds.
    pub wal_sync_duration_seconds: Arc<Histogram>,
    /// Total records appended to the durable WAL.
    pub wal_records_appended: Arc<Counter>,
    /// Total raw bytes written to the durable WAL.
    pub wal_bytes_written: Arc<Counter>,

    /// Current Raft consensus term.
    pub raft_term: Arc<Gauge>,
    /// Indicator whether current node is the consensus leader (1 = leader, 0 = follower/candidate).
    pub raft_is_leader: Arc<Gauge>,
    /// Total consensus proposals submitted.
    pub raft_proposals_total: Arc<Counter>,

    /// Total settlement batches anchored on-chain.
    pub settlement_batches_committed: Arc<Counter>,
    /// Cumulative volume settled across payment rails.
    pub settlement_amount_total: Arc<Counter>,
    /// Cryptographic Merkle inclusion proofs verified.
    pub merkle_proofs_verified: Arc<Counter>,

    /// Total reconciliation audit cycles executed.
    pub reconciliation_audits_total: Arc<Counter>,
    /// Real-time balance drift between application and ledger state ($Drift \equiv 0$).
    pub reconciliation_drift_gauge: Arc<Gauge>,
    /// Total reconciliation incidents detected requiring operational triage.
    pub reconciliation_incidents_total: Arc<Counter>,

    /// Underlying Prometheus registry.
    pub registry: MetricsRegistry,
}

impl Default for LedgerMetrics {
    fn default() -> Self {
        Self::new()
    }
}

impl LedgerMetrics {
    /// Initializes and registers all production metrics into a unified collection.
    #[must_use]
    pub fn new() -> Self {
        let transfers_received = Arc::new(Counter::new(
            "trustledger_transfers_received_total",
            "Total number of transfer commands received at ingress",
        ));
        let transfers_committed = Arc::new(Counter::new(
            "trustledger_transfers_committed_total",
            "Total number of transfers successfully committed to ledger state",
        ));
        let transfers_rejected = Arc::new(Counter::new(
            "trustledger_transfers_rejected_total",
            "Total number of transfers rejected due to validation errors or backpressure",
        ));

        let batch_size = Arc::new(Histogram::new(
            "trustledger_ingress_batch_size",
            "Histogram of ingress micro-batch sizes",
            DEFAULT_BATCH_BUCKETS,
        ));
        let batch_duration_seconds = Arc::new(Histogram::new(
            "trustledger_ingress_batch_duration_seconds",
            "Duration of batch processing and state application in seconds",
            DEFAULT_LATENCY_BUCKETS,
        ));

        let wal_sync_duration_seconds = Arc::new(Histogram::new(
            "trustledger_wal_sync_duration_seconds",
            "Duration of physical storage fsync sync_data operations in seconds",
            DEFAULT_LATENCY_BUCKETS,
        ));
        let wal_records_appended = Arc::new(Counter::new(
            "trustledger_wal_records_appended_total",
            "Total number of records durably appended to the WAL",
        ));
        let wal_bytes_written = Arc::new(Counter::new(
            "trustledger_wal_bytes_written_total",
            "Total raw bytes appended to durable storage",
        ));

        let raft_term = Arc::new(Gauge::new(
            "trustledger_consensus_term",
            "Current Raft consensus election term",
        ));
        let raft_is_leader = Arc::new(Gauge::new(
            "trustledger_consensus_is_leader",
            "Consensus leadership status (1 for leader, 0 for follower)",
        ));
        let raft_proposals_total = Arc::new(Counter::new(
            "trustledger_consensus_proposals_total",
            "Total consensus state proposals submitted",
        ));

        let settlement_batches_committed = Arc::new(Counter::new(
            "trustledger_settlement_batches_committed_total",
            "Total settlement batches committed to the Solana on-chain PDA",
        ));
        let settlement_amount_total = Arc::new(Counter::new(
            "trustledger_settlement_amount_total",
            "Cumulative settlement volume anchored cryptographically on-chain",
        ));
        let merkle_proofs_verified = Arc::new(Counter::new(
            "trustledger_merkle_proofs_verified_total",
            "Total cryptographic Merkle inclusion proofs verified against PDA roots",
        ));

        let reconciliation_audits_total = Arc::new(Counter::new(
            "trustledger_reconciliation_audits_total",
            "Total 3-way continuous reconciliation audit runs",
        ));
        let reconciliation_drift_gauge = Arc::new(Gauge::new(
            "trustledger_reconciliation_drift_gauge",
            "Current measured financial balance drift across ledger, payments, and chain",
        ));
        let reconciliation_incidents_total = Arc::new(Counter::new(
            "trustledger_reconciliation_incidents_total",
            "Total financial divergence incidents flagged",
        ));

        let mut registry = MetricsRegistry::new();
        registry.register_counter(Arc::clone(&transfers_received));
        registry.register_counter(Arc::clone(&transfers_committed));
        registry.register_counter(Arc::clone(&transfers_rejected));
        registry.register_histogram(Arc::clone(&batch_size));
        registry.register_histogram(Arc::clone(&batch_duration_seconds));

        registry.register_histogram(Arc::clone(&wal_sync_duration_seconds));
        registry.register_counter(Arc::clone(&wal_records_appended));
        registry.register_counter(Arc::clone(&wal_bytes_written));

        registry.register_gauge(Arc::clone(&raft_term));
        registry.register_gauge(Arc::clone(&raft_is_leader));
        registry.register_counter(Arc::clone(&raft_proposals_total));

        registry.register_counter(Arc::clone(&settlement_batches_committed));
        registry.register_counter(Arc::clone(&settlement_amount_total));
        registry.register_counter(Arc::clone(&merkle_proofs_verified));

        registry.register_counter(Arc::clone(&reconciliation_audits_total));
        registry.register_gauge(Arc::clone(&reconciliation_drift_gauge));
        registry.register_counter(Arc::clone(&reconciliation_incidents_total));

        Self {
            transfers_received,
            transfers_committed,
            transfers_rejected,
            batch_size,
            batch_duration_seconds,
            wal_sync_duration_seconds,
            wal_records_appended,
            wal_bytes_written,
            raft_term,
            raft_is_leader,
            raft_proposals_total,
            settlement_batches_committed,
            settlement_amount_total,
            merkle_proofs_verified,
            reconciliation_audits_total,
            reconciliation_drift_gauge,
            reconciliation_incidents_total,
            registry,
        }
    }

    /// Renders current telemetry metrics in Prometheus text exposition format.
    #[must_use]
    pub fn render_prometheus(&self) -> String {
        self.registry.render_prometheus()
    }
}
