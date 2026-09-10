//! Encapsulated consensus node running an OpenRaft engine.

use std::sync::Arc;

use ledger_core::{Ledger, Scale};
use openraft::error::{ClientWriteError, Fatal, RaftError};
use openraft::{BasicNode, Config, Raft, RaftMetrics, ServerState};

use crate::network::{NetworkRouter, RouterNetworkFactory};
use crate::state_machine::LedgerStateMachine;
use crate::storage::MemLogStore;
use crate::types::{RaftClientError, RaftRequest, RaftResponse, TypeConfig};

/// A single Raft consensus node with integrated persistence and state machine.
pub struct ConsensusNode {
    id: u64,
    raft: Raft<TypeConfig>,
    state_machine: LedgerStateMachine,
    log_store: MemLogStore,
}

impl ConsensusNode {
    /// Creates and spawns a new consensus node.
    ///
    /// # Errors
    ///
    /// Returns [`Fatal`] if initializing the Raft core task fails.
    #[allow(clippy::result_large_err)]
    pub async fn new(
        id: u64,
        config: Arc<Config>,
        router: NetworkRouter,
        scale: Scale,
    ) -> Result<Self, Fatal<u64>> {
        let state_machine = LedgerStateMachine::new(scale);
        let log_store = MemLogStore::new();
        let network = RouterNetworkFactory::new(id, router.clone());

        let raft = Raft::new(
            id,
            config,
            network,
            log_store.clone(),
            state_machine.clone(),
        )
        .await?;

        // Register node handle in the router for incoming message delivery
        router.register(id, raft.clone()).await;

        Ok(Self {
            id,
            raft,
            state_machine,
            log_store,
        })
    }

    /// Returns the node's unique cluster ID.
    #[must_use]
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Returns a reference to the underlying [`Raft`] handle.
    #[must_use]
    pub fn raft(&self) -> &Raft<TypeConfig> {
        &self.raft
    }

    /// Returns a reference to the underlying [`MemLogStore`].
    #[must_use]
    pub fn log_store(&self) -> &MemLogStore {
        &self.log_store
    }

    /// Proposes a client mutation request to the Raft cluster through this node.
    ///
    /// # Errors
    ///
    /// Returns [`RaftClientError::NotLeader`] if this node is not the active leader,
    /// [`RaftClientError::Ledger`] if invariant checks fail, or [`RaftClientError::Fatal`]
    /// on cluster failure.
    pub async fn client_write(
        &self,
        request: RaftRequest,
    ) -> Result<RaftResponse, RaftClientError> {
        let res = self.raft.client_write(request).await;

        match res {
            Ok(client_response) => client_response.data,
            Err(RaftError::APIError(ClientWriteError::ForwardToLeader(fwd))) => {
                Err(RaftClientError::NotLeader {
                    leader_id: fwd.leader_id,
                })
            }
            Err(RaftError::APIError(ClientWriteError::ChangeMembershipError(err))) => {
                Err(RaftClientError::Fatal(err.to_string()))
            }
            Err(RaftError::Fatal(fatal)) => Err(RaftClientError::Fatal(fatal.to_string())),
        }
    }

    /// Returns `true` if this node is currently the elected cluster leader.
    #[must_use]
    pub fn is_leader(&self) -> bool {
        let metrics = self.raft.metrics().borrow().clone();
        metrics.state == ServerState::Leader
    }

    /// Returns the ID of the current leader known to this node, if any.
    #[must_use]
    pub fn current_leader(&self) -> Option<u64> {
        self.raft.metrics().borrow().current_leader
    }

    /// Returns a snapshot of the current Raft operational metrics.
    #[must_use]
    pub fn metrics(&self) -> RaftMetrics<u64, BasicNode> {
        self.raft.metrics().borrow().clone()
    }

    /// Performs a read-only query against the node's replicated [`Ledger`].
    pub async fn read_ledger<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&Ledger) -> R,
    {
        self.state_machine.read_ledger(f).await
    }

    /// Gracefully shuts down the node.
    ///
    /// # Errors
    ///
    /// Returns an error string if the shutdown task fails to join.
    pub async fn shutdown(&self) -> Result<(), String> {
        self.raft.shutdown().await.map_err(|e| e.to_string())
    }
}
