//! Ledger errors.

use thiserror::Error;

use crate::amount::{Amount, Scale};
use crate::id::{AccountId, TransferId};
use crate::transfer::TransferState;

/// Errors returned by the ledger.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum LedgerError {
    /// Invalid ID string.
    #[error("invalid identifier: {0}")]
    InvalidIdentifier(String),

    /// Scale exceeds maximum allowed (18).
    #[error("scale factor {0} exceeds maximum precision limit")]
    InvalidScale(u8),

    /// Math overflow.
    #[error("arithmetic overflow encountered during monetary calculation")]
    ArithmeticOverflow,

    /// Division by zero.
    #[error("division by zero attempted")]
    DivisionByZero,

    /// Balance is too large to convert to signed integer.
    #[error(
        "balance component exceeds i128::MAX and cannot be represented as a signed net balance"
    )]
    SignedBalanceOverflow,

    /// Account does not have enough funds.
    #[error(
        "account {account_id} has insufficient funds: requested {requested}, available {available}"
    )]
    InsufficientFunds {
        /// Account ID.
        account_id: AccountId,
        /// Requested amount.
        requested: Amount,
        /// Available balance.
        available: Amount,
    },

    /// Account already exists.
    #[error("account already exists: {0}")]
    AccountAlreadyExists(AccountId),

    /// Account not found.
    #[error("account not found: {0}")]
    AccountNotFound(AccountId),

    /// Account is closed.
    #[error("account {0} is closed")]
    AccountClosed(AccountId),

    /// Transfer already exists.
    #[error("transfer already exists: {0}")]
    TransferAlreadyExists(TransferId),

    /// Transfer not found.
    #[error("transfer not found: {0}")]
    TransferNotFound(TransferId),

    /// Transfer is in the wrong state for this action.
    #[error("transfer {id} is in invalid state: expected {expected:?}, actual {actual:?}")]
    InvalidTransferState {
        /// Transfer ID.
        id: TransferId,
        /// Expected state.
        expected: TransferState,
        /// Actual state.
        actual: TransferState,
    },

    /// Debit and credit accounts cannot be the same.
    #[error("debit and credit accounts must be distinct: {0}")]
    DebitCreditAccountSame(AccountId),

    /// Transfer amount must be greater than zero.
    #[error("transfer {0} amount must be greater than zero")]
    ZeroAmountTransfer(TransferId),

    /// Currency scale does not match.
    #[error("scale mismatch: expected {expected}, actual {actual}")]
    ScaleMismatch {
        /// Expected scale.
        expected: Scale,
        /// Given scale.
        actual: Scale,
    },

    /// Total debits do not equal total credits.
    #[error("financial conservation violation: total debits {total_debits} != total credits {total_credits}")]
    ConservationViolation {
        /// Total debits.
        total_debits: Amount,
        /// Total credits.
        total_credits: Amount,
    },

    /// Posted amount is greater than the pending hold.
    #[error("posted amount {requested} exceeds pending reservation {pending} for transfer {transfer_id}")]
    PartialAmountExceedsPending {
        /// Pending transfer ID.
        transfer_id: TransferId,
        /// Amount to post.
        requested: Amount,
        /// Amount held.
        pending: Amount,
    },

    /// An event timestamp is older than a previously recorded one.
    #[error("timestamp {timestamp} is behind the prior event timestamp {last}")]
    TimestampBehindPrior {
        /// Offending timestamp.
        timestamp: u64,
        /// Highest timestamp recorded so far.
        last: u64,
    },

    /// A journal event contradicts the state it is meant to reproduce.
    #[error("journal corruption: voided amount {amount} does not match pending reservation {pending} for transfer {pending_id}")]
    JournalVoidedAmountMismatch {
        /// Pending transfer ID.
        pending_id: TransferId,
        /// Pending reservation derived from ledger state.
        pending: Amount,
        /// Amount recorded on the void event.
        amount: Amount,
    },

    /// An account's pending balance disagrees with the transfers holding it.
    #[error("account {account_id} pending balance {balance_pending} does not match pending transfers {transfers_pending}")]
    PendingBalanceMismatch {
        /// Account ID.
        account_id: AccountId,
        /// Balance field on the account.
        balance_pending: Amount,
        /// Sum of transfers that should back the field.
        transfers_pending: Amount,
    },

    /// A transfer carries a reference to a pending hold that cannot back it.
    #[error("transfer {transfer_id} references pending hold {pending_id} in state {actual:?}, which cannot back the reference")]
    InvalidPendingLink {
        /// Transfer carrying the reference.
        transfer_id: TransferId,
        /// Referenced pending hold.
        pending_id: TransferId,
        /// State of the referenced hold.
        actual: TransferState,
    },

    /// A transfer carries a reference to a pending hold that does not exist.
    #[error("transfer {transfer_id} references missing pending hold {pending_id}")]
    PendingLinkMissing {
        /// Transfer carrying the reference.
        transfer_id: TransferId,
        /// Referenced pending hold.
        pending_id: TransferId,
    },
}
