//! FoundationDB/TigerBeetle-style Deterministic Simulation Testing (DST) for TrustLedger.
//!
//! Provides a fully virtualized, discrete-event simulated environment including:
//! - Deterministic pseudo-random number generator (`SimRng`).
//! - Discrete virtual time scheduler (`SimClock`, `SimInstant`).
//! - Virtual network with packet delays, drops, duplicates, and partitions (`SimNetwork`).
//! - Virtual storage with un-fsynced write loss and torn-write fault injection (`SimDisk`).
//! - Multi-node consensus state machine cluster (`SimCluster`).
//! - Synthetic double-entry payment workload generator (`WorkloadGenerator`).
//! - Invariant checking oracle enforcing total wealth conservation and log agreement (`Oracle`).

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod clock;
pub mod cluster;
pub mod network;
pub mod oracle;
pub mod rng;
pub mod scenario;
pub mod storage;
pub mod workload;

pub use clock::{SimClock, SimInstant};
pub use cluster::{ClusterMessage, NodeStatus, SimCluster, SimNode};
pub use network::{NetworkConfig, SimNetwork, SimPacket};
pub use oracle::{Oracle, OracleViolation};
pub use rng::SimRng;
pub use scenario::{ScenarioReport, ScenarioRunner, ScenarioType};
pub use storage::SimDisk;
pub use workload::{WorkloadGenerator, WorkloadOp};
