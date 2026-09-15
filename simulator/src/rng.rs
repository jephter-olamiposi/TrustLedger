//! Seeded pseudo-random number generator for 100% deterministic reproducibility.
//!
//! Every simulation run is parameterized by an immutable 64-bit seed. Given the same seed,
//! every decision, delay, network drop, and crash schedule executes identically.

use rand::seq::SliceRandom;
use rand::Rng;
use rand_chacha::rand_core::SeedableRng;
use rand_chacha::ChaCha8Rng;

/// Deterministic pseudo-random number generator wrapping ChaCha8.
#[derive(Debug, Clone)]
pub struct SimRng {
    seed: u64,
    inner: ChaCha8Rng,
}

impl SimRng {
    /// Creates a new deterministic RNG initialized with the given 64-bit seed.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            inner: ChaCha8Rng::seed_from_u64(seed),
        }
    }

    /// Returns the initial seed that produced this PRNG stream.
    #[must_use]
    pub const fn seed(&self) -> u64 {
        self.seed
    }

    /// Generates a random integer within the given range.
    pub fn gen_range<R: rand::distributions::uniform::SampleRange<u64>>(
        &mut self,
        range: R,
    ) -> u64 {
        self.inner.gen_range(range)
    }

    /// Generates a boolean with the specified probability of being `true`.
    pub fn gen_bool(&mut self, probability: f64) -> bool {
        let p = probability.clamp(0.0, 1.0);
        self.inner.gen_bool(p)
    }

    /// Generates a random 64-bit unsigned integer.
    pub fn gen_u64(&mut self) -> u64 {
        self.inner.gen()
    }

    /// Randomly selects a reference to an element from a slice, or `None` if empty.
    pub fn choose<'a, T>(&mut self, slice: &'a [T]) -> Option<&'a T> {
        slice.choose(&mut self.inner)
    }

    /// Shuffles a slice in-place using deterministic Fisher-Yates.
    pub fn shuffle<T>(&mut self, slice: &mut [T]) {
        slice.shuffle(&mut self.inner);
    }
}
