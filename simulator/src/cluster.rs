//! Multi-node cluster simulation executing consensus and replicated state machines.
//!
//! Models leader election, quorum commits (majority progression vs minority isolation),
//! WAL fsync durability, crash recovery, and catch-up log replication.

use std::collections::{BTreeMap, BTreeSet};

use ledger_core::account::{AccountFlags, AccountType};
use ledger_core::amount::{Amount, Scale};
use ledger_core::id::{AccountId, TransferId};
use ledger_core::transfer::Transfer;
use ledger_core::Ledger;

use crate::clock::SimInstant;
use crate::network::{NetworkConfig, SimNetwork};
use crate::oracle::{Oracle, OracleViolation};
use crate::rng::SimRng;
use crate::storage::SimDisk;
use crate::workload::WorkloadOp;

/// Inter-node consensus and replication protocol messages.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ClusterMessage {
    /// Propose a financial operation at a given term and index.
    Propose {
        /// Raft election term.
        term: u64,
        /// Monotonic log index.
        index: u64,
        /// Payload financial operation.
        op: WorkloadOp,
    },
    /// Acknowledge durable persistence of a proposed index.
    Ack {
        /// Acknowledging node ID.
        from: u64,
        /// Current term.
        term: u64,
        /// Acknowledged index.
        index: u64,
    },
    /// Notify peers that a quorum committed the entry at index with an authoritative timestamp.
    Commit {
        /// Term of committing leader.
        term: u64,
        /// Committed log index.
        index: u64,
        /// Payload operation.
        op: WorkloadOp,
        /// Canonical commit timestamp assigned by leader.
        timestamp: u64,
    },
    /// Solicit votes during leader election.
    RequestVote {
        /// New election term.
        term: u64,
        /// Candidate node soliciting vote.
        candidate_id: u64,
    },
    /// Grant vote to a candidate in the specified term.
    VoteGranted {
        /// Voter node ID.
        from: u64,
        /// Election term.
        term: u64,
    },
    /// Request log catch-up entries from leader.
    CatchupRequest {
        /// Requesting follower ID.
        from: u64,
        /// Last committed index known to follower.
        last_index: u64,
    },
    /// Reply with log entries to bring a partitioned follower up to date.
    CatchupResponse {
        /// Originating leader ID.
        from: u64,
        /// Log entries to apply with their canonical timestamps.
        entries: Vec<(u64, WorkloadOp, u64)>,
    },
}

/// Operational state of an individual cluster node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeStatus {
    /// Node is online and actively processing traffic.
    Active,
    /// Node is halted due to a crash or power cut.
    Crashed,
}

/// A simulated TrustLedger cluster node combining state machine, storage, and consensus state.
#[derive(Debug, Clone)]
pub struct SimNode {
    /// Node identifier.
    pub id: u64,
    /// Accounting state machine.
    pub ledger: Ledger,
    /// Fault-injected durable storage.
    pub disk: SimDisk,
    /// Node health status.
    pub status: NodeStatus,
    /// Current consensus term.
    pub term: u64,
    /// True if currently recognized as cluster leader.
    pub is_leader: bool,
    /// Log of committed operations with their canonical timestamps: `(index -> (op, timestamp))`.
    pub committed_ops: BTreeMap<u64, (WorkloadOp, u64)>,
    /// Out-of-order commits awaiting preceding log entries: `(index -> (op, timestamp))`.
    pub pending_commits: BTreeMap<u64, (WorkloadOp, u64)>,
    /// Uncommitted in-flight proposals pending quorum: `(index -> (op, acks))`.
    pub uncommitted: BTreeMap<u64, (WorkloadOp, BTreeSet<u64>)>,
    /// Highest log index committed by this node.
    pub commit_index: u64,
}

impl SimNode {
    /// Creates a new node with initialized accounts and funded customer balances.
    #[must_use]
    pub fn new(id: u64, scale: Scale, accounts: &[AccountId], initial_balance: u128) -> Self {
        let mut ledger = Ledger::new(scale);

        let vault_acc = AccountId::new(99_999);
        let _ = ledger.create_account(
            vault_acc,
            AccountType::Asset,
            AccountFlags::bank_asset(),
            scale,
            1_000,
        );

        let mut timestamp = 1_000;
        for (idx, &acc_id) in accounts.iter().enumerate() {
            timestamp += 1;
            let _ = ledger.create_account(
                acc_id,
                AccountType::Liability,
                AccountFlags::customer(),
                scale,
                timestamp,
            );

            if initial_balance > 0 {
                timestamp += 1;
                if let Ok(seed_tx) = Transfer::new_immediate(
                    TransferId::new(50_000 + idx as u128),
                    vault_acc,
                    acc_id,
                    Amount::new(initial_balance),
                    timestamp,
                ) {
                    let _ = ledger.create_transfer(seed_tx);
                }
            }
        }

        Self {
            id,
            ledger,
            disk: SimDisk::new(),
            status: NodeStatus::Active,
            term: 1,
            is_leader: id == 1, // Node 1 is initial bootstrap leader
            committed_ops: BTreeMap::new(),
            pending_commits: BTreeMap::new(),
            uncommitted: BTreeMap::new(),
            commit_index: 0,
        }
    }

