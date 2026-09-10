//! Simulated in-memory network router and RPC transport for Raft nodes.
//!
//! Provides deterministic fault injection (network partitions, packet loss,
//! isolation, and healing) for rigorous Jepsen-style consensus testing.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use openraft::error::{InstallSnapshotError, RPCError, RaftError, RemoteError, Unreachable};
use openraft::network::{RPCOption, RaftNetwork, RaftNetworkFactory};
use openraft::raft::{
    AppendEntriesRequest, AppendEntriesResponse, InstallSnapshotRequest, InstallSnapshotResponse,
    VoteRequest, VoteResponse,
};
use openraft::{BasicNode, Raft};
use tokio::sync::RwLock;

use crate::types::TypeConfig;

/// State for the simulated network router.
#[derive(Default)]
struct RouterState {
    /// Active nodes registered in the cluster.
    nodes: BTreeMap<u64, Raft<TypeConfig>>,
    /// Nodes completely isolated from all traffic.
    isolated: BTreeSet<u64>,
    /// Active partition pairs (traffic between set A and set B is blocked).
    partitions: Vec<(BTreeSet<u64>, BTreeSet<u64>)>,
}

impl RouterState {
    fn can_communicate(&self, from: u64, to: u64) -> bool {
        if self.isolated.contains(&from) || self.isolated.contains(&to) {
            return false;
        }

        for (set_a, set_b) in &self.partitions {
            if (set_a.contains(&from) && set_b.contains(&to))
                || (set_b.contains(&from) && set_a.contains(&to))
            {
                return false;
            }
        }

        true
    }
}

/// Simulated network router managing routing and chaos fault injection between cluster nodes.
#[derive(Clone, Default)]
pub struct NetworkRouter {
    state: Arc<RwLock<RouterState>>,
}

impl NetworkRouter {
    /// Creates a new network router.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Arc::new(RwLock::new(RouterState::default())),
        }
    }

    /// Registers a Raft node handle in the network router.
    pub async fn register(&self, node_id: u64, raft: Raft<TypeConfig>) {
        let mut state = self.state.write().await;
        state.nodes.insert(node_id, raft);
    }

    /// Unregisters (removes) a node from the network, simulating an ungraceful kill/crash.
    pub async fn unregister(&self, node_id: u64) {
        let mut state = self.state.write().await;
        state.nodes.remove(&node_id);
    }

    /// Isolates a node completely from all cluster network traffic.
    pub async fn isolate(&self, node_id: u64) {
        let mut state = self.state.write().await;
        state.isolated.insert(node_id);
    }

    /// Reconnects a previously isolated node to the network.
    pub async fn unisolate(&self, node_id: u64) {
        let mut state = self.state.write().await;
        state.isolated.remove(&node_id);
    }

    /// Partitions the network between two disjoint sets of node IDs.
    pub async fn partition(&self, set_a: BTreeSet<u64>, set_b: BTreeSet<u64>) {
        let mut state = self.state.write().await;
        state.partitions.push((set_a, set_b));
    }

    /// Heals all active network partitions and isolations.
    pub async fn heal(&self) {
        let mut state = self.state.write().await;
        state.isolated.clear();
        state.partitions.clear();
    }
}

/// Network factory creating router-based network clients.
#[derive(Clone)]
pub struct RouterNetworkFactory {
    source: u64,
    router: NetworkRouter,
}

impl RouterNetworkFactory {
    /// Creates a new network factory for a specific source node.
    #[must_use]
    pub fn new(source: u64, router: NetworkRouter) -> Self {
        Self { source, router }
    }
}

impl RaftNetworkFactory<TypeConfig> for RouterNetworkFactory {
    type Network = RouterNetwork;

    async fn new_client(&mut self, target: u64, _node: &BasicNode) -> Self::Network {
        RouterNetwork {
            source: self.source,
            target,
            router: self.router.clone(),
        }
    }
}

/// Connection handle from a source node to a target node through the [`NetworkRouter`].
pub struct RouterNetwork {
    source: u64,
    target: u64,
    router: NetworkRouter,
}

impl RouterNetwork {
    #[allow(clippy::result_large_err)]
    async fn get_target_raft(
        &self,
    ) -> Result<Raft<TypeConfig>, RPCError<u64, BasicNode, RaftError<u64>>> {
        let state = self.router.state.read().await;

        if !state.can_communicate(self.source, self.target) {
            let io_err = std::io::Error::new(
                std::io::ErrorKind::ConnectionRefused,
                format!("partitioned: {} -> {}", self.source, self.target),
            );
            return Err(RPCError::Unreachable(Unreachable::new(&io_err)));
        }

        match state.nodes.get(&self.target) {
            Some(raft) => Ok(raft.clone()),
            None => {
                let io_err = std::io::Error::new(
                    std::io::ErrorKind::ConnectionRefused,
                    format!("node {} offline or dead", self.target),
                );
                Err(RPCError::Unreachable(Unreachable::new(&io_err)))
            }
        }
    }
}

impl RaftNetwork<TypeConfig> for RouterNetwork {
    async fn append_entries(
        &mut self,
        rpc: AppendEntriesRequest<TypeConfig>,
        _option: RPCOption,
    ) -> Result<AppendEntriesResponse<u64>, RPCError<u64, BasicNode, RaftError<u64>>> {
        let raft = self.get_target_raft().await?;
        raft.append_entries(rpc)
            .await
            .map_err(|e| RPCError::RemoteError(RemoteError::new(self.target, e)))
    }

    async fn vote(
        &mut self,
        rpc: VoteRequest<u64>,
        _option: RPCOption,
    ) -> Result<VoteResponse<u64>, RPCError<u64, BasicNode, RaftError<u64>>> {
        let raft = self.get_target_raft().await?;
        raft.vote(rpc)
            .await
            .map_err(|e| RPCError::RemoteError(RemoteError::new(self.target, e)))
    }

    async fn install_snapshot(
        &mut self,
        rpc: InstallSnapshotRequest<TypeConfig>,
        _option: RPCOption,
    ) -> Result<
        InstallSnapshotResponse<u64>,
        RPCError<u64, BasicNode, RaftError<u64, InstallSnapshotError>>,
    > {
        let state = self.router.state.read().await;

        if !state.can_communicate(self.source, self.target) {
            let io_err = std::io::Error::new(
                std::io::ErrorKind::ConnectionRefused,
                format!("partitioned: {} -> {}", self.source, self.target),
            );
            return Err(RPCError::Unreachable(Unreachable::new(&io_err)));
        }

        let raft = match state.nodes.get(&self.target) {
            Some(raft) => raft.clone(),
            None => {
                let io_err = std::io::Error::new(
                    std::io::ErrorKind::ConnectionRefused,
                    format!("node {} offline or dead", self.target),
                );
                return Err(RPCError::Unreachable(Unreachable::new(&io_err)));
            }
        };

        raft.install_snapshot(rpc)
            .await
            .map_err(|e| RPCError::RemoteError(RemoteError::new(self.target, e)))
    }
}
