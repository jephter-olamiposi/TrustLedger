//! TrustLedger network ingress and micro-batching crate.
//!
//! Provides high-throughput gRPC endpoints, bounded non-blocking queues,
//! backpressure load-shedding, and single-writer group-committed batching
//! over write-ahead log persistence and double-entry ledger state.

#![deny(missing_docs)]
#![deny(unsafe_code)]

/// Generated Protocol Buffer definitions and gRPC service traits.
#[allow(missing_docs)]
#[allow(clippy::all)]
pub mod proto {
    tonic::include_proto!("ledger");
}

pub mod engine;
pub mod error;
pub mod queue;
pub mod server;

pub use engine::{Engine, EngineConfig};
pub use error::IngestError;
pub use queue::IngestQueue;
pub use server::LedgerServer;
