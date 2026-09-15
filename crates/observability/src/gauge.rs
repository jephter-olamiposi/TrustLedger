//! Atomic gauge metric representing a variable value that can increase or decrease.

use std::sync::atomic::{AtomicI64, Ordering};

/// A thread-safe, lock-free 64-bit signed integer gauge.
#[derive(Debug)]
pub struct Gauge {
    name: &'static str,
    help: &'static str,
    value: AtomicI64,
}

impl Gauge {
    /// Creates a new gauge with the given metric name and documentation string.
    #[must_use]
    pub const fn new(name: &'static str, help: &'static str) -> Self {
        Self {
            name,
            help,
            value: AtomicI64::new(0),
        }
    }

    /// Sets the gauge to a specific signed value.
    pub fn set(&self, val: i64) {
        self.value.store(val, Ordering::Relaxed);
    }

    /// Increments the gauge by 1.
    pub fn inc(&self) {
        self.add(1);
    }

    /// Decrements the gauge by 1.
    pub fn dec(&self) {
        self.sub(1);
    }

    /// Adds a signed delta to the gauge.
    pub fn add(&self, delta: i64) {
        self.value.fetch_add(delta, Ordering::Relaxed);
    }

    /// Subtracts a signed delta from the gauge.
    pub fn sub(&self, delta: i64) {
        self.value.fetch_sub(delta, Ordering::Relaxed);
    }

    /// Returns the current value of the gauge.
    #[must_use]
    pub fn get(&self) -> i64 {
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
