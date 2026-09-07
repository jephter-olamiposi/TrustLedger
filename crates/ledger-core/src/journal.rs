//! Journal events recorded by the ledger.

use serde::{Deserialize, Serialize};

use crate::account::{AccountFlags, AccountType};
use crate::amount::{Amount, Scale};
use crate::id::{AccountId, TransferId};
use crate::transfer::Transfer;

/// An event recorded in the ledger journal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LedgerEvent {
    /// An account was created.
    AccountCreated {
        /// Account ID.
        id: AccountId,
        /// Account type.
        account_type: AccountType,
        /// Account rules and limits.
        flags: AccountFlags,
        /// Currency decimal scale.
        scale: Scale,
        /// Creation timestamp.
        timestamp: u64,
    },
    /// An account was closed and rejects all new transfers.
    AccountClosed {
        /// Account ID.
        id: AccountId,
        /// Closure timestamp.
        timestamp: u64,
    },
    /// A transfer was posted.
    TransferPosted {
        /// The posted transfer.
        transfer: Transfer,
    },
    /// A pending transfer was created to hold funds.
    TransferPendingCreated {
        /// The pending transfer.
        transfer: Transfer,
    },
    /// A pending transfer was posted.
    TransferPendingPosted {
        /// ID of the pending transfer.
        pending_id: TransferId,
        /// ID of the new posted transfer.
        post_transfer_id: TransferId,
        /// Amount to post.
        amount: Amount,
        /// When this was posted.
        timestamp: u64,
    },
    /// A pending transfer was cancelled.
    TransferPendingVoided {
        /// ID of the cancelled pending transfer.
        pending_id: TransferId,
        /// Amount released back to the account.
        amount: Amount,
        /// When this was cancelled.
        timestamp: u64,
    },
}
