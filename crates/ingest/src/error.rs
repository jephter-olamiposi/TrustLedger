//! Ingress errors and gRPC status mappings.

use ledger_core::error::LedgerError;
use wal::WalError;

/// Errors produced during request ingestion, micro-batch accumulation, or execution.
#[derive(Clone, Debug, thiserror::Error)]
pub enum IngestError {
    /// Domain error reported by the ledger state machine.
    #[error("ledger error: {0}")]
    Ledger(#[from] LedgerError),

    /// Disk I/O or checksum error from the write-ahead log.
    #[error("write-ahead log error: {0}")]
    Wal(String),

    /// Serialization error when encoding journal events for persistence.
    #[error("codec error: {0}")]
    Codec(String),

    /// Ingress queue reached maximum capacity; requests are shed immediately.
    #[error("ingress queue saturated: backpressure shedding load")]
    QueueSaturated,

    /// Engine channel disconnected or background writer task terminated.
    #[error("engine channel closed")]
    ChannelClosed,

    /// Client request payload violates protocol shape or invariant constraints.
    #[error("invalid request input: {0}")]
    InvalidInput(String),

    /// Internal server error during batch processing or persistence.
    #[error("internal server error: {0}")]
    Internal(String),
}

impl From<WalError> for IngestError {
    fn from(err: WalError) -> Self {
        Self::Wal(err.to_string())
    }
}

impl From<ledger_core::codec::CodecError> for IngestError {
    fn from(err: ledger_core::codec::CodecError) -> Self {
        Self::Codec(err.to_string())
    }
}

impl From<IngestError> for tonic::Status {
    fn from(err: IngestError) -> Self {
        match err {
            IngestError::QueueSaturated => tonic::Status::resource_exhausted(
                "ingress queue saturated: backpressure shedding load",
            ),
            IngestError::ChannelClosed => tonic::Status::unavailable("ingress engine queue closed"),
            IngestError::InvalidInput(msg) => tonic::Status::invalid_argument(msg),
            IngestError::Wal(wal_err) => tonic::Status::internal(wal_err),
            IngestError::Codec(codec_err) => tonic::Status::internal(codec_err),
            IngestError::Internal(msg) => tonic::Status::internal(msg),
            IngestError::Ledger(ledger_err) => match ledger_err {
                LedgerError::InsufficientFunds { .. }
                | LedgerError::AccountClosed(_)
                | LedgerError::InvalidTransferState { .. }
                | LedgerError::PartialAmountExceedsPending { .. } => {
                    tonic::Status::failed_precondition(ledger_err.to_string())
                }
                LedgerError::AccountNotFound(_) | LedgerError::TransferNotFound(_) => {
                    tonic::Status::not_found(ledger_err.to_string())
                }
                LedgerError::AccountAlreadyExists(_) | LedgerError::TransferAlreadyExists(_) => {
                    tonic::Status::already_exists(ledger_err.to_string())
                }
                LedgerError::DebitCreditAccountSame(_)
                | LedgerError::ZeroAmountTransfer(_)
                | LedgerError::ScaleMismatch { .. }
                | LedgerError::InvalidScale(_)
                | LedgerError::InvalidIdentifier(_)
                | LedgerError::TimestampBehindPrior { .. }
                | LedgerError::InvalidPendingLink { .. }
                | LedgerError::PendingLinkMissing { .. } => {
                    tonic::Status::invalid_argument(ledger_err.to_string())
                }
                LedgerError::ArithmeticOverflow | LedgerError::SignedBalanceOverflow => {
                    tonic::Status::out_of_range(ledger_err.to_string())
                }
                LedgerError::ConservationViolation { .. }
                | LedgerError::JournalVoidedAmountMismatch { .. }
                | LedgerError::PendingBalanceMismatch { .. }
                | LedgerError::DivisionByZero => tonic::Status::internal(ledger_err.to_string()),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ledger_core::id::{AccountId, TransferId};
    use ledger_core::transfer::TransferState;

    #[test]
    fn status_code_mappings() {
        assert_eq!(
            tonic::Status::from(IngestError::QueueSaturated).code(),
            tonic::Code::ResourceExhausted
        );
        assert_eq!(
            tonic::Status::from(IngestError::ChannelClosed).code(),
            tonic::Code::Unavailable
        );
        assert_eq!(
            tonic::Status::from(IngestError::InvalidInput("bad input".into())).code(),
            tonic::Code::InvalidArgument
        );
        assert_eq!(
            tonic::Status::from(IngestError::Wal("wal disk fail".into())).code(),
            tonic::Code::Internal
        );
        assert_eq!(
            tonic::Status::from(IngestError::Codec("codec error".into())).code(),
            tonic::Code::Internal
        );
        assert_eq!(
            tonic::Status::from(IngestError::Internal("internal crash".into())).code(),
            tonic::Code::Internal
        );

        assert_eq!(
            tonic::Status::from(IngestError::Ledger(LedgerError::InsufficientFunds {
                account_id: AccountId::new(1),
                available: ledger_core::amount::Amount::new(0),
                requested: ledger_core::amount::Amount::new(100),
            }))
            .code(),
            tonic::Code::FailedPrecondition
        );
        assert_eq!(
            tonic::Status::from(IngestError::Ledger(LedgerError::AccountNotFound(
                AccountId::new(1)
            )))
            .code(),
            tonic::Code::NotFound
        );
        assert_eq!(
            tonic::Status::from(IngestError::Ledger(LedgerError::AccountAlreadyExists(
                AccountId::new(1)
            )))
            .code(),
            tonic::Code::AlreadyExists
        );
        assert_eq!(
            tonic::Status::from(IngestError::Ledger(LedgerError::AccountClosed(
                AccountId::new(1)
            )))
            .code(),
            tonic::Code::FailedPrecondition
        );
        assert_eq!(
            tonic::Status::from(IngestError::Ledger(LedgerError::InvalidTransferState {
                id: TransferId::new(1),
                expected: TransferState::Pending,
                actual: TransferState::Posted,
            }))
            .code(),
            tonic::Code::FailedPrecondition
        );
        assert_eq!(
            tonic::Status::from(IngestError::Ledger(LedgerError::DebitCreditAccountSame(
                AccountId::new(1)
            )))
            .code(),
            tonic::Code::InvalidArgument
        );
        assert_eq!(
            tonic::Status::from(IngestError::Ledger(LedgerError::ArithmeticOverflow)).code(),
            tonic::Code::OutOfRange
        );
    }

    #[test]
    fn ingest_error_is_cloneable() {
        let err = IngestError::Wal("disk full".into());
        let cloned = err.clone();
        assert_eq!(err.to_string(), cloned.to_string());
    }
}
