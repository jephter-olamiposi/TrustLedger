//! Cluster simulation for consensus and replication.
//!
//! The harness exercises leader election, quorum commits, WAL durability, crash
//! recovery, and catch-up replication under deterministic failures.

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
        /// Monotonic log index of candidate's last committed entry.
        last_index: u64,
    },
    /// Grant vote to a candidate in the specified term.
    VoteGranted {
        /// Voter node ID.
        from: u64,
        /// Election term.
        term: u64,
    },
    /// Heartbeat sent periodically by leader to suppress follower elections.
    Heartbeat {
        /// Leader's current term.
        term: u64,
        /// Leader node ID.
        leader_id: u64,
        /// Current commit index of leader.
        commit_index: u64,
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

/// Consensus role of an individual cluster node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeRole {
    /// Follower waiting for periodic heartbeats or proposals from leader.
    Follower,
    /// Candidate soliciting votes across the cluster.
    Candidate,
    /// Authoritative cluster leader coordinating log replication.
    Leader,
}

/// Simulated WAL entry written to durable disk for each committed cluster operation.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SimWalEntry {
    /// Consensus log index.
    pub index: u64,
    /// Canonical timestamp assigned to this entry.
    pub timestamp: u64,
    /// Financial workload operation.
    pub op: WorkloadOp,
}

/// A simulated TrustLedger cluster node combining state machine, storage, and consensus state.
#[derive(Debug, Clone)]
pub struct SimNode {
    /// Node identifier.
    pub id: u64,
    /// Accounting state machine.
    pub ledger: Ledger,
    /// Currency decimal scale.
    pub scale: Scale,
    /// Configured accounts tracked across the cluster.
    pub accounts: Vec<AccountId>,
    /// Initial balance per customer account.
    pub initial_balance: u128,
    /// Fault-injected durable storage.
    pub disk: SimDisk,
    /// Node health status.
    pub status: NodeStatus,
    /// Current consensus role (Follower, Candidate, Leader).
    pub role: NodeRole,
    /// Current consensus term.
    pub term: u64,
    /// True if currently recognized as cluster leader.
    pub is_leader: bool,
    /// Candidate voted for in current term.
    pub voted_for: Option<u64>,
    /// Set of voter IDs that granted votes in current election term.
    pub votes_received: BTreeSet<u64>,
    /// Virtual tick of last received heartbeat or leader communication.
    pub last_heartbeat_tick: u64,
    /// Randomized election timeout in virtual ticks.
    pub election_timeout: u64,
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
        let accounts_vec = accounts.to_vec();
        let mut node = Self {
            id,
            ledger: Ledger::new(scale),
            scale,
            accounts: accounts_vec,
            initial_balance,
            disk: SimDisk::new(),
            status: NodeStatus::Active,
            role: if id == 1 {
                NodeRole::Leader
            } else {
                NodeRole::Follower
            },
            term: 1,
            is_leader: id == 1,
            voted_for: if id == 1 { Some(1) } else { None },
            votes_received: BTreeSet::new(),
            last_heartbeat_tick: 0,
            election_timeout: 15 + (id * 5),
            committed_ops: BTreeMap::new(),
            pending_commits: BTreeMap::new(),
            uncommitted: BTreeMap::new(),
            commit_index: 0,
        };

