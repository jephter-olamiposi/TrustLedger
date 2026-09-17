//! Observability and Prometheus metrics for TrustLedger.
//!
//! The crate exposes counters, gauges, histograms, and a registry that renders
//! Prometheus text format without external runtime dependencies.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod counter;
pub mod gauge;
pub mod histogram;
pub mod metrics;
pub mod registry;

pub use counter::Counter;
pub use gauge::Gauge;
pub use histogram::{Histogram, DEFAULT_BATCH_BUCKETS, DEFAULT_LATENCY_BUCKETS};
pub use metrics::LedgerMetrics;
pub use registry::MetricsRegistry;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_counter_increments() {
        let counter = Counter::new("test_counter", "A test counter");
        assert_eq!(counter.get(), 0);
        counter.inc();
        assert_eq!(counter.get(), 1);
        counter.inc_by(41);
        assert_eq!(counter.get(), 42);
    }

    #[test]
    fn test_gauge_operations() {
        let gauge = Gauge::new("test_gauge", "A test gauge");
        assert_eq!(gauge.get(), 0);
        gauge.set(100);
        assert_eq!(gauge.get(), 100);
        gauge.inc();
        assert_eq!(gauge.get(), 101);
        gauge.sub(51);
        assert_eq!(gauge.get(), 50);
        gauge.dec();
        assert_eq!(gauge.get(), 49);
    }

    #[test]
    fn test_histogram_observations() {
        let hist = Histogram::new("test_hist", "A test histogram", &[0.01, 0.1, 1.0]);
        hist.observe(0.005);
        hist.observe(0.05);
        hist.observe(0.5);
        hist.observe(5.0);

        assert_eq!(hist.count(), 4);
        assert!((hist.sum() - 5.555).abs() < 1e-4);

        let buckets = hist.snapshot_buckets();
        assert_eq!(buckets[0], (0.01, 1));
        assert_eq!(buckets[1], (0.1, 2));
        assert_eq!(buckets[2], (1.0, 3));
    }

    #[test]
    fn test_prometheus_exposition_format() {
        let metrics = LedgerMetrics::new();
        metrics.transfers_received.inc_by(5);
        metrics.transfers_committed.inc_by(4);
        metrics.transfers_rejected.inc();
        metrics.batch_size.observe(256.0);
        metrics.reconciliation_drift_gauge.set(0);

        let rendered = metrics.render_prometheus();
        assert!(rendered.contains("# TYPE trustledger_transfers_received_total counter"));
        assert!(rendered.contains("trustledger_transfers_received_total 5"));
        assert!(rendered.contains("# TYPE trustledger_reconciliation_drift_gauge gauge"));
        assert!(rendered.contains("trustledger_reconciliation_drift_gauge 0"));
        assert!(rendered.contains("trustledger_ingress_batch_size_bucket{le=\"256\"} 1"));
        assert!(rendered.contains("trustledger_ingress_batch_size_bucket{le=\"+Inf\"} 1"));
    }
}
