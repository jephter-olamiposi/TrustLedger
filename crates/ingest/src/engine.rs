//! Single-writer micro-batch accumulator engine.

use std::time::Duration;

use ledger_core::account::Account;
use ledger_core::id::{AccountId, TransferId};
use ledger_core::journal::LedgerEvent;
use ledger_core::transfer::Transfer;
use ledger_core::{BatchOp, Ledger};
use tokio::sync::oneshot;
use wal::Wal;

use crate::error::IngestError;
use crate::queue::{IngestCommand, IngestQueue, MutationResult, TargetEntity};

/// Configuration options for the micro-batch engine.
#[derive(Clone, Debug)]
pub struct EngineConfig {
    /// Maximum number of incoming commands to accumulate before committing a batch.
    pub max_batch_size: usize,
    /// Maximum duration to wait while accumulating a batch.
    pub max_batch_delay: Duration,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            max_batch_size: 512,
            max_batch_delay: Duration::from_millis(2),
        }
    }
}

enum Responder {
    Single {
        target: TargetEntity,
        respond_to: oneshot::Sender<Result<MutationResult, IngestError>>,
    },
    Batch {
        count: u32,
        respond_to: oneshot::Sender<Result<u32, IngestError>>,
    },
}

impl Responder {
    fn respond_err(self, err: IngestError) {
        match self {
            Self::Single { respond_to, .. } => {
                let _ = respond_to.send(Err(err));
            }
            Self::Batch { respond_to, .. } => {
                let _ = respond_to.send(Err(err));
            }
        }
    }

    fn respond_ok(self, ledger: &Ledger) {
        match self {
            Self::Single { target, respond_to } => match target {
                TargetEntity::Account(id) => {
                    let res = ledger
                        .get_account(id)
                        .cloned()
                        .map_err(IngestError::Ledger)
                        .map(MutationResult::Account);
                    let _ = respond_to.send(res);
                }
                TargetEntity::Transfer(id) => {
                    let res = ledger
                        .get_transfer(id)
                        .cloned()
                        .map_err(IngestError::Ledger)
                        .map(MutationResult::Transfer);
                    let _ = respond_to.send(res);
                }
            },
            Self::Batch { count, respond_to } => {
                let _ = respond_to.send(Ok(count));
            }
        }
    }
}

enum QueryCommand {
    GetAccount(AccountId, oneshot::Sender<Result<Account, IngestError>>),
    GetTransfer(TransferId, oneshot::Sender<Result<Transfer, IngestError>>),
}

/// Single-writer ledger engine that drains ingress commands, groups mutations into
/// micro-batches, persists them to the write-ahead log with group commits, and updates state.
pub struct Engine {
    ledger: Ledger,
    wal: Wal,
    receiver: tokio::sync::mpsc::Receiver<IngestCommand>,
    config: EngineConfig,
}

impl Engine {
    /// Create a new single-writer engine paired with a bounded ingress queue.
    #[must_use]
    pub fn new(
        ledger: Ledger,
        wal: Wal,
        config: EngineConfig,
        queue_capacity: usize,
    ) -> (Self, IngestQueue) {
        let (sender, receiver) = tokio::sync::mpsc::channel(queue_capacity);
        let queue = IngestQueue::new(sender);
        let engine = Self {
            ledger,
            wal,
            receiver,
            config,
        };
        (engine, queue)
    }

    /// Run the single-writer accumulator loop until all senders disconnect.
    ///
    /// # Errors
    ///
    /// Returns [`IngestError`] if an unrecoverable persistence or state machine error occurs.
    pub async fn run(&mut self) -> Result<(), IngestError> {
        loop {
            let Some(first_cmd) = self.receiver.recv().await else {
                break;
            };

            let mut batch = Vec::with_capacity(self.config.max_batch_size);
            batch.push(first_cmd);

            let deadline = tokio::time::Instant::now() + self.config.max_batch_delay;
            while batch.len() < self.config.max_batch_size {
                let now = tokio::time::Instant::now();
                if now >= deadline {
                    break;
                }
                let remaining = deadline - now;
                match self.receiver.try_recv() {
                    Ok(cmd) => batch.push(cmd),
                    Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                        match tokio::time::timeout(remaining, self.receiver.recv()).await {
                            Ok(Some(cmd)) => batch.push(cmd),
                            Ok(None) | Err(_) => break,
                        }
                    }
                    Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => break,
                }
            }

            self.process_batch(batch)?;
        }

