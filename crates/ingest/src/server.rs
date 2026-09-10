//! gRPC service implementation for ledger operations.

use ledger_core::account::{Account, AccountFlags, AccountType};
use ledger_core::amount::{Amount, Scale};
use ledger_core::error::LedgerError;
use ledger_core::id::{AccountId, TransferId};
use ledger_core::transfer::{Transfer, TransferState};
use tokio::sync::oneshot;
use tonic::{Request, Response, Status};

use crate::error::IngestError;
use crate::proto;
use crate::queue::{IngestCommand, IngestOp, IngestQueue, MutationResult};

fn amount_to_proto(amount: Amount, scale: u32) -> proto::Amount {
    let val = amount.as_u128();
    proto::Amount {
        units: (val & 0xFFFF_FFFF_FFFF_FFFF) as u64,
        scale,
        units_high: (val >> 64) as u64,
    }
}

fn proto_to_account_type(val: i32) -> Result<AccountType, IngestError> {
    match proto::AccountType::try_from(val) {
        Ok(proto::AccountType::Asset) => Ok(AccountType::Asset),
        Ok(proto::AccountType::Liability) => Ok(AccountType::Liability),
        Ok(proto::AccountType::Equity) => Ok(AccountType::Equity),
        Ok(proto::AccountType::Revenue) => Ok(AccountType::Revenue),
        Ok(proto::AccountType::Expense) => Ok(AccountType::Expense),
        _ => Err(IngestError::InvalidInput(
            "unknown or unspecified account type".into(),
        )),
    }
}

fn account_type_to_proto(account_type: AccountType) -> i32 {
    match account_type {
        AccountType::Asset => proto::AccountType::Asset as i32,
        AccountType::Liability => proto::AccountType::Liability as i32,
        AccountType::Equity => proto::AccountType::Equity as i32,
        AccountType::Revenue => proto::AccountType::Revenue as i32,
        AccountType::Expense => proto::AccountType::Expense as i32,
    }
}

fn proto_to_flags(proto: Option<proto::AccountFlags>) -> AccountFlags {
    match proto {
        Some(f) => AccountFlags {
            debits_must_not_exceed_credits: f.debits_must_not_exceed_credits,
            credits_must_not_exceed_debits: f.credits_must_not_exceed_debits,
            is_closed: f.is_closed,
        },
        None => AccountFlags::default(),
    }
}

fn flags_to_proto(flags: AccountFlags) -> proto::AccountFlags {
    proto::AccountFlags {
        debits_must_not_exceed_credits: flags.debits_must_not_exceed_credits,
        credits_must_not_exceed_debits: flags.credits_must_not_exceed_debits,
        is_closed: flags.is_closed,
    }
}

fn balance_to_proto(balance: ledger_core::account::Balance, scale: u32) -> proto::Balance {
    proto::Balance {
        debits_posted: Some(amount_to_proto(balance.debits_posted, scale)),
        credits_posted: Some(amount_to_proto(balance.credits_posted, scale)),
        debits_pending: Some(amount_to_proto(balance.debits_pending, scale)),
        credits_pending: Some(amount_to_proto(balance.credits_pending, scale)),
    }
}

fn account_to_proto(account: Account) -> proto::Account {
    let scale_u32 = account.scale.as_u8() as u32;
    proto::Account {
        id: account.id.as_u128() as u64,
        account_type: account_type_to_proto(account.account_type),
        flags: Some(flags_to_proto(account.flags)),
        scale: scale_u32,
        balance: Some(balance_to_proto(account.balance, scale_u32)),
    }
}

fn transfer_state_to_proto(state: TransferState) -> i32 {
    match state {
        TransferState::Pending => proto::TransferState::Pending as i32,
        TransferState::Posted => proto::TransferState::Posted as i32,
        TransferState::Voided => proto::TransferState::Voided as i32,
    }
}