    /// Applies an operation directly to the local state machine and flushes to durable disk.
    pub fn apply_op(&mut self, op: &WorkloadOp, timestamp: u64) {
        match op {
            WorkloadOp::DirectTransfer {
                transfer_id,
                from,
                to,
                amount,
            } => {
                if let Ok(tx) = Transfer::new_immediate(
                    TransferId::new(*transfer_id),
                    AccountId::new(*from),
                    AccountId::new(*to),
                    Amount::new(*amount),
                    timestamp,
                ) {
                    let _ = self.ledger.create_transfer(tx);
                }
            }
            WorkloadOp::CreatePending {
                transfer_id,
                from,
                to,
                amount,
            } => {
                if let Ok(hold) = Transfer::new_pending(
                    TransferId::new(*transfer_id),
                    AccountId::new(*from),
                    AccountId::new(*to),
                    Amount::new(*amount),
                    timestamp,
                ) {
                    let _ = self.ledger.create_pending(hold);
                }
            }
            WorkloadOp::PostPending {
                pending_id,
                post_id,
                amount,
            } => {
                let _ = self.ledger.post_pending(
                    TransferId::new(*pending_id),
                    TransferId::new(*post_id),
                    Amount::new(*amount),
                    timestamp,
                );
            }
            WorkloadOp::VoidPending {
                pending_id,
                void_id: _,
            } => {
                let _ = self
                    .ledger
                    .void_pending(TransferId::new(*pending_id), timestamp);
            }
        }

        // Durably sync state modification to simulated disk
        let mut log_bytes = Vec::new();
        log_bytes.extend_from_slice(&timestamp.to_be_bytes());
        self.disk.write(&log_bytes);
        self.disk.sync();
    }
}

/// Replicated cluster simulator coordinating nodes, network, and fault injection.
#[derive(Debug, Clone)]
pub struct SimCluster {
    /// Active cluster nodes keyed by ID.
    pub nodes: BTreeMap<u64, SimNode>,
    /// Virtual packet transport network.
    pub network: SimNetwork<ClusterMessage>,
    /// Monotonic operation index counter across the cluster.
    pub next_log_index: u64,
    /// Account IDs tracked across the cluster.
    pub accounts: Vec<AccountId>,
    /// Total constant money supply initialized in the cluster.
    pub total_wealth: u128,
}

impl SimCluster {
    /// Creates a new 3-node simulated cluster initialized with known accounts.
    #[must_use]
    pub fn new_3_node(
        net_config: NetworkConfig,
        account_ids: &[u128],
        balance_per_account: u128,
    ) -> Self {
        let scale = Scale::usdc();
        let accounts: Vec<AccountId> = account_ids.iter().copied().map(AccountId::new).collect();
        let total_wealth = balance_per_account.saturating_mul(account_ids.len() as u128);

        let mut nodes = BTreeMap::new();
        for id in 1..=3 {
            nodes.insert(id, SimNode::new(id, scale, &accounts, balance_per_account));
        }

        Self {
            nodes,
            network: SimNetwork::new(net_config),
            next_log_index: 0,
            accounts,
            total_wealth,
        }
    }

    /// Proposes a new workload operation to the cluster leader.
    pub fn propose(&mut self, op: WorkloadOp, now: SimInstant, rng: &mut SimRng) -> bool {
        let leader_id = self.find_leader_id();
        let Some(leader_id) = leader_id else {
            return false;
        };

        if self.nodes.get(&leader_id).map(|n| n.status) != Some(NodeStatus::Active) {
            return false;
        }

        self.next_log_index = self.next_log_index.saturating_add(1);
        let index = self.next_log_index;
        let term = self.nodes.get(&leader_id).map_or(0, |n| n.term);

        // Leader records self-ack
        if let Some(leader) = self.nodes.get_mut(&leader_id) {
            let mut acks = BTreeSet::new();
            acks.insert(leader_id);
            leader.uncommitted.insert(index, (op.clone(), acks));
        }

        // Broadcast proposal to peer nodes
        let peer_ids: Vec<u64> = self
            .nodes
            .keys()
            .copied()
            .filter(|&id| id != leader_id)
            .collect();
        for peer_id in peer_ids {
            let msg = ClusterMessage::Propose {
                term,
                index,
                op: op.clone(),
            };
            self.network.send(leader_id, peer_id, msg, now, rng);
        }

        true
    }

