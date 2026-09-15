//! Monotonically increasing atomic counter metric.

use std::sync::atomic::{AtomicU64, Ordering};

/// A thread-safe, lock-free 64-bit monotonically increasing counter.
#[derive(Debug)]
pub struct Counter {
    name: &'static str,
    help: &'static str,
    value: AtomicU64,
}

impl Counter {
    /// Creates a new counter with the given metric name and documentation string.
    #[must_use]
    pub const fn new(name: &'static str, help: &'static str) -> Self {
        Self {
            name,
            help,
            value: AtomicU64::new(0),
        }
    }

    /// Increments the counter by 1.
    pub fn inc(&self) {
        self.inc_by(1);
    }

    /// Increments the counter by an arbitrary positive amount.
    pub fn inc_by(&self, val: u64) {
        // Relaxed ordering suffices because counter telemetry does not synchronize memory.
        self.value.fetch_add(val, Ordering::Relaxed);
    }

    /// Returns the current value of the counter.
    #[must_use]
    pub fn get(&self) -> u64 {
        self.value.load(Ordering::Relaxed)
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
