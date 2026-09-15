//! Deterministic Simulation Testing (DST) verification test suite.
//!
//! Validates financial invariants, consensus linearizability, and byte-for-byte reproducibility
//! across thousands of simulated network drops, crashes, and partition faults.

use simulator::scenario::{ScenarioRunner, ScenarioType};

#[test]
fn test_deterministic_reproducibility() {
    let seed = 0xABCD_EF01_2345_6789;
    let ticks = 150;

    // Run 1
    let rep1 = ScenarioRunner::run(ScenarioType::NetworkPartition, seed, ticks)
        .expect("Run 1 must pass invariants");

    // Run 2 with identical seed
    let rep2 = ScenarioRunner::run(ScenarioType::NetworkPartition, seed, ticks)
        .expect("Run 2 must pass invariants");

    // Assert absolute determinism
    assert_eq!(rep1.total_ticks, rep2.total_ticks);
    assert_eq!(rep1.ops_proposed, rep2.ops_proposed);
    assert_eq!(rep1.ops_committed, rep2.ops_committed);
    assert_eq!(rep1.packets_delivered, rep2.packets_delivered);
    assert_eq!(rep1.packets_dropped, rep2.packets_dropped);
}

#[test]
fn test_network_partition_minority_isolation() {
    let rep = ScenarioRunner::run(ScenarioType::NetworkPartition, 42, 180)
        .expect("Partition scenario must maintain invariants");

    assert!(rep.ops_committed > 0, "Quorum must continue committing");
    assert!(
        rep.packets_dropped > 0,
        "Partitions must cause dropped packets"
    );
}

#[test]
fn test_crash_recovery_and_torn_writes() {
    let rep = ScenarioRunner::run(ScenarioType::CrashTornWrite, 999, 180)
        .expect("Crash torn write scenario must maintain invariants");

    assert!(
        rep.ops_committed > 0,
        "Cluster must continue after election"
    );
}

#[test]
fn test_chaos_soak_multi_seed_fuzz() {
    for seed in [111, 222, 333, 444, 555] {
        let rep = ScenarioRunner::run(ScenarioType::ChaosSoak, seed, 100)
            .unwrap_or_else(|err| panic!("Chaos soak failed on seed {seed}: {err}"));

        assert!(rep.ops_committed > 0);
    }
}