    /// Returns the ID of the current leader node, if any.
    #[must_use]
    pub fn find_leader_id(&self) -> Option<u64> {
        self.nodes
            .values()
            .find(|n| n.is_leader && n.status == NodeStatus::Active)
            .map(|n| n.id)
    }

    /// Advances the cluster by processing deliverable packets at tick `now`.
    pub fn step(&mut self, now: SimInstant, rng: &mut SimRng) {
        let packets = self.network.drain_ready(now);

        for packet in packets {
            self.process_packet(packet.from, packet.to, packet.payload, now, rng);
        }
    }

    fn process_packet(
        &mut self,
        from: u64,
        to: u64,
        msg: ClusterMessage,
        now: SimInstant,
        rng: &mut SimRng,
    ) {
        // Drop message if destination node is currently crashed
        let dest_status = self.nodes.get(&to).map(|n| n.status);
        if dest_status != Some(NodeStatus::Active) {
            return;
        }

        match msg {
            ClusterMessage::Propose { term, index, op } => {
                let mut reply = None;
                if let Some(node) = self.nodes.get_mut(&to) {
                    if term >= node.term {
                        node.term = term;
                        node.uncommitted.insert(index, (op, BTreeSet::new()));
                        reply = Some(ClusterMessage::Ack {
                            from: to,
                            term,
                            index,
                        });
                    }
                }
                if let Some(reply_msg) = reply {
                    self.network.send(to, from, reply_msg, now, rng);
                }
            }
            ClusterMessage::Ack {
                from: ack_from,
                term,
                index,
            } => {
                let mut commit_op = None;
                let quorum_size = (self.nodes.len() / 2) + 1; // 2 out of 3

                if let Some(leader) = self.nodes.get_mut(&to) {
                    if leader.is_leader && leader.term == term {
                        if let Some((op, acks)) = leader.uncommitted.get_mut(&index) {
                            acks.insert(ack_from);
                            if acks.len() >= quorum_size
                                && !leader.committed_ops.contains_key(&index)
                            {
                                commit_op = Some(op.clone());
                            }
                        }
                    }
                }

                if let Some(op) = commit_op {
                    let timestamp = now.ticks().saturating_add(1_700_000_000);
                    let mut newly_committed = Vec::new();

                    if let Some(leader) = self.nodes.get_mut(&to) {
                        leader.pending_commits.insert(index, (op, timestamp));
                        while let Some((next_op, next_ts)) =
                            leader.pending_commits.remove(&(leader.commit_index + 1))
                        {
                            let next_idx = leader.commit_index + 1;
                            leader.apply_op(&next_op, next_ts);
                            leader
                                .committed_ops
                                .insert(next_idx, (next_op.clone(), next_ts));
                            leader.commit_index = next_idx;
                            leader.uncommitted.remove(&next_idx);
                            newly_committed.push((next_idx, next_op, next_ts));
                        }
                    }

                    // Broadcast commit for all newly committed sequential items to all peers
                    let peer_ids: Vec<u64> =
                        self.nodes.keys().copied().filter(|&id| id != to).collect();
                    for peer_id in peer_ids {
                        for &(c_idx, ref c_op, c_ts) in &newly_committed {
                            let commit_msg = ClusterMessage::Commit {
                                term,
                                index: c_idx,
                                op: c_op.clone(),
                                timestamp: c_ts,
                            };
                            self.network.send(to, peer_id, commit_msg, now, rng);
                        }
                    }
                }
            }
            ClusterMessage::Commit {
                term,
                index,
                op,
                timestamp,
            } => {
                if let Some(node) = self.nodes.get_mut(&to) {
                    if term >= node.term {
                        node.term = term;
                        if !node.committed_ops.contains_key(&index) {
                            node.pending_commits.insert(index, (op, timestamp));
                            while let Some((next_op, next_ts)) =
                                node.pending_commits.remove(&(node.commit_index + 1))
                            {
                                let next_idx = node.commit_index + 1;
                                node.apply_op(&next_op, next_ts);
                                node.committed_ops.insert(next_idx, (next_op, next_ts));
                                node.commit_index = next_idx;
                                node.uncommitted.remove(&next_idx);
                            }
                        }
                    }
                }
            }
            ClusterMessage::RequestVote { term, candidate_id } => {
                let mut grant = false;
                if let Some(node) = self.nodes.get_mut(&to) {
                    if term > node.term {
                        node.term = term;
                        node.is_leader = false;
                        grant = true;
                    }
                }
                if grant {
                    self.network.send(
                        to,
                        candidate_id,
                        ClusterMessage::VoteGranted { from: to, term },
                        now,
                        rng,
                    );
                }
            }
            ClusterMessage::VoteGranted { from: _, term } => {
                if let Some(node) = self.nodes.get_mut(&to) {
                    if node.term == term && !node.is_leader {
                        node.is_leader = true;
                    }
                }
            }
            ClusterMessage::CatchupRequest {
                from: req_from,
                last_index,
            } => {
                let mut entries = Vec::new();
                if let Some(node) = self.nodes.get(&to) {
                    for (&idx, (op, ts)) in &node.committed_ops {
                        if idx > last_index {
                            entries.push((idx, op.clone(), *ts));
                        }
                    }
                }
                if !entries.is_empty() {
                    self.network.send(
                        to,
                        req_from,
                        ClusterMessage::CatchupResponse { from: to, entries },
                        now,
                        rng,
                    );
                }
            }
            ClusterMessage::CatchupResponse { from: _, entries } => {
                if let Some(node) = self.nodes.get_mut(&to) {
                    for (idx, op, ts) in entries {
                        if !node.committed_ops.contains_key(&idx) {
                            node.pending_commits.insert(idx, (op, ts));
                        }
                    }
                    while let Some((next_op, next_ts)) =
                        node.pending_commits.remove(&(node.commit_index + 1))
                    {
                        let next_idx = node.commit_index + 1;
                        node.apply_op(&next_op, next_ts);
                        node.committed_ops.insert(next_idx, (next_op, next_ts));
                        node.commit_index = next_idx;
                        node.uncommitted.remove(&next_idx);
                    }
                }
            }
        }
    }

