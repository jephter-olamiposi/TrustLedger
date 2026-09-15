//! Pre-packaged deterministic test scenarios exercising consensus, partitions, and storage faults.

use std::collections::BTreeSet;

use crate::clock::SimClock;
use crate::cluster::SimCluster;
use crate::network::NetworkConfig;
use crate::oracle::OracleViolation;
use crate::rng::SimRng;
use crate::workload::WorkloadGenerator;

/// Named deterministic simulation scenario.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScenarioType {
    /// Jepsen-style network partition isolating a minority node while majority continues.
    NetworkPartition,
    /// Leader crash with torn write fault injection followed by election and recovery.
    CrashTornWrite,
    /// High-entropy chaos soak testing packet loss, latency jitter, and concurrent partitions.
    ChaosSoak,
}

/// Execution summary returned upon successful scenario completion.
#[derive(Debug, Clone)]
pub struct ScenarioReport {
    /// Scenario name.
    pub scenario: ScenarioType,
    /// Deterministic seed utilized.
    pub seed: u64,
    /// Total discrete virtual ticks simulated.
    pub total_ticks: u64,
    /// Operations proposed during the run.
    pub ops_proposed: u64,
    /// Operations committed across quorum.
    pub ops_committed: u64,
    /// Packets delivered across simulated network.
    pub packets_delivered: u64,
    /// Packets dropped due to partitions or fault injection.
    pub packets_dropped: u64,
}

/// Scenario runner executing deterministic simulation schedules.
pub struct ScenarioRunner;

impl ScenarioRunner {
    /// Executes the specified scenario with a deterministic seed.
    ///
    /// # Errors
    ///
    /// Returns [`OracleViolation`] if financial conservation or log agreement fails.
    pub fn run(
        scenario: ScenarioType,
        seed: u64,
        max_ticks: u64,
    ) -> Result<ScenarioReport, OracleViolation> {
        match scenario {
            ScenarioType::NetworkPartition => Self::run_partition(seed, max_ticks),
            ScenarioType::CrashTornWrite => Self::run_crash_torn_write(seed, max_ticks),
            ScenarioType::ChaosSoak => Self::run_chaos_soak(seed, max_ticks),
        }
    }

    fn run_partition(seed: u64, max_ticks: u64) -> Result<ScenarioReport, OracleViolation> {
        let mut rng = SimRng::new(seed);
        let mut clock = SimClock::new();

        let net_config = NetworkConfig {
            min_latency_ticks: 1,
            max_latency_ticks: 5,
            drop_rate: 0.0,
            duplicate_rate: 0.0,
        };

        let account_ids = vec![101, 102, 103, 104];
        let mut cluster = SimCluster::new_3_node(net_config, &account_ids, 100_000);
        let mut workload = WorkloadGenerator::new(account_ids, 10_000);

        let mut ops_proposed = 0;
        let partition_start = 30;
        let partition_heal = 100;

        for tick in 1..=max_ticks {
            clock.advance(1);
            let now = clock.now();

            // Inject fault: Partition minority node 3 away from majority {1, 2}
            if tick == partition_start {
                let mut majority = BTreeSet::new();
                majority.insert(1);
                majority.insert(2);
                let mut minority = BTreeSet::new();
                minority.insert(3);
                cluster.network.partition(majority, minority);
            }

            // Heal fault: Restore full network connectivity
            if tick == partition_heal {
                cluster.network.heal();
                // Minority requests catchup
                cluster.reboot_node(3, now, &mut rng);
            }

            // Generate transactions regularly
            if tick % 3 == 0 && tick < max_ticks.saturating_sub(20) {
                let op = workload.next_op(&mut rng);
                if cluster.propose(op, now, &mut rng) {
                    ops_proposed += 1;
                }
            }

            cluster.step(now, &mut rng);
        }

        // Quiesce: Drain remaining inflight packets
        for _ in 0..20 {
            clock.advance(1);
            cluster.step(clock.now(), &mut rng);
        }

        // Verify all invariants
        cluster.verify_invariants()?;

        let ops_committed = cluster
            .nodes
            .get(&1)
            .map(|n| n.committed_ops.len() as u64)
            .unwrap_or(0);

        Ok(ScenarioReport {
            scenario: ScenarioType::NetworkPartition,
            seed,
            total_ticks: clock.now().ticks(),
            ops_proposed,
            ops_committed,
            packets_delivered: cluster.network.packets_delivered(),
            packets_dropped: cluster.network.packets_dropped(),
        })
    }

