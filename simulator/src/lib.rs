//! Deterministic simulation harness for consensus, storage faults, and ledger invariants.
//!
//! The simulator provides a virtual clock, virtual network, fault-injected storage, and
//! workload generators so cluster failures and financial invariants can be tested under
//! reproducible conditions.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

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