        node.reinit_base_state();
        node
    }

    /// Re-initializes base genesis state machine with accounts and seed balance.
    pub fn reinit_base_state(&mut self) {
        let mut ledger = Ledger::new(self.scale);
        let vault_acc = AccountId::new(99_999);
        let _ = ledger.create_account(
            vault_acc,
            AccountType::Asset,
            AccountFlags::bank_asset(),
            self.scale,
            1_000,
        );

        let mut timestamp = 1_000;
        for (idx, &acc_id) in self.accounts.iter().enumerate() {
            timestamp += 1;
            let _ = ledger.create_account(
                acc_id,
                AccountType::Liability,
                AccountFlags::customer(),
                self.scale,
                timestamp,
            );

            if self.initial_balance > 0 {
                timestamp += 1;
                if let Ok(seed_tx) = Transfer::new_immediate(
                    TransferId::new(50_000 + idx as u128),
                    vault_acc,
                    acc_id,
                    Amount::new(self.initial_balance),
                    timestamp,
                ) {
                    let _ = ledger.create_transfer(seed_tx);
                }
            }
        }
        self.ledger = ledger;
    }

    /// Applies an operation directly to in-memory state machine without writing to disk.
    fn apply_op_in_memory(&mut self, index: u64, op: &WorkloadOp, timestamp: u64) {
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
        self.committed_ops.insert(index, (op.clone(), timestamp));
        self.commit_index = self.commit_index.max(index);
    }

    /// Applies an operation to the state machine and flushes a CRC32C-verified WAL frame to disk.
    pub fn apply_op(&mut self, index: u64, op: &WorkloadOp, timestamp: u64) {
        self.apply_op_in_memory(index, op, timestamp);

        let entry = SimWalEntry {
            index,
            timestamp,
            op: op.clone(),
        };
        if let Ok(payload) = postcard::to_allocvec(&entry) {
            if let Ok(frame) =
                wal::record::encode(index, &payload, wal::record::DEFAULT_MAX_PAYLOAD_LEN)
            {
                self.disk.write(&frame);
                self.disk.sync();
            }
        }
    }

    /// Recovers state machine from durable WAL disk blocks, verifying CRC32C checksums
    /// and cleanly discarding any torn write tail.
    pub fn recover(&mut self) {
        use std::io::BufReader;

        self.reinit_base_state();
        self.committed_ops.clear();
        self.pending_commits.clear();
        self.uncommitted.clear();
        self.commit_index = 0;

        let durable_bytes = self.disk.read_durable().to_vec();
        let mut reader = BufReader::new(durable_bytes.as_slice());
        let mut valid_bytes = 0usize;

        loop {
            match wal::record::next_record(&mut reader, wal::record::DEFAULT_MAX_PAYLOAD_LEN, true)
            {
                Ok(wal::record::ScanStep::Record {
                    len,
                    payload: Some(payload),
                    ..
                }) => {
                    let frame_size = wal::record::frame_len(len);
                    if let Ok(entry) = postcard::from_bytes::<SimWalEntry>(&payload) {
                        self.apply_op_in_memory(entry.index, &entry.op, entry.timestamp);
                        valid_bytes += frame_size;
                    } else {
                        break;
                    }
                }
                Ok(wal::record::ScanStep::Record { .. }) => {}
                Ok(wal::record::ScanStep::End) => {
                    break;
                }
                Ok(wal::record::ScanStep::Broken) => {
                    // Torn write or corrupted frame detected via CRC32C. Truncate log here.
                    break;
                }
                Err(_) => {
                    break;
                }
            }
        }

        self.disk.truncate_durable(valid_bytes);
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

        if let Some(leader) = self.nodes.get_mut(&leader_id) {
            let mut acks = BTreeSet::new();
            acks.insert(leader_id);
            leader.uncommitted.insert(index, (op.clone(), acks));
        }

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

    /// Advances the cluster by processing deliverable packets at tick `now` and updating node timers.
    pub fn step(&mut self, now: SimInstant, rng: &mut SimRng) {
        let packets = self.network.drain_ready(now);

        for packet in packets {
            self.process_packet(packet.from, packet.to, packet.payload, now, rng);
        }

        self.tick_nodes(now, rng);
    }

    fn tick_nodes(&mut self, now: SimInstant, rng: &mut SimRng) {
        let active_node_ids: Vec<u64> = self
            .nodes
            .values()
            .filter(|n| n.status == NodeStatus::Active)
            .map(|n| n.id)
            .collect();

        for node_id in active_node_ids {
            let mut action = None;
            if let Some(node) = self.nodes.get_mut(&node_id) {
                if node.role == NodeRole::Leader {
                    // Invariant: leader broadcasts heartbeats every 5 ticks to suppress elections
                    if now.ticks().saturating_sub(node.last_heartbeat_tick) >= 5 {
                        node.last_heartbeat_tick = now.ticks();
                        action = Some((node.term, node.commit_index, true));
                    }
                } else if now.ticks().saturating_sub(node.last_heartbeat_tick)
                    >= node.election_timeout
                {
                    // Election timeout expired: transition to candidate and solicit quorum votes
                    node.term = node.term.saturating_add(1);
                    node.role = NodeRole::Candidate;
                    node.is_leader = false;
                    node.voted_for = Some(node_id);
                    node.votes_received.clear();
                    node.votes_received.insert(node_id);
                    node.last_heartbeat_tick = now.ticks();
                    node.election_timeout = rng.gen_range(15..=30);
                    action = Some((node.term, node.commit_index, false));
                }
            }

            if let Some((term, commit_index, is_heartbeat)) = action {
                let peer_ids: Vec<u64> = self
                    .nodes
                    .keys()
                    .copied()
                    .filter(|&id| id != node_id)
                    .collect();

                for peer_id in peer_ids {
                    let msg = if is_heartbeat {
                        ClusterMessage::Heartbeat {
                            term,
                            leader_id: node_id,
                            commit_index,
                        }
                    } else {
                        ClusterMessage::RequestVote {
                            term,
                            candidate_id: node_id,
                            last_index: commit_index,
                        }
                    };
                    self.network.send(node_id, peer_id, msg, now, rng);
                }
            }
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
        let dest_status = self.nodes.get(&to).map(|n| n.status);
        if dest_status != Some(NodeStatus::Active) {
            return;
        }

        match msg {
            ClusterMessage::Propose { term, index, op } => {
                let mut reply = None;
                if let Some(node) = self.nodes.get_mut(&to) {
                    if term > node.term {
                        node.term = term;
                        node.role = NodeRole::Follower;
                        node.is_leader = false;
                        node.voted_for = None;
                    }
                    if term >= node.term {
                        node.last_heartbeat_tick = now.ticks();
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
                let quorum_size = (self.nodes.len() / 2) + 1;

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
                            leader.apply_op(next_idx, &next_op, next_ts);
                            leader.uncommitted.remove(&next_idx);
                            newly_committed.push((next_idx, next_op, next_ts));
                        }
                    }

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
                    if term > node.term {
                        node.term = term;
                        node.role = NodeRole::Follower;
                        node.is_leader = false;
                        node.voted_for = None;
                    }
                    if term >= node.term {
                        node.last_heartbeat_tick = now.ticks();
                        if !node.committed_ops.contains_key(&index) {
                            node.pending_commits.insert(index, (op, timestamp));
                            while let Some((next_op, next_ts)) =
                                node.pending_commits.remove(&(node.commit_index + 1))
                            {
                                let next_idx = node.commit_index + 1;
                                node.apply_op(next_idx, &next_op, next_ts);
                                node.uncommitted.remove(&next_idx);
                            }
                        }
                    }
                }
            }
            ClusterMessage::Heartbeat {
                term,
                leader_id,
                commit_index,
            } => {
                let mut needs_catchup = false;
                let mut my_commit_index = 0;

                if let Some(node) = self.nodes.get_mut(&to) {
                    if term > node.term {
                        node.term = term;
                        node.role = NodeRole::Follower;
                        node.is_leader = false;
                        node.voted_for = None;
                    }
                    if term >= node.term {
                        node.last_heartbeat_tick = now.ticks();
                        node.role = NodeRole::Follower;
                        node.is_leader = false;
                        if commit_index > node.commit_index {
                            needs_catchup = true;
                            my_commit_index = node.commit_index;
                        }
                    }
                }

                if needs_catchup {
                    self.network.send(
                        to,
                        leader_id,
                        ClusterMessage::CatchupRequest {
                            from: to,
                            last_index: my_commit_index,
                        },
                        now,
                        rng,
                    );
                }
            }
            ClusterMessage::RequestVote {
                term,
                candidate_id,
                last_index,
            } => {
                let mut grant = false;
                if let Some(node) = self.nodes.get_mut(&to) {
                    if term > node.term {
                        node.term = term;
                        node.role = NodeRole::Follower;
                        node.is_leader = false;
                        node.voted_for = None;
                    }
                    let log_ok = last_index >= node.commit_index;
                    let vote_ok = node.voted_for.is_none() || node.voted_for == Some(candidate_id);
                    if term == node.term && vote_ok && log_ok {
                        node.voted_for = Some(candidate_id);
                        node.last_heartbeat_tick = now.ticks();
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
            ClusterMessage::VoteGranted { from, term } => {
                let mut became_leader = false;
                let quorum_size = (self.nodes.len() / 2) + 1;

                if let Some(node) = self.nodes.get_mut(&to) {
                    if node.term == term && node.role == NodeRole::Candidate {
                        node.votes_received.insert(from);
                        if node.votes_received.len() >= quorum_size {
                            node.role = NodeRole::Leader;
                            node.is_leader = true;
                            node.last_heartbeat_tick = now.ticks();
                            became_leader = true;
                        }
                    }
                }

                if became_leader {
                    let commit_idx = self.nodes.get(&to).map_or(0, |n| n.commit_index);
                    let peer_ids: Vec<u64> =
                        self.nodes.keys().copied().filter(|&id| id != to).collect();
                    for peer in peer_ids {
                        self.network.send(
                            to,
                            peer,
                            ClusterMessage::Heartbeat {
                                term,
                                leader_id: to,
                                commit_index: commit_idx,
                            },
                            now,
                            rng,
                        );
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
                        node.apply_op(next_idx, &next_op, next_ts);
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
                candidate.role = NodeRole::Candidate;
                candidate.is_leader = false;
                candidate.voted_for = Some(candidate_id);
                candidate.votes_received.clear();
                candidate.votes_received.insert(candidate_id);
                candidate.last_heartbeat_tick = now.ticks();
                candidate.election_timeout = rng.gen_range(15..=30);
                let term = candidate.term;
                let last_index = candidate.commit_index;

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
                        ClusterMessage::RequestVote {
                            term,
                            candidate_id,
                            last_index,
                        },
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
            node.role = NodeRole::Follower;
            node.is_leader = false;
            node.voted_for = None;
            node.votes_received.clear();
            node.uncommitted.clear();
            node.pending_commits.clear();
            node.disk.crash(rng, torn_write);
        }
        self.network.isolate(node_id);
    }

    /// Reboots a crashed node, recovering from disk and requesting catch-up synchronization.
    pub fn reboot_node(&mut self, node_id: u64, now: SimInstant, rng: &mut SimRng) {
        let mut last_index = 0;
        if let Some(node) = self.nodes.get_mut(&node_id) {
            node.recover();
            node.status = NodeStatus::Active;
            node.role = NodeRole::Follower;
            node.is_leader = false;
            node.voted_for = None;
            node.votes_received.clear();
            node.last_heartbeat_tick = now.ticks();
            node.election_timeout = rng.gen_range(15..=30);
            last_index = node.commit_index;
        }
        self.network.unisolate(node_id);

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

        // Invariant: wealth conservation must strictly hold on every active node.
        for node in &active_nodes {
            Oracle::assert_conservation(node.id, &node.ledger, &self.accounts, self.total_wealth)?;
        }

        // Invariant: log agreement must strictly hold across all pairs of active nodes.
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
