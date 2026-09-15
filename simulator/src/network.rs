//! Simulated discrete-event network transport with fault injection.
//!
//! Models packet delay jitter, random drops, duplication, out-of-order delivery,
//! asymmetric and symmetric network partitions, and node isolation.

use std::collections::{BTreeSet, BinaryHeap};

use crate::clock::{ScheduledEvent, SimInstant};
use crate::rng::SimRng;

/// A routable network packet traveling between cluster nodes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimPacket<T> {
    /// Originating node ID.
    pub from: u64,
    /// Destination node ID.
    pub to: u64,
    /// Packet message contents.
    pub payload: T,
}

/// Configuration tuning fault injection probabilities for the simulated network.
#[derive(Debug, Clone)]
pub struct NetworkConfig {
    /// Minimum transport delay in ticks.
    pub min_latency_ticks: u64,
    /// Maximum transport delay in ticks.
    pub max_latency_ticks: u64,
    /// Probability that a packet is dropped in transit `[0.0, 1.0]`.
    pub drop_rate: f64,
    /// Probability that a packet is duplicated in transit `[0.0, 1.0]`.
    pub duplicate_rate: f64,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            min_latency_ticks: 1,
            max_latency_ticks: 10,
            drop_rate: 0.0,
            duplicate_rate: 0.0,
        }
    }
}

/// Deterministic in-memory network router managing packet scheduling and partitions.
#[derive(Debug, Clone)]
pub struct SimNetwork<T> {
    config: NetworkConfig,
    queue: BinaryHeap<ScheduledEvent<SimPacket<T>>>,
    sequence_counter: u64,
    isolated: BTreeSet<u64>,
    partitions: Vec<(BTreeSet<u64>, BTreeSet<u64>)>,
    packets_sent: u64,
    packets_dropped: u64,
    packets_delivered: u64,
}

impl<T: Clone + Ord> SimNetwork<T> {
    /// Creates a new simulated network with the specified fault injection profile.
    #[must_use]
    pub fn new(config: NetworkConfig) -> Self {
        Self {
            config,
            queue: BinaryHeap::new(),
            sequence_counter: 0,
            isolated: BTreeSet::new(),
            partitions: Vec::new(),
            packets_sent: 0,
            packets_dropped: 0,
            packets_delivered: 0,
        }
    }

    /// Determines whether communication between `from` and `to` is currently allowed.
    #[must_use]
    pub fn can_communicate(&self, from: u64, to: u64) -> bool {
        if self.isolated.contains(&from) || self.isolated.contains(&to) {
            return false;
        }

        for (set_a, set_b) in &self.partitions {
            if (set_a.contains(&from) && set_b.contains(&to))
                || (set_b.contains(&from) && set_a.contains(&to))
            {
                return false;
            }
        }

        true
    }

    /// Dispatches a packet from sender to receiver across the simulated network.
    pub fn send(&mut self, from: u64, to: u64, payload: T, now: SimInstant, rng: &mut SimRng) {
        self.packets_sent = self.packets_sent.saturating_add(1);

        // Check topological partition reachability
        if !self.can_communicate(from, to) {
            self.packets_dropped = self.packets_dropped.saturating_add(1);
            return;
        }

        // Check randomized packet loss fault injection
        if rng.gen_bool(self.config.drop_rate) {
            self.packets_dropped = self.packets_dropped.saturating_add(1);
            return;
        }

        let delay = rng.gen_range(self.config.min_latency_ticks..=self.config.max_latency_ticks);
        let deliver_at = now.saturating_add(delay);

        self.sequence_counter = self.sequence_counter.saturating_add(1);
        self.queue.push(ScheduledEvent {
            execute_at: deliver_at,
            sequence: self.sequence_counter,
            payload: SimPacket {
                from,
                to,
                payload: payload.clone(),
            },
        });

        // Check randomized packet duplication fault injection
        if rng.gen_bool(self.config.duplicate_rate) {
            let dup_delay = delay.saturating_add(rng.gen_range(1..=3));
            self.sequence_counter = self.sequence_counter.saturating_add(1);
            self.queue.push(ScheduledEvent {
                execute_at: now.saturating_add(dup_delay),
                sequence: self.sequence_counter,
                payload: SimPacket { from, to, payload },
            });
        }
    }

    /// Drains and returns all packets scheduled for delivery at or before `now`.
    pub fn drain_ready(&mut self, now: SimInstant) -> Vec<SimPacket<T>> {
        let mut ready = Vec::new();

        while let Some(event) = self.queue.peek() {
            if event.execute_at.ticks() <= now.ticks() {
                let Some(event) = self.queue.pop() else {
                    break;
                };
                // Verify partition hasn't severed the link between scheduling and arrival
                if self.can_communicate(event.payload.from, event.payload.to) {
                    self.packets_delivered = self.packets_delivered.saturating_add(1);
                    ready.push(event.payload);
                } else {
                    self.packets_dropped = self.packets_dropped.saturating_add(1);
                }
            } else {
                break;
            }
        }

        ready
    }

    /// Returns the timestamp of the earliest pending packet in the queue, if any.
    #[must_use]
    pub fn next_event_time(&self) -> Option<SimInstant> {
        self.queue.peek().map(|e| e.execute_at)
    }

    /// Isolates a node completely from all cluster traffic.
    pub fn isolate(&mut self, node_id: u64) {
        self.isolated.insert(node_id);
    }

    /// Reconnects a previously isolated node to the network.
    pub fn unisolate(&mut self, node_id: u64) {
        self.isolated.remove(&node_id);
    }

    /// Introduces a network partition severing all communication between two sets of nodes.
    pub fn partition(&mut self, set_a: BTreeSet<u64>, set_b: BTreeSet<u64>) {
        self.partitions.push((set_a, set_b));
    }

    /// Heals all active network partitions and node isolations.
    pub fn heal(&mut self) {
        self.isolated.clear();
        self.partitions.clear();
    }

    /// Returns total packets successfully delivered.
    #[must_use]
    pub const fn packets_delivered(&self) -> u64 {
        self.packets_delivered
    }

    /// Returns total packets dropped due to partitions or fault injection.
    #[must_use]
    pub const fn packets_dropped(&self) -> u64 {
        self.packets_dropped
    }
}
