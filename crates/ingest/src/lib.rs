//! Ingestion service for TrustLedger traffic and micro-batched writes.
//!
//! The crate accepts bounded gRPC input, applies queue backpressure, and
//! persists committed batches into the ledger.

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