    fn run_crash_torn_write(seed: u64, max_ticks: u64) -> Result<ScenarioReport, OracleViolation> {
        let mut rng = SimRng::new(seed);
        let mut clock = SimClock::new();

        let net_config = NetworkConfig {
            min_latency_ticks: 1,
            max_latency_ticks: 4,
            drop_rate: 0.0,
            duplicate_rate: 0.0,
        };

        let account_ids = vec![201, 202, 203, 204];
        let mut cluster = SimCluster::new_3_node(net_config, &account_ids, 200_000);
        let mut workload = WorkloadGenerator::new(account_ids, 20_000);

        let mut ops_proposed = 0;
        let crash_tick = 40;
        let reboot_tick = 90;

        for tick in 1..=max_ticks {
            clock.advance(1);
            let now = clock.now();

            // Crash leader (node 1) with torn-write fault injection
            if tick == crash_tick {
                cluster.crash_node(1, true, &mut rng);
                // Trigger election on node 2 to maintain forward progress
                cluster.trigger_election(2, now, &mut rng);
            }

            // Reboot node 1 and trigger catch-up synchronization
            if tick == reboot_tick {
                cluster.reboot_node(1, now, &mut rng);
            }

            if tick % 3 == 0 && tick < max_ticks.saturating_sub(20) {
                let op = workload.next_op(&mut rng);
                if cluster.propose(op, now, &mut rng) {
                    ops_proposed += 1;
                }
            }

            cluster.step(now, &mut rng);
        }

        // Quiesce: Drain remaining inflight packets
        for _ in 0..25 {
            clock.advance(1);
            cluster.step(clock.now(), &mut rng);
        }

        // Verify all invariants
        cluster.verify_invariants()?;

        let ops_committed = cluster
            .nodes
            .get(&2)
            .map(|n| n.committed_ops.len() as u64)
            .unwrap_or(0);

        Ok(ScenarioReport {
            scenario: ScenarioType::CrashTornWrite,
            seed,
            total_ticks: clock.now().ticks(),
            ops_proposed,
            ops_committed,
            packets_delivered: cluster.network.packets_delivered(),
            packets_dropped: cluster.network.packets_dropped(),
        })
    }

    fn run_chaos_soak(seed: u64, max_ticks: u64) -> Result<ScenarioReport, OracleViolation> {
        let mut rng = SimRng::new(seed);
        let mut clock = SimClock::new();

        let net_config = NetworkConfig {
            min_latency_ticks: 1,
            max_latency_ticks: 8,
            drop_rate: 0.05,      // 5% randomized packet loss
            duplicate_rate: 0.02, // 2% packet duplication
        };

        let account_ids = vec![301, 302, 303, 304, 305];
        let mut cluster = SimCluster::new_3_node(net_config, &account_ids, 500_000);
        let mut workload = WorkloadGenerator::new(account_ids, 30_000);

        let mut ops_proposed = 0;

        for tick in 1..=max_ticks {
            clock.advance(1);
            let now = clock.now();

            // Periodic chaos injection
            if tick % 50 == 0 {
                let victim = rng.gen_range(1..=3);
                cluster.crash_node(victim, true, &mut rng);
            } else if tick % 50 == 25 {
                // Reboot any crashed node
                for id in 1..=3 {
                    if cluster.nodes.get(&id).map(|n| n.status)
                        == Some(crate::cluster::NodeStatus::Crashed)
                    {
                        cluster.reboot_node(id, now, &mut rng);
                    }
                }
            }

            if tick % 2 == 0 && tick < max_ticks.saturating_sub(40) {
                let op = workload.next_op(&mut rng);
                if cluster.propose(op, now, &mut rng) {
                    ops_proposed += 1;
                }
            }

            cluster.step(now, &mut rng);
        }

        // Final healing phase
        cluster.network.heal();
        for id in 1..=3 {
            cluster.reboot_node(id, clock.now(), &mut rng);
        }

        // Quiesce: Drain remaining inflight packets
        for _ in 0..50 {
            clock.advance(1);
            cluster.step(clock.now(), &mut rng);
        }

        // Verify all invariants
        cluster.verify_invariants()?;

        let ops_committed = cluster
            .nodes
            .values()
            .map(|n| n.committed_ops.len() as u64)
            .max()
            .unwrap_or(0);

        Ok(ScenarioReport {
            scenario: ScenarioType::ChaosSoak,
            seed,
            total_ticks: clock.now().ticks(),
            ops_proposed,
            ops_committed,
            packets_delivered: cluster.network.packets_delivered(),
            packets_dropped: cluster.network.packets_dropped(),
        })
    }
}
