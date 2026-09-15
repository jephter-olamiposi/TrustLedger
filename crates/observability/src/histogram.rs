//! Fixed-bucket histogram metric for latency and size distributions.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Standard latency buckets in seconds spanning 100 microseconds to 5 seconds.
pub const DEFAULT_LATENCY_BUCKETS: &[f64] = &[
    0.0001, 0.0005, 0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0,
];

/// Standard batch size buckets for ingress and settlement batches.
pub const DEFAULT_BATCH_BUCKETS: &[f64] =
    &[1.0, 5.0, 10.0, 25.0, 50.0, 100.0, 256.0, 512.0, 1024.0];

/// Thread-safe cumulative histogram measuring observation distributions across configured upper bounds.
#[derive(Debug)]
pub struct Histogram {
    name: &'static str,
    help: &'static str,
    buckets: Vec<f64>,
    bucket_counts: Vec<AtomicU64>,
    count: AtomicU64,
    sum_bits: AtomicU64,
}

impl Histogram {
    /// Creates a new histogram with metric name, documentation, and upper bound thresholds.
    #[must_use]
    pub fn new(name: &'static str, help: &'static str, buckets: &[f64]) -> Self {
        let mut sorted = buckets.to_vec();
        // Invariant: Prometheus requires cumulative buckets to be sorted in strictly ascending order.
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        let bucket_counts = (0..sorted.len()).map(|_| AtomicU64::new(0)).collect();

        Self {
            name,
            help,
            buckets: sorted,
            bucket_counts,
            count: AtomicU64::new(0),
            sum_bits: AtomicU64::new(0.0_f64.to_bits()),
        }
    }

    /// Records an observation value into the histogram.
    pub fn observe(&self, val: f64) {
        if val.is_nan() || val.is_sign_negative() {
            return;
        }

        self.count.fetch_add(1, Ordering::Relaxed);

        let mut current = self.sum_bits.load(Ordering::Relaxed);
        loop {
            let current_float = f64::from_bits(current);
            let new_float = current_float + val;
            match self.sum_bits.compare_exchange_weak(
                current,
                new_float.to_bits(),
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }

        for (i, &upper_bound) in self.buckets.iter().enumerate() {
            if val <= upper_bound {
                self.bucket_counts[i].fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Records a duration observation in seconds.
    pub fn observe_duration(&self, duration: Duration) {
        self.observe(duration.as_secs_f64());
    }

    /// Returns the total observation count.
    #[must_use]
    pub fn count(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }

    /// Returns the sum of all observed values.
    #[must_use]
    pub fn sum(&self) -> f64 {
        f64::from_bits(self.sum_bits.load(Ordering::Relaxed))
    }

    /// Returns snapshot of bucket upper bounds and cumulative counts.
    #[must_use]
    pub fn snapshot_buckets(&self) -> Vec<(f64, u64)> {
        self.buckets
            .iter()
            .zip(&self.bucket_counts)
            .map(|(&bound, count)| (bound, count.load(Ordering::Relaxed)))
            .collect()
    }

    /// Returns the metric identifier name.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        self.name
    }

    /// Returns the human-readable description string.
    #[must_use]
    pub const fn help(&self) -> &'static str {
        self.help
    }
}
