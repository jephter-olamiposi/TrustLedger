//! Invariant oracles verifying financial correctness and consensus linearizability.
//!
//! An oracle inspects the state of all cluster nodes at tick boundaries and at simulation
//! completion, asserting fundamental double-entry and distributed systems invariants.

use ledger_core::id::AccountId;
use ledger_core::journal::LedgerEvent;
use ledger_core::Ledger;
use thiserror::Error;

/// Violations detected by the simulation invariant oracle.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum OracleViolation {
    /// Total money in the ledger deviated from the immutable initial supply.
    #[error("Wealth conservation violated! Expected total {expected}, found {actual} (drift = {drift}) on node {node_id}")]
    WealthLeak {
        /// Cluster node where violation was discovered.
        node_id: u64,
        /// Expected constant initial monetary volume.
        expected: u128,
        /// Measured aggregate volume.
        actual: u128,
        /// Measured drift difference.
        drift: i128,
    },
    /// An account experienced an illegal negative balance.
    #[error("Negative balance detected on node {node_id} for account {account_id:?}")]
    NegativeBalance {
        /// Cluster node where violation was discovered.
        node_id: u64,
        /// Faulty account identifier.
        account_id: AccountId,
    },
    /// Conflicting state machine log entries at the same sequence index across consensus nodes.
    #[error("Split-brain consensus divergence! Node {node_a} and Node {node_b} disagree at index {index}")]
    SplitBrain {
        /// First disagreeing node.
        node_a: u64,
        /// Second disagreeing node.
        node_b: u64,
        /// Consensus log index where divergence occurred.
        index: usize,
    },
}

/// Invariant checking oracle for simulation runs.
pub struct Oracle;

impl Oracle {
    /// Asserts that total wealth is strictly conserved across all accounts on every node.
    ///
    /// # Errors
    ///
    /// Returns [`OracleViolation::WealthLeak`] if sum of balances != initial supply, or
    /// [`OracleViolation::NegativeBalance`] if any account violates balance bounds.
    pub fn assert_conservation(
        node_id: u64,
        ledger: &Ledger,
        accounts: &[AccountId],
        expected_supply: u128,
    ) -> Result<(), OracleViolation> {
        // First assert underlying double-entry structural conservation
        if let Err(_e) = ledger.verify_invariants() {
            return Err(OracleViolation::WealthLeak {
                node_id,
                expected: expected_supply,
                actual: 0,
                drift: -1,
            });
        }

        let mut total_wealth: u128 = 0;
        for &id in accounts {
            let account = ledger
                .get_account(id)
                .map_err(|_| OracleViolation::NegativeBalance {
                    node_id,
                    account_id: id,
                })?;

            let credits = account.balance.credits_posted.as_u128();
            let debits = account.balance.debits_posted.as_u128();
            if debits > credits {
                return Err(OracleViolation::NegativeBalance {
                    node_id,
                    account_id: id,
                });
            }
            let posted = credits - debits;

            let Some(next_wealth) = total_wealth.checked_add(posted) else {
                return Err(OracleViolation::WealthLeak {
                    node_id,
                    expected: expected_supply,
                    actual: u128::MAX,
                    drift: i128::MAX,
                });
            };
            total_wealth = next_wealth;
        }

        if total_wealth != expected_supply {
            let drift = (total_wealth as i128) - (expected_supply as i128);
            return Err(OracleViolation::WealthLeak {
                node_id,
                expected: expected_supply,
                actual: total_wealth,
                drift,
            });
        }

        Ok(())
    }

    /// Asserts that two nodes with overlapping committed histories agree exactly on all entries.
    ///
    /// # Errors
    ///
    /// Returns [`OracleViolation::SplitBrain`] if divergent journal events are detected.
    pub fn assert_log_agreement(
        node_a: u64,
        events_a: &[LedgerEvent],
        node_b: u64,
        events_b: &[LedgerEvent],
    ) -> Result<(), OracleViolation> {
        for (i, (ev_a, ev_b)) in events_a.iter().zip(events_b.iter()).enumerate() {
            if ev_a != ev_b {
                return Err(OracleViolation::SplitBrain {
                    node_a,
                    node_b,
                    index: i,
                });
            }
        }
        Ok(())
    }
}