fn transfer_to_proto(transfer: Transfer, scale: u32) -> proto::Transfer {
    proto::Transfer {
        id: transfer.id().as_u128() as u64,
        debit_account_id: transfer.debit_account_id().as_u128() as u64,
        credit_account_id: transfer.credit_account_id().as_u128() as u64,
        amount: Some(amount_to_proto(transfer.amount(), scale)),
        state: transfer_state_to_proto(transfer.state()),
        pending_id: transfer.pending_id().map(|id| id.as_u128() as u64),
        timestamp: transfer.timestamp(),
    }
}

/// Tonic gRPC service implementation for the TrustLedger network ingress.
#[derive(Clone, Debug)]
pub struct LedgerServer {
    queue: IngestQueue,
    scale: Scale,
}

impl LedgerServer {
    /// Create a new gRPC service instance with the given bounded queue and currency scale.
    #[must_use]
    pub fn new(queue: IngestQueue, scale: Scale) -> Self {
        Self { queue, scale }
    }

    fn parse_amount(&self, proto: Option<proto::Amount>) -> Result<Amount, IngestError> {
        let p = proto.ok_or_else(|| IngestError::InvalidInput("amount is required".into()))?;
        if p.scale != 0 {
            let scale_u8 = u8::try_from(p.scale)
                .map_err(|_| IngestError::InvalidInput("amount scale exceeds u8 limit".into()))?;
            let scale = Scale::new(scale_u8).map_err(IngestError::Ledger)?;
            if scale != self.scale {
                return Err(IngestError::Ledger(LedgerError::ScaleMismatch {
                    expected: self.scale,
                    actual: scale,
                }));
            }
        }
        let units = ((p.units_high as u128) << 64) | (p.units as u128);
        Ok(Amount::new(units))
    }
}

#[tonic::async_trait]
impl proto::ledger_service_server::LedgerService for LedgerServer {
    async fn create_account(
        &self,
        request: Request<proto::CreateAccountRequest>,
    ) -> Result<Response<proto::CreateAccountResponse>, Status> {
        let req = request.into_inner();
        let id = AccountId::new(req.id as u128);
        let account_type = proto_to_account_type(req.account_type)?;
        let flags = proto_to_flags(req.flags);
        let scale_u8 = u8::try_from(req.scale)
            .map_err(|_| IngestError::InvalidInput("scale exceeds u8 limit".into()))?;
        let scale = Scale::new(scale_u8).map_err(IngestError::Ledger)?;

        let (tx, rx) = oneshot::channel();
        self.queue.submit(IngestCommand::Mutate {
            op: IngestOp::CreateAccount {
                id,
                account_type,
                flags,
                scale,
                timestamp: req.timestamp,
            },
            respond_to: tx,
        })?;

        let res = rx.await.map_err(|_| IngestError::ChannelClosed)??;
        let account = match res {
            MutationResult::Account(account) => account,
            MutationResult::Transfer(_) => {
                return Err(IngestError::Internal("unexpected mutation result".into()).into())
            }
        };
        Ok(Response::new(proto::CreateAccountResponse {
            account: Some(account_to_proto(account)),
        }))
    }

    async fn create_transfer(
        &self,
        request: Request<proto::CreateTransferRequest>,
    ) -> Result<Response<proto::CreateTransferResponse>, Status> {
        let req = request.into_inner();
        let id = TransferId::new(req.id as u128);
        let debit_account_id = AccountId::new(req.debit_account_id as u128);
        let credit_account_id = AccountId::new(req.credit_account_id as u128);
        let amount = self.parse_amount(req.amount)?;

        let (tx, rx) = oneshot::channel();
        self.queue.submit(IngestCommand::Mutate {
            op: IngestOp::CreateTransfer {
                id,
                debit_account_id,
                credit_account_id,
                amount,
                timestamp: req.timestamp,
            },
            respond_to: tx,
        })?;

        let res = rx.await.map_err(|_| IngestError::ChannelClosed)??;
        let transfer = match res {
            MutationResult::Transfer(transfer) => transfer,
            MutationResult::Account(_) => {
                return Err(IngestError::Internal("unexpected mutation result".into()).into())
            }
        };
        let scale_u32 = self.scale.as_u8() as u32;
        Ok(Response::new(proto::CreateTransferResponse {
            transfer: Some(transfer_to_proto(transfer, scale_u32)),
        }))
    }

