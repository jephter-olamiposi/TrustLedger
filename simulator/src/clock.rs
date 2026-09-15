//! Discrete virtual time scheduler for deterministic simulation.
//!
//! Unlike wall-clock timers, virtual time advances deterministically in discrete ticks,
//! enabling fully reproducible execution traces independent of host CPU speed or OS scheduling.

use std::cmp::Ordering;
use std::fmt;

/// A discrete virtual instant in simulated time measured in integer ticks (milliseconds).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct SimInstant(u64);

impl SimInstant {
    /// Creates a simulated instant at tick zero.
    #[must_use]
    pub const fn zero() -> Self {
        Self(0)
    }

    /// Creates a simulated instant from raw tick count.
    #[must_use]
    pub const fn from_ticks(ticks: u64) -> Self {
        Self(ticks)
    }

    /// Returns the raw tick value.
    #[must_use]
    pub const fn ticks(self) -> u64 {
        self.0
    }

    /// Returns a new instant advanced by the specified number of ticks.
    #[must_use]
    pub const fn saturating_add(self, ticks: u64) -> Self {
        Self(self.0.saturating_add(ticks))
    }

    /// Computes the tick duration between this instant and an earlier instant.
    #[must_use]
    pub const fn saturating_sub(self, earlier: Self) -> u64 {
        self.0.saturating_sub(earlier.0)
    }
}

impl fmt::Debug for SimInstant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Tick({})", self.0)
    }
}

impl fmt::Display for SimInstant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ticks", self.0)
    }
}

/// Simulated clock managing monotonic discrete time progression.
#[derive(Debug, Clone, Default)]
pub struct SimClock {
    current_tick: u64,
}

impl SimClock {
    /// Creates a new simulated clock initialized at tick zero.
    #[must_use]
    pub const fn new() -> Self {
        Self { current_tick: 0 }
    }

    /// Returns the current simulated instant.
    #[must_use]
    pub const fn now(&self) -> SimInstant {
        SimInstant(self.current_tick)
    }

    /// Advances the simulated clock forward by `ticks`.
    pub fn advance(&mut self, ticks: u64) {
        self.current_tick = self.current_tick.saturating_add(ticks);
    }

    /// Advances the simulated clock directly to a target instant if it is in the future.
    pub fn advance_to(&mut self, target: SimInstant) {
        if target.0 > self.current_tick {
            self.current_tick = target.0;
        }
    }
}

/// A timed simulation event scheduled for future discrete execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledEvent<T> {
    /// The instant at which this event is scheduled to execute.
    pub execute_at: SimInstant,
    /// Monotonic tie-breaker counter to maintain deterministic FIFO ordering for events at the same tick.
    pub sequence: u64,
    /// Event payload.
    pub payload: T,
}

impl<T: Eq> Ord for ScheduledEvent<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reverse ordering so BinaryHeap acts as a min-heap by execute_at, then FIFO by sequence.
        other
            .execute_at
            .cmp(&self.execute_at)
            .then_with(|| other.sequence.cmp(&self.sequence))
    }
}

impl<T: Eq> PartialOrd for ScheduledEvent<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
