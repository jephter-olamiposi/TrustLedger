//! Central metrics registry formatting telemetry into Prometheus 2.0 exposition text.

use std::fmt::Write;
use std::sync::Arc;

use crate::counter::Counter;
use crate::gauge::Gauge;
use crate::histogram::Histogram;

/// Thread-safe central registry collecting active system metrics.
#[derive(Default, Clone)]
pub struct MetricsRegistry {
    counters: Vec<Arc<Counter>>,
    gauges: Vec<Arc<Gauge>>,
    histograms: Vec<Arc<Histogram>>,
}

impl MetricsRegistry {
    /// Creates an empty metrics registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a new counter metric with the registry.
    pub fn register_counter(&mut self, counter: Arc<Counter>) {
        self.counters.push(counter);
    }

    /// Registers a new gauge metric with the registry.
    pub fn register_gauge(&mut self, gauge: Arc<Gauge>) {
        self.gauges.push(gauge);
    }

    /// Registers a new histogram metric with the registry.
    pub fn register_histogram(&mut self, histogram: Arc<Histogram>) {
        self.histograms.push(histogram);
    }

    /// Formats all registered metrics into valid Prometheus 2.0 text exposition format.
    #[must_use]
    pub fn render_prometheus(&self) -> String {
        let mut out = String::with_capacity(4096);

        for counter in &self.counters {
            let _ = writeln!(out, "# HELP {} {}", counter.name(), counter.help());
            let _ = writeln!(out, "# TYPE {} counter", counter.name());
            let _ = writeln!(out, "{} {}", counter.name(), counter.get());
        }

        for gauge in &self.gauges {
            let _ = writeln!(out, "# HELP {} {}", gauge.name(), gauge.help());
            let _ = writeln!(out, "# TYPE {} gauge", gauge.name());
            let _ = writeln!(out, "{} {}", gauge.name(), gauge.get());
        }

        for hist in &self.histograms {
            let _ = writeln!(out, "# HELP {} {}", hist.name(), hist.help());
            let _ = writeln!(out, "# TYPE {} histogram", hist.name());

            for (bound, count) in hist.snapshot_buckets() {
                let _ = writeln!(out, "{}_bucket{{le=\"{bound}\"}} {count}", hist.name());
            }
            // Prometheus specification requires +Inf bucket matching total count.
            let _ = writeln!(
                out,
                "{}_bucket{{le=\"+Inf\"}} {}",
                hist.name(),
                hist.count()
            );
            let _ = writeln!(out, "{}_sum {}", hist.name(), hist.sum());
            let _ = writeln!(out, "{}_count {}", hist.name(), hist.count());
        }

        out
    }
}