    /// Triggers leader election on a live node when heartbeats time out.
    pub fn trigger_election(&mut self, candidate_id: u64, now: SimInstant, rng: &mut SimRng) {
        if let Some(candidate) = self.nodes.get_mut(&candidate_id) {
            if candidate.status == NodeStatus::Active {
                candidate.term = candidate.term.saturating_add(1);
                candidate.is_leader = true; // Votes for self
                let term = candidate.term;

                let peer_ids: Vec<u64> = self
                    .nodes
                    .keys()
                    .copied()
                    .filter(|&id| id != candidate_id)
                    .collect();

                for peer_id in peer_ids {
                    self.network.send(
                        candidate_id,
                        peer_id,
                        ClusterMessage::RequestVote { term, candidate_id },
                        now,
                        rng,
                    );
                }
            }
        }
    }

    /// Crashes a node, dropping volatile memory and simulating torn-write disk behavior.
    pub fn crash_node(&mut self, node_id: u64, torn_write: bool, rng: &mut SimRng) {
        if let Some(node) = self.nodes.get_mut(&node_id) {
            node.status = NodeStatus::Crashed;
            node.is_leader = false;
            node.uncommitted.clear();
            node.disk.crash(rng, torn_write);
        }
        self.network.isolate(node_id);
    }

    /// Reboots a crashed node, reconnecting it to the network and triggering catch-up sync.
    pub fn reboot_node(&mut self, node_id: u64, now: SimInstant, rng: &mut SimRng) {
        let mut last_index = 0;
        if let Some(node) = self.nodes.get_mut(&node_id) {
            node.status = NodeStatus::Active;
            node.is_leader = false;
            last_index = node.commit_index;
        }
        self.network.unisolate(node_id);

        // Send catch-up request to remaining nodes
        let peers: Vec<u64> = self
            .nodes
            .keys()
            .copied()
            .filter(|&id| id != node_id)
            .collect();
        for peer in peers {
            self.network.send(
                node_id,
                peer,
                ClusterMessage::CatchupRequest {
                    from: node_id,
                    last_index,
                },
                now,
                rng,
            );
        }
    }

    /// Asserts all financial and consistency invariants across all online nodes.
    ///
    /// # Errors
    ///
    /// Returns [`OracleViolation`] if money conservation fails or split-brain logs are detected.
    pub fn verify_invariants(&self) -> Result<(), OracleViolation> {
        let active_nodes: Vec<&SimNode> = self
            .nodes
            .values()
            .filter(|n| n.status == NodeStatus::Active)
            .collect();

        // 1. Invariant: Wealth conservation on every active node
        for node in &active_nodes {
            Oracle::assert_conservation(node.id, &node.ledger, &self.accounts, self.total_wealth)?;
        }

        // 2. Invariant: Log agreement across all pairs of active nodes
        for i in 0..active_nodes.len() {
            for j in (i + 1)..active_nodes.len() {
                let events_a = active_nodes[i].ledger.journal();
                let events_b = active_nodes[j].ledger.journal();
                Oracle::assert_log_agreement(
                    active_nodes[i].id,
                    events_a,
                    active_nodes[j].id,
                    events_b,
                )?;
            }
        }

        Ok(())
    }
}
