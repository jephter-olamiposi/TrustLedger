//! Multi-node cluster harness for coordination, testing, and chaos injection.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use ledger_core::Scale;
use openraft::error::Fatal;
use openraft::{BasicNode, Config};
use tokio::time::{sleep, Instant};

use crate::network::NetworkRouter;
use crate::node::ConsensusNode;
use crate::types::{RaftClientError, RaftRequest, RaftResponse};

/// Errors encountered during cluster operations.
#[derive(Debug, thiserror::Error)]
pub enum RaftClusterError {
    /// Timed out waiting for an elected cluster leader.
    #[error("timeout waiting for cluster leader after {0:?}")]
    LeaderTimeout(Duration),
    /// Cluster membership initialization error.
    #[error("cluster initialization error: {0}")]
    Initialize(String),
    /// Fatal node error.
    #[error("fatal node error: {0}")]
    Fatal(Box<Fatal<u64>>),
    /// Client write error from the underlying Raft node.
    #[error("client write error: {0}")]
    ClientWrite(#[from] RaftClientError),
    /// Requested node ID not found in cluster.
    #[error("node {0} not found")]
    NodeNotFound(u64),
    /// Node shutdown error.
    #[error("node shutdown error: {0}")]
    Shutdown(String),
}

impl From<Fatal<u64>> for RaftClusterError {
    fn from(err: Fatal<u64>) -> Self {
        Self::Fatal(Box::new(err))
    }
}

/// A coordinated multi-node consensus cluster.
pub struct RaftCluster {
    nodes: BTreeMap<u64, ConsensusNode>,
    router: NetworkRouter,
}

impl RaftCluster {
    /// Creates and initializes a 3-node Raft consensus cluster with tuned low-latency timers.
    ///
    /// # Errors
    ///
    /// Returns [`RaftClusterError`] if creating nodes or bootstrapping membership fails.
    pub async fn new_3node(scale: Scale) -> Result<Self, RaftClusterError> {
        let config = Config {
            heartbeat_interval: 50,
            election_timeout_min: 150,
            election_timeout_max: 300,
            ..Default::default()
        };
        let config = Arc::new(
            config
                .validate()
                .map_err(|e| RaftClusterError::Initialize(e.to_string()))?,
        );

        let router = NetworkRouter::new();

        let node1 = ConsensusNode::new(1, config.clone(), router.clone(), scale).await?;
        let node2 = ConsensusNode::new(2, config.clone(), router.clone(), scale).await?;
        let node3 = ConsensusNode::new(3, config.clone(), router.clone(), scale).await?;

        let mut members = BTreeMap::new();
        members.insert(1, BasicNode::new("127.0.0.1:21001"));
        members.insert(2, BasicNode::new("127.0.0.1:21002"));
        members.insert(3, BasicNode::new("127.0.0.1:21003"));

        node1
            .raft()
            .initialize(members)
            .await
            .map_err(|e| RaftClusterError::Initialize(e.to_string()))?;

        let mut nodes = BTreeMap::new();
        nodes.insert(1, node1);
        nodes.insert(2, node2);
        nodes.insert(3, node3);

        Ok(Self { nodes, router })
    }

    /// Polls the cluster until an active leader is elected or the timeout expires.
    ///
    /// # Errors
    ///
    /// Returns [`RaftClusterError::LeaderTimeout`] if no leader is elected in time.
    pub async fn wait_for_leader(&self, timeout: Duration) -> Result<u64, RaftClusterError> {
        let start = Instant::now();

        while start.elapsed() < timeout {
            for (&id, node) in &self.nodes {
                if node.is_leader() {
                    return Ok(id);
                }
            }
            sleep(Duration::from_millis(15)).await;
        }

        Err(RaftClusterError::LeaderTimeout(timeout))
    }

    /// Returns a reference to the specified node by ID.
    #[must_use]
    pub fn get_node(&self, id: u64) -> Option<&ConsensusNode> {
        self.nodes.get(&id)
    }

    /// Returns a reference to all running nodes in the cluster.
    #[must_use]
    pub fn nodes(&self) -> &BTreeMap<u64, ConsensusNode> {
        &self.nodes
    }

    /// Returns a reference to the network router for chaos fault injection.
    #[must_use]
    pub fn router(&self) -> &NetworkRouter {
        &self.router
    }

    /// Submits a client write request to the current cluster leader, retrying on leadership handoffs.
    ///
    /// # Errors
    ///
    /// Returns [`RaftClientError`] if the proposal cannot be committed or fails validation.
    pub async fn propose(&self, request: RaftRequest) -> Result<RaftResponse, RaftClientError> {
        let mut retries = 0;

        while retries < 10 {
            // Find current leader
            let leader_id =
                self.nodes
                    .values()
                    .find_map(|n| if n.is_leader() { Some(n.id()) } else { None });

            if let Some(id) = leader_id {
                if let Some(node) = self.nodes.get(&id) {
                    match node.client_write(request.clone()).await {
                        Ok(resp) => return Ok(resp),
                        Err(RaftClientError::NotLeader { .. }) => {
                            sleep(Duration::from_millis(30)).await;
                            retries += 1;
                            continue;
                        }
                        Err(err) => return Err(err),
                    }
                }
            }

            sleep(Duration::from_millis(30)).await;
            retries += 1;
        }

        Err(RaftClientError::NotLeader { leader_id: None })
    }

    /// Isolates a node completely from network traffic.
    pub async fn isolate(&self, node_id: u64) {
        self.router.isolate(node_id).await;
    }

    /// Reconnects a previously isolated node to the network.
    pub async fn unisolate(&self, node_id: u64) {
        self.router.unisolate(node_id).await;
    }

    /// Partitions the cluster into two disjoint network components.
    pub async fn partition(&self, set_a: &[u64], set_b: &[u64]) {
        let a: BTreeSet<u64> = set_a.iter().copied().collect();
        let b: BTreeSet<u64> = set_b.iter().copied().collect();
        self.router.partition(a, b).await;
    }

    /// Restores full network connectivity across all cluster nodes.
    pub async fn heal(&self) {
        self.router.heal().await;
    }

    /// Forcibly kills and shuts down a specific node.
    ///
    /// # Errors
    ///
    /// Returns [`RaftClusterError`] if the node ID is invalid or shutdown fails.
    pub async fn kill_node(&mut self, node_id: u64) -> Result<(), RaftClusterError> {
        if let Some(node) = self.nodes.remove(&node_id) {
            self.router.unregister(node_id).await;
            node.shutdown().await.map_err(RaftClusterError::Shutdown)?;
            Ok(())
        } else {
            Err(RaftClusterError::NodeNotFound(node_id))
        }
    }

    /// Shuts down all running nodes in the cluster.
    pub async fn shutdown(&mut self) {
        for (&id, node) in &self.nodes {
            self.router.unregister(id).await;
            let _ = node.shutdown().await;
        }
        self.nodes.clear();
    }
}