        Ok(())
    }

    fn process_batch(&mut self, batch: Vec<IngestCommand>) -> Result<(), IngestError> {
        if batch.is_empty() {
            return Ok(());
        }

        let mut watermark = self.ledger.last_timestamp();
        let mut tx_ops: Vec<Vec<BatchOp>> = Vec::new();
        let mut responders: Vec<Responder> = Vec::new();
        let mut early_errors: Vec<(Responder, IngestError)> = Vec::new();
        let mut queries: Vec<QueryCommand> = Vec::new();

        for cmd in batch {
            match cmd {
                IngestCommand::Mutate { op, respond_to } => {
                    let target = op.target();
                    let responder = Responder::Single { target, respond_to };
                    match op.into_batch_op(&mut watermark) {
                        Ok(batch_op) => {
                            tx_ops.push(vec![batch_op]);
                            responders.push(responder);
                        }
                        Err(err) => {
                            early_errors.push((responder, err));
                        }
                    }
                }
                IngestCommand::ApplyBatch { ops, respond_to } => {
                    let count = ops.len() as u32;
                    let responder = Responder::Batch { count, respond_to };
                    let mut batch_ops = Vec::with_capacity(ops.len());
                    let mut batch_err = None;

                    for op in ops {
                        match op.into_batch_op(&mut watermark) {
                            Ok(batch_op) => batch_ops.push(batch_op),
                            Err(err) => {
                                batch_err = Some(err);
                                break;
                            }
                        }
                    }

                    if let Some(err) = batch_err {
                        early_errors.push((responder, err));
                    } else {
                        tx_ops.push(batch_ops);
                        responders.push(responder);
                    }
                }
                IngestCommand::GetAccount { id, respond_to } => {
                    queries.push(QueryCommand::GetAccount(id, respond_to));
                }
                IngestCommand::GetTransfer { id, respond_to } => {
                    queries.push(QueryCommand::GetTransfer(id, respond_to));
                }
            }
        }

        for (resp, err) in early_errors {
            resp.respond_err(err);
        }

        if !tx_ops.is_empty() {
            let results = self.ledger.prepare_transactions(&tx_ops);
            let mut successful_responders = Vec::new();
            let mut successful_event_batches = Vec::new();

            for (resp, res) in responders.into_iter().zip(results) {
                match res {
                    Ok(events) => {
                        successful_responders.push(resp);
                        successful_event_batches.push(events);
                    }
                    Err(err) => {
                        resp.respond_err(IngestError::Ledger(err));
                    }
                }
            }

            if !successful_event_batches.is_empty() {
                let mut payloads = Vec::with_capacity(successful_event_batches.len());
                for events in &successful_event_batches {
                    let payload = ledger_core::codec::encode_events(events)?;
                    payloads.push(payload);
                }

                // Invariant: group commit persists all prepared events in a single append and fsync.
                if let Err(wal_err) = self.wal.append_batch(&payloads) {
                    let ingest_err = IngestError::from(wal_err);
                    for resp in successful_responders {
                        resp.respond_err(ingest_err.clone());
                    }
                    return Err(ingest_err);
                }

                let all_events: Vec<LedgerEvent> =
                    successful_event_batches.into_iter().flatten().collect();
                if let Err(ledger_err) = self.ledger.commit_events(&all_events) {
                    let ingest_err = IngestError::Ledger(ledger_err);
                    for resp in successful_responders {
                        resp.respond_err(ingest_err.clone());
                    }
                    return Err(ingest_err);
                }

                for resp in successful_responders {
                    resp.respond_ok(&self.ledger);
                }
            }
        }

        for query in queries {
            match query {
                QueryCommand::GetAccount(id, respond_to) => {
                    let res = self
                        .ledger
                        .get_account(id)
                        .cloned()
                        .map_err(IngestError::Ledger);
                    let _ = respond_to.send(res);
                }
                QueryCommand::GetTransfer(id, respond_to) => {
                    let res = self
                        .ledger
                        .get_transfer(id)
                        .cloned()
                        .map_err(IngestError::Ledger);
                    let _ = respond_to.send(res);
                }
            }
        }

        Ok(())
    }
}