    async fn create_pending(
        &self,
        request: Request<proto::CreatePendingRequest>,
    ) -> Result<Response<proto::CreatePendingResponse>, Status> {
        let req = request.into_inner();
        let id = TransferId::new(req.id as u128);
        let debit_account_id = AccountId::new(req.debit_account_id as u128);
        let credit_account_id = AccountId::new(req.credit_account_id as u128);
        let amount = self.parse_amount(req.amount)?;

        let (tx, rx) = oneshot::channel();
        self.queue.submit(IngestCommand::Mutate {
            op: IngestOp::CreatePending {
                id,
                debit_account_id,
                credit_account_id,
                amount,
                timestamp: req.timestamp,
            },
            respond_to: tx,
        })?;

        let res = rx.await.map_err(|_| IngestError::ChannelClosed)??;
        let transfer = match res {
            MutationResult::Transfer(transfer) => transfer,
            MutationResult::Account(_) => {
                return Err(IngestError::Internal("unexpected mutation result".into()).into())
            }
        };
        let scale_u32 = self.scale.as_u8() as u32;
        Ok(Response::new(proto::CreatePendingResponse {
            transfer: Some(transfer_to_proto(transfer, scale_u32)),
        }))
    }

    async fn post_pending(
        &self,
        request: Request<proto::PostPendingRequest>,
    ) -> Result<Response<proto::PostPendingResponse>, Status> {
        let req = request.into_inner();
        let pending_id = TransferId::new(req.pending_id as u128);
        let post_transfer_id = TransferId::new(req.post_transfer_id as u128);
        let amount = self.parse_amount(req.amount)?;

        let (tx, rx) = oneshot::channel();
        self.queue.submit(IngestCommand::Mutate {
            op: IngestOp::PostPending {
                pending_id,
                post_transfer_id,
                amount,
                timestamp: req.timestamp,
            },
            respond_to: tx,
        })?;

        let res = rx.await.map_err(|_| IngestError::ChannelClosed)??;
        let transfer = match res {
            MutationResult::Transfer(transfer) => transfer,
            MutationResult::Account(_) => {
                return Err(IngestError::Internal("unexpected mutation result".into()).into())
            }
        };
        let scale_u32 = self.scale.as_u8() as u32;
        Ok(Response::new(proto::PostPendingResponse {
            transfer: Some(transfer_to_proto(transfer, scale_u32)),
        }))
    }

    async fn void_pending(
        &self,
        request: Request<proto::VoidPendingRequest>,
    ) -> Result<Response<proto::VoidPendingResponse>, Status> {
        let req = request.into_inner();
        let pending_id = TransferId::new(req.pending_id as u128);

        let (tx, rx) = oneshot::channel();
        self.queue.submit(IngestCommand::Mutate {
            op: IngestOp::VoidPending {
                pending_id,
                timestamp: req.timestamp,
            },
            respond_to: tx,
        })?;

        let res = rx.await.map_err(|_| IngestError::ChannelClosed)??;
        let transfer = match res {
            MutationResult::Transfer(transfer) => transfer,
            MutationResult::Account(_) => {
                return Err(IngestError::Internal("unexpected mutation result".into()).into())
            }
        };
        let scale_u32 = self.scale.as_u8() as u32;
        Ok(Response::new(proto::VoidPendingResponse {
            transfer: Some(transfer_to_proto(transfer, scale_u32)),
        }))
    }

    async fn get_account(
        &self,
        request: Request<proto::GetAccountRequest>,
    ) -> Result<Response<proto::GetAccountResponse>, Status> {
        let req = request.into_inner();
        let id = AccountId::new(req.id as u128);

        let (tx, rx) = oneshot::channel();
        self.queue
            .submit(IngestCommand::GetAccount { id, respond_to: tx })?;

        let account = rx.await.map_err(|_| IngestError::ChannelClosed)??;
        Ok(Response::new(proto::GetAccountResponse {
            account: Some(account_to_proto(account)),
        }))
    }

    async fn get_transfer(
        &self,
        request: Request<proto::GetTransferRequest>,
    ) -> Result<Response<proto::GetTransferResponse>, Status> {
        let req = request.into_inner();
        let id = TransferId::new(req.id as u128);

        let (tx, rx) = oneshot::channel();
        self.queue
            .submit(IngestCommand::GetTransfer { id, respond_to: tx })?;

        let transfer = rx.await.map_err(|_| IngestError::ChannelClosed)??;
        let scale_u32 = self.scale.as_u8() as u32;
        Ok(Response::new(proto::GetTransferResponse {
            transfer: Some(transfer_to_proto(transfer, scale_u32)),
        }))
    }

    async fn apply_batch(
        &self,
        request: Request<proto::ApplyBatchRequest>,
    ) -> Result<Response<proto::ApplyBatchResponse>, Status> {
        let req = request.into_inner();
        let mut ops = Vec::with_capacity(req.operations.len());

        for op in req.operations {
            let Some(operation) = op.operation else {
                return Err(IngestError::InvalidInput("empty batch operation".into()).into());
            };
            match operation {
                proto::batch_operation::Operation::CreateAccount(c) => {
                    let id = AccountId::new(c.id as u128);
                    let account_type = proto_to_account_type(c.account_type)?;
                    let flags = proto_to_flags(c.flags);
                    let scale_u8 = u8::try_from(c.scale)
                        .map_err(|_| IngestError::InvalidInput("scale exceeds u8 limit".into()))?;
                    let scale = Scale::new(scale_u8).map_err(IngestError::Ledger)?;
                    ops.push(IngestOp::CreateAccount {
                        id,
                        account_type,
                        flags,
                        scale,
                        timestamp: c.timestamp,
                    });
                }
                proto::batch_operation::Operation::CreateTransfer(t) => {
                    let id = TransferId::new(t.id as u128);
                    let debit_account_id = AccountId::new(t.debit_account_id as u128);
                    let credit_account_id = AccountId::new(t.credit_account_id as u128);
                    let amount = self.parse_amount(t.amount)?;
                    ops.push(IngestOp::CreateTransfer {
                        id,
                        debit_account_id,
                        credit_account_id,
                        amount,
                        timestamp: t.timestamp,
                    });
                }
                proto::batch_operation::Operation::CreatePending(p) => {
                    let id = TransferId::new(p.id as u128);
                    let debit_account_id = AccountId::new(p.debit_account_id as u128);
                    let credit_account_id = AccountId::new(p.credit_account_id as u128);
                    let amount = self.parse_amount(p.amount)?;
                    ops.push(IngestOp::CreatePending {
                        id,
                        debit_account_id,
                        credit_account_id,
                        amount,
                        timestamp: p.timestamp,
                    });
                }
                proto::batch_operation::Operation::PostPending(pp) => {
                    let pending_id = TransferId::new(pp.pending_id as u128);
                    let post_transfer_id = TransferId::new(pp.post_transfer_id as u128);
                    let amount = self.parse_amount(pp.amount)?;
                    ops.push(IngestOp::PostPending {
                        pending_id,
                        post_transfer_id,
                        amount,
                        timestamp: pp.timestamp,
                    });
                }
                proto::batch_operation::Operation::VoidPending(vp) => {
                    let pending_id = TransferId::new(vp.pending_id as u128);
                    ops.push(IngestOp::VoidPending {
                        pending_id,
                        timestamp: vp.timestamp,
                    });
                }
            }
        }

        let (tx, rx) = oneshot::channel();
        self.queue.submit(IngestCommand::ApplyBatch {
            ops,
            respond_to: tx,
        })?;

        let count = rx.await.map_err(|_| IngestError::ChannelClosed)??;
        Ok(Response::new(proto::ApplyBatchResponse {
            applied_count: count,
        }))
    }
}
