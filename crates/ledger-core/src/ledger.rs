//! In-memory double-entry ledger.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::account::{Account, AccountFlags, AccountType, Balance};
use crate::amount::{Amount, Scale};
use crate::error::LedgerError;
use crate::id::{AccountId, TransferId};
use crate::journal::LedgerEvent;
use crate::transfer::{Transfer, TransferState};

/// An operation in a multi-operation batch submitted to the ledger engine.
///
/// Batch operations are validated speculatively by [`Ledger::prepare_batch`].
/// Each operation maps directly to the corresponding state-machine mutation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BatchOp {
    /// Create a new account with its initial parameters and limits.
    CreateAccount {
        /// Account ID.
        id: AccountId,
        /// Account type (asset, liability, equity, revenue, expense).
        account_type: AccountType,
        /// Account rules and balance constraints.
        flags: AccountFlags,
        /// Currency decimal scale.
        scale: Scale,
        /// Sequence/wall-clock timestamp of account creation.
        timestamp: u64,
    },
    /// An immediate transfer settling funds between two accounts.
    Transfer(Transfer),
    /// A pending transfer that reserves/holds funds without settling.
    Pending(Transfer),
    /// Post (settle) a previously created pending hold.
    PostPending {
        /// The ID of the pending transfer being settled.
        pending_id: TransferId,
        /// The new transfer ID assigned to the posted settlement transfer.
        post_transfer_id: TransferId,
        /// The amount to settle (must match or be within pending reservation).
        amount: Amount,
        /// Sequence/wall-clock timestamp of this settlement.
        timestamp: u64,
    },
    /// Void (cancel) a previously created pending hold, returning reserved funds.
    VoidPending {
        /// The ID of the pending transfer being voided.
        pending_id: TransferId,
        /// Sequence/wall-clock timestamp of this cancellation.
        timestamp: u64,
    },
    /// Close an account, preventing any future debits or credits.
    CloseAccount {
        /// The ID of the account to close.
        id: AccountId,
        /// Sequence/wall-clock timestamp of the closure.
        timestamp: u64,
    },
}

impl From<Transfer> for BatchOp {
    fn from(transfer: Transfer) -> Self {
        if transfer.state() == TransferState::Pending {
            Self::Pending(transfer)
        } else {
            Self::Transfer(transfer)
        }
    }
}

/// In-memory ledger holding accounts, transfers, and the event journal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ledger {
    scale: Scale,
    accounts: BTreeMap<AccountId, Account>,
    transfers: BTreeMap<TransferId, Transfer>,
    journal: Vec<LedgerEvent>,
    last_timestamp: u64,
}

impl Ledger {
    /// Create an empty ledger with the given decimal scale.
    #[inline]
    #[must_use]
    pub fn new(scale: Scale) -> Self {
        Self {
            scale,
            accounts: BTreeMap::new(),
            transfers: BTreeMap::new(),
            journal: Vec::new(),
            last_timestamp: 0,
        }
    }

    /// The currency scale for this ledger.
    #[inline]
    #[must_use]
    pub const fn scale(&self) -> Scale {
        self.scale
    }

    /// Highest timestamp recorded so far.
    #[inline]
    #[must_use]
    pub const fn last_timestamp(&self) -> u64 {
        self.last_timestamp
    }

    /// Get an account by ID.
    ///
    /// # Errors
    ///
    /// Returns [`LedgerError::AccountNotFound`] if unregistered.
    pub fn get_account(&self, id: AccountId) -> Result<&Account, LedgerError> {
        self.accounts
            .get(&id)
            .ok_or(LedgerError::AccountNotFound(id))
    }

    /// Get a transfer by ID.
    ///
    /// # Errors
    ///
    /// Returns [`LedgerError::TransferNotFound`] if unregistered.
    pub fn get_transfer(&self, id: TransferId) -> Result<&Transfer, LedgerError> {
        self.transfers
            .get(&id)
            .ok_or(LedgerError::TransferNotFound(id))
    }

    /// Get the full journal of events.
    #[inline]
    #[must_use]
    pub fn journal(&self) -> &[LedgerEvent] {
        &self.journal
    }

    /// Returns an iterator over all recorded transfers in the ledger.
    #[inline]
    pub fn transfers(&self) -> impl Iterator<Item = &Transfer> {
        self.transfers.values()
    }

    /// Create a new account with zero balance.
    ///
    /// # Errors
    ///
    /// Returns [`LedgerError::TimestampBehindPrior`] if `timestamp` is older than the previous event.
    /// Returns [`LedgerError::AccountAlreadyExists`] if `id` already exists.
    /// Returns [`LedgerError::ScaleMismatch`] if `scale` does not match the ledger scale.
    pub fn create_account(
        &mut self,
        id: AccountId,
        account_type: AccountType,
        flags: AccountFlags,
        scale: Scale,
        timestamp: u64,
    ) -> Result<(), LedgerError> {
        self.check_timestamp(timestamp)?;

        if self.accounts.contains_key(&id) {
            return Err(LedgerError::AccountAlreadyExists(id));
        }

        if scale != self.scale {
            return Err(LedgerError::ScaleMismatch {
                expected: self.scale,
                actual: scale,
            });
        }

        let account = Account::new(id, account_type, flags, scale);
        self.accounts.insert(id, account);

        self.advance_timestamp(timestamp);
        self.journal.push(LedgerEvent::AccountCreated {
            id,
            account_type,
            flags,
            scale,
            timestamp,
        });

        #[cfg(debug_assertions)]
        {
            self.check_after_mutation()?;
        }

        Ok(())
    }

    /// Close an account. Closed accounts reject all new transfers.
    ///
    /// # Errors
    ///
    /// Returns [`LedgerError::TimestampBehindPrior`] if `timestamp` is older than the previous event.
    /// Returns [`LedgerError::AccountNotFound`] if `id` does not exist.
    /// Returns [`LedgerError::AccountClosed`] if the account is already closed.
    pub fn close_account(&mut self, id: AccountId, timestamp: u64) -> Result<(), LedgerError> {
        self.check_timestamp(timestamp)?;

        let account = self
            .accounts
            .get_mut(&id)
            .ok_or(LedgerError::AccountNotFound(id))?;

        if account.flags.is_closed {
            return Err(LedgerError::AccountClosed(id));
        }

        account.flags.is_closed = true;

        self.advance_timestamp(timestamp);
        self.journal
            .push(LedgerEvent::AccountClosed { id, timestamp });

        #[cfg(debug_assertions)]
        self.check_after_mutation()?;

        Ok(())
    }

    /// Reject timestamps behind the previous recorded event so the journal stays
    /// chronologically consistent. Equal timestamps are allowed.
    fn check_timestamp(&self, timestamp: u64) -> Result<(), LedgerError> {
        if timestamp < self.last_timestamp {
            Err(LedgerError::TimestampBehindPrior {
                timestamp,
                last: self.last_timestamp,
            })
        } else {
            Ok(())
        }
    }

    /// Advance the watermark after a timestamp has passed [`Self::check_timestamp`].
    fn advance_timestamp(&mut self, timestamp: u64) {
        self.last_timestamp = self.last_timestamp.max(timestamp);
    }

    /// Re-check all invariants after each mutation in debug/test builds, so a
    /// bug surfaces at the mutation instead of at the next explicit
    /// [`Self::verify_invariants`]. Compiled out in release builds; tests and
    /// proptests also verify after every action.
    #[cfg(debug_assertions)]
    fn check_after_mutation(&self) -> Result<(), LedgerError> {
        self.verify_invariants()
    }

    /// Create and immediately settle a transfer between two accounts.
    ///
    /// Read-only checks run first, then the balance updates (see
    /// [`Self::post_pending`] for the same ordering), so a failure can never
    /// leave a half-applied change.
    ///
    /// # Errors
    ///
    /// Returns [`LedgerError::TimestampBehindPrior`] if `transfer.timestamp` is older than the previous event.
    /// Returns [`LedgerError::TransferAlreadyExists`] if `transfer.id` already exists.
    /// Returns [`LedgerError::AccountNotFound`] if debit or credit account is missing.
    /// Returns [`LedgerError::AccountClosed`] if debit or credit account is closed.
    /// Returns [`LedgerError::InsufficientFunds`] if debit or credit violates account flags.
    /// Returns [`LedgerError::ArithmeticOverflow`] if balance summation overflows.
    /// Returns [`LedgerError::InvalidPendingLink`] or [`LedgerError::PendingLinkMissing`] if a posted transfer references an invalid hold.
    pub fn create_transfer(&mut self, transfer: Transfer) -> Result<(), LedgerError> {
        self.check_timestamp(transfer.timestamp())?;

        if transfer.state() != TransferState::Posted {
            return Err(LedgerError::InvalidTransferState {
                id: transfer.id(),
                expected: TransferState::Posted,
                actual: transfer.state(),
            });
        }

        self.validate_new_transfer(&transfer)?;
        self.validate_pending_link(&transfer)?;
        let (new_debits_posted, new_credits_posted) = {
            let debit_acc = self
                .accounts
                .get(&transfer.debit_account_id())
                .ok_or(LedgerError::AccountNotFound(transfer.debit_account_id()))?;
            debit_acc.ensure_can_debit(transfer.amount())?;
            let new_debits = debit_acc
                .balance
                .debits_posted
                .checked_add(transfer.amount())?;

            let credit_acc = self
                .accounts
                .get(&transfer.credit_account_id())
                .ok_or(LedgerError::AccountNotFound(transfer.credit_account_id()))?;
            credit_acc.ensure_can_credit(transfer.amount())?;
            let new_credits = credit_acc
                .balance
                .credits_posted
                .checked_add(transfer.amount())?;

            (new_debits, new_credits)
        };

        let debit_acc = self
            .accounts
            .get_mut(&transfer.debit_account_id())
            .ok_or(LedgerError::AccountNotFound(transfer.debit_account_id()))?;
        debit_acc.balance.debits_posted = new_debits_posted;

        let credit_acc = self
            .accounts
            .get_mut(&transfer.credit_account_id())
            .ok_or(LedgerError::AccountNotFound(transfer.credit_account_id()))?;
        credit_acc.balance.credits_posted = new_credits_posted;

        self.advance_timestamp(transfer.timestamp());
        self.transfers.insert(transfer.id(), transfer.clone());
        self.journal.push(LedgerEvent::TransferPosted { transfer });

        #[cfg(debug_assertions)]
        self.check_after_mutation()?;

        Ok(())
    }

    /// Apply a batch of posted transfers atomically in sequence.
    ///
    /// Either all transfers succeed, or the ledger is rolled back to its
    /// prior state on the first failure.
    ///
    /// # Errors
    ///
    /// Returns [`LedgerError`] on the first transfer that fails validation or balance limits.
    pub fn apply_batch(&mut self, transfers: &[Transfer]) -> Result<(), LedgerError> {
        let events: Vec<LedgerEvent> = transfers
            .iter()
            .map(|transfer| LedgerEvent::TransferPosted {
                transfer: transfer.clone(),
            })
            .collect();
        let base_journal_len = self.journal.len();
        let base_timestamp = self.last_timestamp;
        self.apply_with_undo(&events, base_journal_len, base_timestamp)?;

        #[cfg(debug_assertions)]
        self.check_after_mutation()?;

        Ok(())
    }

    /// Validate a batch of operations and return the journal events it would emit,
    /// without persisting any change to this ledger.
    ///
    /// Validation runs speculatively against live state (an O(batch) apply
    /// with no whole-ledger clone) and is then rolled back; on success this
    /// ledger is left unchanged and the returned events are exactly what a
    /// later [`Self::commit_events`] will apply. For the write-ahead flow:
    /// persist the returned events, then commit them. Committing re-validates
    /// every event, so it fails cleanly if this ledger changed since the
    /// prepare (the single-writer wal contract).
    ///
    /// # Errors
    ///
    /// Returns [`LedgerError`] on the first operation that fails validation or
    /// balance limits; on error, this ledger is unchanged.
    pub fn prepare_batch(&mut self, ops: &[BatchOp]) -> Result<Vec<LedgerEvent>, LedgerError> {
        let base_journal_len = self.journal.len();
        let base_timestamp = self.last_timestamp;

        let mut undo = Vec::new();
        for op in ops {
            let mut pre = Vec::new();
            if let Err(err) = op_pre_images(op, self, &mut pre) {
                self.rollback(&undo, base_journal_len, base_timestamp);
                return Err(err);
            }
            if let Err(err) = self.apply_batch_op(op) {
                self.rollback(&undo, base_journal_len, base_timestamp);
                return Err(err);
            }
            undo.extend(pre);
        }

        let produced = self.journal[base_journal_len..].to_vec();
        self.rollback(&undo, base_journal_len, base_timestamp);
        #[cfg(debug_assertions)]
        self.check_after_mutation()?;
        Ok(produced)
    }

    /// Speculatively prepare a sequence of transactions, where each transaction is a sequence
    /// of [`BatchOp`]s that must succeed atomically.
    ///
    /// If all operations in a transaction succeed, its speculative state modifications remain
    /// visible to subsequent transactions in the slice, and its emitted [`LedgerEvent`]s are
    /// returned in `Ok(events)`.
    ///
    /// If any operation in a transaction fails, all speculative modifications from that
    /// transaction are rolled back, its error is recorded in `Err(err)`, and execution proceeds
    /// to the next transaction.
    ///
    /// At the conclusion of this method, all speculative changes are rolled back to the ledger's
    /// pre-call state. The caller is responsible for persisting all produced events to the
    /// write-ahead log before applying them permanently via [`Self::commit_events`].
    pub fn prepare_transactions(
        &mut self,
        txs: &[Vec<BatchOp>],
    ) -> Vec<Result<Vec<LedgerEvent>, LedgerError>> {
        let base_journal_len = self.journal.len();
        let base_timestamp = self.last_timestamp;

        let mut all_undo = Vec::new();
        let mut results = Vec::with_capacity(txs.len());

        for tx in txs {
            let tx_journal_len = self.journal.len();
            let tx_timestamp = self.last_timestamp;
            let mut tx_undo = Vec::new();
            let mut tx_err = None;

            for op in tx {
                let mut pre = Vec::new();
                if let Err(err) = op_pre_images(op, self, &mut pre) {
                    tx_err = Some(err);
                    break;
                }
                if let Err(err) = self.apply_batch_op(op) {
                    tx_err = Some(err);
                    break;
                }
                tx_undo.extend(pre);
            }

            if let Some(err) = tx_err {
                self.rollback(&tx_undo, tx_journal_len, tx_timestamp);
                results.push(Err(err));
            } else {
                let events = self.journal[tx_journal_len..].to_vec();
                all_undo.extend(tx_undo);
                results.push(Ok(events));
            }
        }

        self.rollback(&all_undo, base_journal_len, base_timestamp);
        results
    }

    /// Speculatively prepare a batch of operations individually, returning a per-operation
    /// result without aborting the batch on individual failures.
    ///
    /// Operations that succeed mutate the speculative ledger state so that later operations
    /// in the same batch observe their effects (e.g. creating an account before transferring to it).
    /// If an operation fails, any speculative mutations from that operation are rolled back,
    /// its error is recorded, and execution proceeds with the next operation.
    ///
    /// At the conclusion of this method, all speculative changes are rolled back to the ledger's
    /// pre-call state. The caller is responsible for persisting all produced events to the
    /// write-ahead log before applying them permanently via [`Self::commit_events`].
    pub fn prepare_batch_results(
        &mut self,
        ops: &[BatchOp],
    ) -> Vec<Result<Vec<LedgerEvent>, LedgerError>> {
        let txs: Vec<Vec<BatchOp>> = ops.iter().map(|op| vec![op.clone()]).collect();
        self.prepare_transactions(&txs)
    }

    /// Convenience wrapper around [`Self::prepare_batch`] for immediate transfers.
    ///
    /// # Errors
    ///
    /// Returns [`LedgerError`] on the first transfer that fails validation or balance limits.
    pub fn prepare_transfers(
        &mut self,
        transfers: &[Transfer],
    ) -> Result<Vec<LedgerEvent>, LedgerError> {
        let ops: Vec<BatchOp> = transfers.iter().cloned().map(BatchOp::Transfer).collect();
        self.prepare_batch(&ops)
    }

    /// Apply a batch of journal events previously produced by
    /// [`Self::prepare_batch`].
    ///
    /// Each event is re-validated against live state (the same checks replay
    /// performs), so a stale or duplicated batch cannot be applied twice.
    /// The whole batch is atomic: a failure rolls the ledger back to its
    /// prior state.
    ///
    /// # Errors
    ///
    /// Returns [`LedgerError`] if any event fails its re-validation or
    /// application.
    pub fn commit_events(&mut self, events: &[LedgerEvent]) -> Result<(), LedgerError> {
        let base_journal_len = self.journal.len();
        let base_timestamp = self.last_timestamp;
        self.apply_with_undo(events, base_journal_len, base_timestamp)?;

        #[cfg(debug_assertions)]
        self.check_after_mutation()?;

        Ok(())
    }

    /// Apply each event, recording its pre-images so the application can be
    /// reverted. On the first failure the events applied so far are rolled
    /// back to `(base_journal_len, base_timestamp)` and the error is
    /// returned.
    ///
    /// Every mutation below is compute-then-write (ADR-0006): all checks run
    /// before any state changes, so a failing event never leaves a partial
    /// write for the rollback to clean up.
    fn apply_with_undo(
        &mut self,
        events: &[LedgerEvent],
        base_journal_len: usize,
        base_timestamp: u64,
    ) -> Result<Vec<UndoOp>, LedgerError> {
        let mut undo = Vec::new();
        for event in events {
            let mut pre = Vec::new();
            if let Err(err) = pre_images(event, self, &mut pre) {
                self.rollback(&undo, base_journal_len, base_timestamp);
                return Err(err);
            }
            if let Err(err) = self.apply_event(event) {
                self.rollback(&undo, base_journal_len, base_timestamp);
                return Err(err);
            }
            undo.extend(pre);
        }
        Ok(undo)
    }

    /// Revert every recorded pre-image in reverse order, then restore the
    /// journal length and timestamp watermark captured before the batch.
    ///
    /// Rollback must never fail midway, so reads are guarded: a missing entry
    /// means the state already diverged and is left as it is rather than
    /// panicking during recovery of an error path.
    fn rollback(&mut self, undo: &[UndoOp], base_journal_len: usize, base_timestamp: u64) {
        for op in undo.iter().rev() {
            match op {
                UndoOp::RemoveAccount { id } => {
                    self.accounts.remove(id);
                }
                UndoOp::AccountFlagsBefore { id, flags } => {
                    if let Some(account) = self.accounts.get_mut(id) {
                        account.flags = *flags;
                    }
                }
                UndoOp::AccountBalanceBefore { id, balance } => {
                    if let Some(account) = self.accounts.get_mut(id) {
                        account.balance = *balance;
                    }
                }
                UndoOp::RemoveTransfer { id } => {
                    self.transfers.remove(id);
                }
                UndoOp::TransferStateBefore { id, state } => {
                    if let Some(transfer) = self.transfers.get_mut(id) {
                        transfer.restore_state(*state);
                    }
                }
            }
        }
        self.journal.truncate(base_journal_len);
        self.last_timestamp = base_timestamp;
    }

    /// Apply one journal event against the live ledger. Shared by
    /// [`Self::replay`] and [`Self::commit_events`]; every event is
    /// re-validated instead of trusting the event's claims about balances.
    fn apply_event(&mut self, event: &LedgerEvent) -> Result<(), LedgerError> {
        match event {
            LedgerEvent::AccountCreated {
                id,
                account_type,
                flags,
                scale,
                timestamp,
            } => self.create_account(*id, *account_type, *flags, *scale, *timestamp),
            LedgerEvent::AccountClosed { id, timestamp } => self.close_account(*id, *timestamp),
            LedgerEvent::TransferPosted { transfer } => self.create_transfer(transfer.clone()),
            LedgerEvent::TransferPendingCreated { transfer } => {
                self.create_pending(transfer.clone())
            }
            LedgerEvent::TransferPendingPosted {
                pending_id,
                post_transfer_id,
                amount,
                timestamp,
            } => self.post_pending(*pending_id, *post_transfer_id, *amount, *timestamp),
            LedgerEvent::TransferPendingVoided {
                pending_id,
                amount,
                timestamp,
            } => {
                let pending = self
                    .transfers
                    .get(pending_id)
                    .map(|transfer| transfer.amount())
                    .ok_or(LedgerError::TransferNotFound(*pending_id))?;
                if pending != *amount {
                    return Err(LedgerError::JournalVoidedAmountMismatch {
                        pending_id: *pending_id,
                        pending,
                        amount: *amount,
                    });
                }
                self.void_pending(*pending_id, *timestamp)
            }
        }
    }

    /// Apply one batch operation against live ledger state during speculative batch execution.
    fn apply_batch_op(&mut self, op: &BatchOp) -> Result<(), LedgerError> {
        match op {
            BatchOp::CreateAccount {
                id,
                account_type,
                flags,
                scale,
                timestamp,
            } => self.create_account(*id, *account_type, *flags, *scale, *timestamp),
            BatchOp::Transfer(transfer) => self.create_transfer(transfer.clone()),
            BatchOp::Pending(transfer) => self.create_pending(transfer.clone()),
            BatchOp::PostPending {
                pending_id,
                post_transfer_id,
                amount,
                timestamp,
            } => self.post_pending(*pending_id, *post_transfer_id, *amount, *timestamp),
            BatchOp::VoidPending {
                pending_id,
                timestamp,
            } => self.void_pending(*pending_id, *timestamp),
            BatchOp::CloseAccount { id, timestamp } => self.close_account(*id, *timestamp),
        }
    }

    /// Create a pending transfer that holds funds without settling yet.
    ///
    /// Follows the same compute-then-write ordering as [`Self::post_pending`].
    ///
    /// # Errors
    ///
    /// Returns [`LedgerError::TimestampBehindPrior`] if `transfer.timestamp` is older than the previous event.
    /// Returns [`LedgerError::TransferAlreadyExists`] if `transfer.id` already exists.
    /// Returns [`LedgerError::AccountNotFound`] if debit or credit account is missing.
    /// Returns [`LedgerError::AccountClosed`] if debit or credit account is closed.
    /// Returns [`LedgerError::InsufficientFunds`] if the debit account cannot reserve the amount.
    /// Returns [`LedgerError::ArithmeticOverflow`] if balance summation overflows.
    pub fn create_pending(&mut self, transfer: Transfer) -> Result<(), LedgerError> {
        self.check_timestamp(transfer.timestamp())?;

        if transfer.state() != TransferState::Pending {
            return Err(LedgerError::InvalidTransferState {
                id: transfer.id(),
                expected: TransferState::Pending,
                actual: transfer.state(),
            });
        }

        self.validate_new_transfer(&transfer)?;
        let (new_debits_pending, new_credits_pending) = {
            let debit_acc = self
                .accounts
                .get(&transfer.debit_account_id())
                .ok_or(LedgerError::AccountNotFound(transfer.debit_account_id()))?;
            debit_acc.ensure_can_debit(transfer.amount())?;
            let new_debits = debit_acc
                .balance
                .debits_pending
                .checked_add(transfer.amount())?;

            let credit_acc = self
                .accounts
                .get(&transfer.credit_account_id())
                .ok_or(LedgerError::AccountNotFound(transfer.credit_account_id()))?;
            credit_acc.ensure_can_credit(transfer.amount())?;
            let new_credits = credit_acc
                .balance
                .credits_pending
                .checked_add(transfer.amount())?;

            (new_debits, new_credits)
        };

        let debit_acc = self
            .accounts
            .get_mut(&transfer.debit_account_id())
            .ok_or(LedgerError::AccountNotFound(transfer.debit_account_id()))?;
        debit_acc.balance.debits_pending = new_debits_pending;

        let credit_acc = self
            .accounts
            .get_mut(&transfer.credit_account_id())
            .ok_or(LedgerError::AccountNotFound(transfer.credit_account_id()))?;
        credit_acc.balance.credits_pending = new_credits_pending;

        self.advance_timestamp(transfer.timestamp());
        self.transfers.insert(transfer.id(), transfer.clone());
        self.journal
            .push(LedgerEvent::TransferPendingCreated { transfer });

        #[cfg(debug_assertions)]
        self.check_after_mutation()?;

        Ok(())
    }

    /// Check that a new transfer's identity and shape are legal: ID unused,
    /// accounts distinct, amount non-zero, and the pending-link shape valid.
    /// Re-checked at apply time because a transfer can also arrive via
    /// deserialization, not only through the constructors.
    fn validate_new_transfer(&self, transfer: &Transfer) -> Result<(), LedgerError> {
        if self.transfers.contains_key(&transfer.id()) {
            return Err(LedgerError::TransferAlreadyExists(transfer.id()));
        }

        if transfer.debit_account_id() == transfer.credit_account_id() {
            return Err(LedgerError::DebitCreditAccountSame(
                transfer.debit_account_id(),
            ));
        }

        if transfer.amount().is_zero() {
            return Err(LedgerError::ZeroAmountTransfer(transfer.id()));
        }

        transfer.validate_shape()?;

        Ok(())
    }

    /// Check a posted transfer's hold link: if it names a `pending_id`, the hold
    /// must exist and already be [`TransferState::Posted`]. Rejected at apply
    /// time so a forged or deserialized transfer cannot slip past
    /// [`Self::verify_structure`].
    fn validate_pending_link(&self, transfer: &Transfer) -> Result<(), LedgerError> {
        let Some(pending_id) = transfer.pending_id() else {
            return Ok(());
        };

        let hold = self
            .transfers
            .get(&pending_id)
            .ok_or(LedgerError::PendingLinkMissing {
                transfer_id: transfer.id(),
                pending_id,
            })?;

        if hold.state() != TransferState::Posted {
            return Err(LedgerError::InvalidPendingLink {
                transfer_id: transfer.id(),
                pending_id,
                actual: hold.state(),
            });
        }

        Ok(())
    }

    /// Post a pending transfer. If amount is less than the hold, the rest is released.
    ///
    /// # Errors
    ///
    /// Returns [`LedgerError::TimestampBehindPrior`] if `timestamp` is older than the previous event.
    /// Returns [`LedgerError::TransferNotFound`] if `pending_id` does not exist.
    /// Returns [`LedgerError::TransferAlreadyExists`] if `post_transfer_id` already exists.
    /// Returns [`LedgerError::InvalidTransferState`] if transfer is not in [`TransferState::Pending`].
    /// Returns [`LedgerError::PartialAmountExceedsPending`] if `amount` exceeds original reservation.
    /// Returns [`LedgerError::ZeroAmountTransfer`] if `amount` is zero.
    pub fn post_pending(
        &mut self,
        pending_id: TransferId,
        post_transfer_id: TransferId,
        amount: Amount,
        timestamp: u64,
    ) -> Result<(), LedgerError> {
        self.check_timestamp(timestamp)?;

        if self.transfers.contains_key(&post_transfer_id) {
            return Err(LedgerError::TransferAlreadyExists(post_transfer_id));
        }

        let (debit_account_id, credit_account_id, pending_amount) = {
            let pending_transfer = self
                .transfers
                .get(&pending_id)
                .ok_or(LedgerError::TransferNotFound(pending_id))?;

            if pending_transfer.state() != TransferState::Pending {
                return Err(LedgerError::InvalidTransferState {
                    id: pending_id,
                    expected: TransferState::Pending,
                    actual: pending_transfer.state(),
                });
            }

            if amount > pending_transfer.amount() {
                return Err(LedgerError::PartialAmountExceedsPending {
                    transfer_id: pending_id,
                    requested: amount,
                    pending: pending_transfer.amount(),
                });
            }

            if amount.is_zero() {
                return Err(LedgerError::ZeroAmountTransfer(post_transfer_id));
            }

            (
                pending_transfer.debit_account_id(),
                pending_transfer.credit_account_id(),
                pending_transfer.amount(),
            )
        };

        let new_debits_pending = {
            let debit_account = self
                .accounts
                .get(&debit_account_id)
                .ok_or(LedgerError::AccountNotFound(debit_account_id))?;
            (
                debit_account
                    .balance
                    .debits_pending
                    .checked_sub(pending_amount)?,
                debit_account.balance.debits_posted.checked_add(amount)?,
            )
        };

        let new_credits_pending = {
            let credit_account = self
                .accounts
                .get(&credit_account_id)
                .ok_or(LedgerError::AccountNotFound(credit_account_id))?;
            (
                credit_account
                    .balance
                    .credits_pending
                    .checked_sub(pending_amount)?,
                credit_account.balance.credits_posted.checked_add(amount)?,
            )
        };

        let posted_transfer = Transfer::new_settlement(
            post_transfer_id,
            debit_account_id,
            credit_account_id,
            amount,
            pending_id,
            timestamp,
        )?;

        let debit_acc = self
            .accounts
            .get_mut(&debit_account_id)
            .ok_or(LedgerError::AccountNotFound(debit_account_id))?;
        debit_acc.balance.debits_pending = new_debits_pending.0;
        debit_acc.balance.debits_posted = new_debits_pending.1;

        let credit_acc = self
            .accounts
            .get_mut(&credit_account_id)
            .ok_or(LedgerError::AccountNotFound(credit_account_id))?;
        credit_acc.balance.credits_pending = new_credits_pending.0;
        credit_acc.balance.credits_posted = new_credits_pending.1;

        let pending_transfer = self
            .transfers
            .get_mut(&pending_id)
            .ok_or(LedgerError::TransferNotFound(pending_id))?;
        pending_transfer.transition_to(TransferState::Posted)?;

        self.advance_timestamp(timestamp);
        self.transfers.insert(post_transfer_id, posted_transfer);

        self.journal.push(LedgerEvent::TransferPendingPosted {
            pending_id,
            post_transfer_id,
            amount,
            timestamp,
        });

        #[cfg(debug_assertions)]
        self.check_after_mutation()?;

        Ok(())
    }

    /// Cancel a pending transfer and release the held funds.
    ///
    /// # Errors
    ///
    /// Returns [`LedgerError::TimestampBehindPrior`] if `timestamp` is older than the previous event.
    /// Returns [`LedgerError::TransferNotFound`] if `pending_id` does not exist.
    /// Returns [`LedgerError::InvalidTransferState`] if transfer is not in [`TransferState::Pending`].
    pub fn void_pending(
        &mut self,
        pending_id: TransferId,
        timestamp: u64,
    ) -> Result<(), LedgerError> {
        self.check_timestamp(timestamp)?;

        let (debit_account_id, credit_account_id, pending_amount) = {
            let pending_transfer = self
                .transfers
                .get(&pending_id)
                .ok_or(LedgerError::TransferNotFound(pending_id))?;

            if pending_transfer.state() != TransferState::Pending {
                return Err(LedgerError::InvalidTransferState {
                    id: pending_id,
                    expected: TransferState::Pending,
                    actual: pending_transfer.state(),
                });
            }

            (
                pending_transfer.debit_account_id(),
                pending_transfer.credit_account_id(),
                pending_transfer.amount(),
            )
        };

        let new_debits_pending = {
            let debit_account = self
                .accounts
                .get(&debit_account_id)
                .ok_or(LedgerError::AccountNotFound(debit_account_id))?;
            debit_account
                .balance
                .debits_pending
                .checked_sub(pending_amount)?
        };

        let new_credits_pending = {
            let credit_account = self
                .accounts
                .get(&credit_account_id)
                .ok_or(LedgerError::AccountNotFound(credit_account_id))?;
            credit_account
                .balance
                .credits_pending
                .checked_sub(pending_amount)?
        };

        let debit_acc = self
            .accounts
            .get_mut(&debit_account_id)
            .ok_or(LedgerError::AccountNotFound(debit_account_id))?;
        debit_acc.balance.debits_pending = new_debits_pending;

        let credit_acc = self
            .accounts
            .get_mut(&credit_account_id)
            .ok_or(LedgerError::AccountNotFound(credit_account_id))?;
        credit_acc.balance.credits_pending = new_credits_pending;

        let pending_transfer = self
            .transfers
            .get_mut(&pending_id)
            .ok_or(LedgerError::TransferNotFound(pending_id))?;
        pending_transfer.transition_to(TransferState::Voided)?;

        self.advance_timestamp(timestamp);
        self.journal.push(LedgerEvent::TransferPendingVoided {
            pending_id,
            amount: pending_amount,
            timestamp,
        });

        #[cfg(debug_assertions)]
        self.check_after_mutation()?;

        Ok(())
    }

    /// Check total-debits-equal-total-credits conservation and that the transfer
    /// index structurally backs the account balances.
    ///
    /// # Errors
    ///
    /// Returns [`LedgerError::ConservationViolation`] if debit and credit sums diverge.
    /// Returns [`LedgerError::AccountNotFound`] if a transfer references an unknown account.
    /// Returns [`LedgerError::PendingLinkMissing`] if a transfer references a missing hold.
    /// Returns [`LedgerError::PendingBalanceMismatch`] if an account's reservations disagree with its transfers.
    /// Returns [`LedgerError::InvalidPendingLink`] if a transfer's hold reference is invalid.
    pub fn verify_invariants(&self) -> Result<(), LedgerError> {
        let mut total_debits_posted = Amount::ZERO;
        let mut total_credits_posted = Amount::ZERO;
        let mut total_debits_pending = Amount::ZERO;
        let mut total_credits_pending = Amount::ZERO;

        for account in self.accounts.values() {
            total_debits_posted = total_debits_posted.checked_add(account.balance.debits_posted)?;
            total_credits_posted =
                total_credits_posted.checked_add(account.balance.credits_posted)?;
            total_debits_pending =
                total_debits_pending.checked_add(account.balance.debits_pending)?;
            total_credits_pending =
                total_credits_pending.checked_add(account.balance.credits_pending)?;
        }

        if total_debits_posted != total_credits_posted {
            return Err(LedgerError::ConservationViolation {
                total_debits: total_debits_posted,
                total_credits: total_credits_posted,
            });
        }

        if total_debits_pending != total_credits_pending {
            return Err(LedgerError::ConservationViolation {
                total_debits: total_debits_pending,
                total_credits: total_credits_pending,
            });
        }

        self.verify_structure()?;

        Ok(())
    }

    /// Verify the transfer index is consistent with account balances: every
    /// account's pending reservations are exactly the sum of its pending
    /// transfers, and posted/voided transfers link to holds in a legal state.
    fn verify_structure(&self) -> Result<(), LedgerError> {
        let mut held_debits: BTreeMap<AccountId, Amount> = BTreeMap::new();
        let mut held_credits: BTreeMap<AccountId, Amount> = BTreeMap::new();

        for transfer in self.transfers.values() {
            if !self.accounts.contains_key(&transfer.debit_account_id()) {
                return Err(LedgerError::AccountNotFound(transfer.debit_account_id()));
            }
            if !self.accounts.contains_key(&transfer.credit_account_id()) {
                return Err(LedgerError::AccountNotFound(transfer.credit_account_id()));
            }

            match transfer.state() {
                TransferState::Pending => {
                    let debit_total = held_debits
                        .entry(transfer.debit_account_id())
                        .or_insert(Amount::ZERO);
                    *debit_total = debit_total.checked_add(transfer.amount())?;
                    let credit_total = held_credits
                        .entry(transfer.credit_account_id())
                        .or_insert(Amount::ZERO);
                    *credit_total = credit_total.checked_add(transfer.amount())?;
                }
                TransferState::Posted => self.validate_pending_link(transfer)?,
                TransferState::Voided => {
                    if let Some(pending_id) = transfer.pending_id() {
                        return Err(LedgerError::InvalidPendingLink {
                            transfer_id: transfer.id(),
                            pending_id,
                            actual: transfer.state(),
                        });
                    }
                }
            }
        }

        for (account_id, account) in &self.accounts {
            let held = held_debits.get(account_id).copied().unwrap_or(Amount::ZERO);
            if account.balance.debits_pending != held {
                return Err(LedgerError::PendingBalanceMismatch {
                    account_id: *account_id,
                    balance_pending: account.balance.debits_pending,
                    transfers_pending: held,
                });
            }
            let held = held_credits
                .get(account_id)
                .copied()
                .unwrap_or(Amount::ZERO);
            if account.balance.credits_pending != held {
                return Err(LedgerError::PendingBalanceMismatch {
                    account_id: *account_id,
                    balance_pending: account.balance.credits_pending,
                    transfers_pending: held,
                });
            }
        }

        Ok(())
    }

    /// Rebuild ledger state by replaying a list of journal events.
    ///
    /// # Errors
    ///
    /// Returns [`LedgerError`] if any journal event fails application.
    pub fn replay(scale: Scale, events: &[LedgerEvent]) -> Result<Self, LedgerError> {
        let mut ledger = Self::new(scale);

        for event in events {
            ledger.apply_event(event)?;
        }

        Ok(ledger)
    }
}

/// A pre-image of one side effect, recorded before its event applies so the
/// application can be reverted exactly.
#[derive(Clone, Debug, PartialEq, Eq)]
enum UndoOp {
    /// The event created an account; rollback removes it.
    RemoveAccount {
        /// Account inserted by the event.
        id: AccountId,
    },
    /// The event changed an account's flags; rollback restores them.
    AccountFlagsBefore {
        /// Account whose flags changed.
        id: AccountId,
        /// Flags captured before the event.
        flags: AccountFlags,
    },
    /// The event changed an account's balances; rollback restores them.
    AccountBalanceBefore {
        /// Account whose balances changed.
        id: AccountId,
        /// Balances captured before the event.
        balance: Balance,
    },
    /// The event inserted a transfer; rollback removes it.
    RemoveTransfer {
        /// Transfer inserted by the event.
        id: TransferId,
    },
    /// The event transitioned a transfer; rollback restores its prior state.
    TransferStateBefore {
        /// Transfer whose state changed.
        id: TransferId,
        /// State captured before the event.
        state: TransferState,
    },
}

/// Capture the pre-images of every side effect `event` will make.
///
/// The captured ops are only kept if the event applies successfully, so an
/// event that fails its validation (and therefore changes nothing) never has
/// its ops rolled back.
fn pre_images(
    event: &LedgerEvent,
    ledger: &Ledger,
    undo: &mut Vec<UndoOp>,
) -> Result<(), LedgerError> {
    match event {
        LedgerEvent::AccountCreated { id, .. } => {
            undo.push(UndoOp::RemoveAccount { id: *id });
        }
        LedgerEvent::AccountClosed { id, .. } => {
            let account = ledger
                .accounts
                .get(id)
                .ok_or(LedgerError::AccountNotFound(*id))?;
            undo.push(UndoOp::AccountFlagsBefore {
                id: *id,
                flags: account.flags,
            });
        }
        LedgerEvent::TransferPosted { transfer } => {
            push_balance_before(ledger, transfer.debit_account_id(), undo)?;
            push_balance_before(ledger, transfer.credit_account_id(), undo)?;
            undo.push(UndoOp::RemoveTransfer { id: transfer.id() });
        }
        LedgerEvent::TransferPendingCreated { transfer } => {
            push_balance_before(ledger, transfer.debit_account_id(), undo)?;
            push_balance_before(ledger, transfer.credit_account_id(), undo)?;
            undo.push(UndoOp::RemoveTransfer { id: transfer.id() });
        }
        LedgerEvent::TransferPendingPosted {
            pending_id,
            post_transfer_id,
            ..
        } => {
            let pending = ledger
                .transfers
                .get(pending_id)
                .ok_or(LedgerError::TransferNotFound(*pending_id))?;
            push_balance_before(ledger, pending.debit_account_id(), undo)?;
            push_balance_before(ledger, pending.credit_account_id(), undo)?;
            undo.push(UndoOp::TransferStateBefore {
                id: *pending_id,
                state: pending.state(),
            });
            undo.push(UndoOp::RemoveTransfer {
                id: *post_transfer_id,
            });
        }
        LedgerEvent::TransferPendingVoided { pending_id, .. } => {
            let pending = ledger
                .transfers
                .get(pending_id)
                .ok_or(LedgerError::TransferNotFound(*pending_id))?;
            push_balance_before(ledger, pending.debit_account_id(), undo)?;
            push_balance_before(ledger, pending.credit_account_id(), undo)?;
            undo.push(UndoOp::TransferStateBefore {
                id: *pending_id,
                state: pending.state(),
            });
        }
    }
    Ok(())
}

/// Capture the pre-images of every side effect `op` will make during speculative
/// batch execution.
fn op_pre_images(op: &BatchOp, ledger: &Ledger, undo: &mut Vec<UndoOp>) -> Result<(), LedgerError> {
    match op {
        BatchOp::CreateAccount { id, .. } => {
            if ledger.accounts.contains_key(id) {
                return Err(LedgerError::AccountAlreadyExists(*id));
            }
            undo.push(UndoOp::RemoveAccount { id: *id });
            Ok(())
        }
        BatchOp::CloseAccount { id, .. } => {
            let account = ledger
                .accounts
                .get(id)
                .ok_or(LedgerError::AccountNotFound(*id))?;
            undo.push(UndoOp::AccountFlagsBefore {
                id: *id,
                flags: account.flags,
            });
            Ok(())
        }
        BatchOp::Transfer(transfer) | BatchOp::Pending(transfer) => {
            push_balance_before(ledger, transfer.debit_account_id(), undo)?;
            push_balance_before(ledger, transfer.credit_account_id(), undo)?;
            undo.push(UndoOp::RemoveTransfer { id: transfer.id() });
            Ok(())
        }
        BatchOp::PostPending {
            pending_id,
            post_transfer_id,
            ..
        } => {
            let pending = ledger
                .transfers
                .get(pending_id)
                .ok_or(LedgerError::TransferNotFound(*pending_id))?;
            push_balance_before(ledger, pending.debit_account_id(), undo)?;
            push_balance_before(ledger, pending.credit_account_id(), undo)?;
            undo.push(UndoOp::TransferStateBefore {
                id: *pending_id,
                state: pending.state(),
            });
            undo.push(UndoOp::RemoveTransfer {
                id: *post_transfer_id,
            });
            Ok(())
        }
        BatchOp::VoidPending { pending_id, .. } => {
            let pending = ledger
                .transfers
                .get(pending_id)
                .ok_or(LedgerError::TransferNotFound(*pending_id))?;
            push_balance_before(ledger, pending.debit_account_id(), undo)?;
            push_balance_before(ledger, pending.credit_account_id(), undo)?;
            undo.push(UndoOp::TransferStateBefore {
                id: *pending_id,
                state: pending.state(),
            });
            Ok(())
        }
    }
}

/// Push the account's balances as an [`UndoOp::AccountBalanceBefore`] pre-image.
fn push_balance_before(
    ledger: &Ledger,
    id: AccountId,
    undo: &mut Vec<UndoOp>,
) -> Result<(), LedgerError> {
    let account = ledger
        .accounts
        .get(&id)
        .ok_or(LedgerError::AccountNotFound(id))?;
    undo.push(UndoOp::AccountBalanceBefore {
        id,
        balance: account.balance,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transfer::Transfer;

    fn seeded() -> Ledger {
        let mut ledger = Ledger::new(Scale::usdc());
        ledger
            .create_account(
                AccountId::new(1),
                AccountType::Asset,
                AccountFlags::bank_asset(),
                Scale::usdc(),
                0,
            )
            .unwrap();
        ledger
            .create_account(
                AccountId::new(2),
                AccountType::Liability,
                AccountFlags::customer(),
                Scale::usdc(),
                0,
            )
            .unwrap();
        ledger
    }

    #[test]
    fn create_transfer_rejects_link_to_missing_hold() {
        let mut ledger = seeded();
        let forged = Transfer::new_settlement(
            TransferId::new(9),
            AccountId::new(1),
            AccountId::new(2),
            Amount::new(10),
            TransferId::new(999),
            1,
        )
        .unwrap();

        assert_eq!(
            ledger.create_transfer(forged).unwrap_err(),
            LedgerError::PendingLinkMissing {
                transfer_id: TransferId::new(9),
                pending_id: TransferId::new(999),
            },
        );
    }

    #[test]
    fn create_transfer_rejects_link_to_unposted_hold() {
        let mut ledger = seeded();
        let hold = Transfer::new_pending(
            TransferId::new(3),
            AccountId::new(1),
            AccountId::new(2),
            Amount::new(100),
            1,
        )
        .unwrap();
        ledger.create_pending(hold).unwrap();

        let forged = Transfer::new_settlement(
            TransferId::new(9),
            AccountId::new(1),
            AccountId::new(2),
            Amount::new(10),
            TransferId::new(3),
            2,
        )
        .unwrap();

        assert_eq!(
            ledger.create_transfer(forged).unwrap_err(),
            LedgerError::InvalidPendingLink {
                transfer_id: TransferId::new(9),
                pending_id: TransferId::new(3),
                actual: TransferState::Pending,
            },
        );
    }

    #[test]
    fn commit_events_failure_rolls_back_the_whole_batch() {
        let mut ledger = seeded();
        let before = ledger.clone();
        let events = vec![
            LedgerEvent::TransferPosted {
                transfer: Transfer::new_immediate(
                    TransferId::new(7),
                    AccountId::new(1),
                    AccountId::new(2),
                    Amount::new(10),
                    1,
                )
                .unwrap(),
            },
            LedgerEvent::TransferPosted {
                transfer: Transfer::new_immediate(
                    TransferId::new(7),
                    AccountId::new(1),
                    AccountId::new(2),
                    Amount::new(10),
                    1,
                )
                .unwrap(),
            },
        ];

        assert!(ledger.commit_events(&events).is_err());
        assert_eq!(
            ledger, before,
            "failed commit must leave the ledger untouched"
        );
    }

    #[test]
    fn prepare_batch_validates_without_persisting_state() {
        let mut ledger = seeded();
        let before = ledger.clone();
        let events = ledger
            .prepare_batch(&[BatchOp::Transfer(
                Transfer::new_immediate(
                    TransferId::new(7),
                    AccountId::new(1),
                    AccountId::new(2),
                    Amount::new(10),
                    1,
                )
                .unwrap(),
            )])
            .unwrap();

        assert!(matches!(&events[..], [LedgerEvent::TransferPosted { .. }]));
        assert_eq!(
            ledger, before,
            "prepare must roll back its speculative apply"
        );

        let mut applied = before.clone();
        applied.commit_events(&events).unwrap();
        assert_eq!(ledger, before);
        assert_eq!(
            applied.journal(),
            &before
                .journal()
                .iter()
                .cloned()
                .chain(events)
                .collect::<Vec<_>>(),
        );
    }

    #[test]
    fn failing_prepare_batch_leaves_the_ledger_untouched() {
        let mut ledger = seeded();
        let before = ledger.clone();
        let first = BatchOp::Transfer(
            Transfer::new_immediate(
                TransferId::new(7),
                AccountId::new(1),
                AccountId::new(2),
                Amount::new(10),
                1,
            )
            .unwrap(),
        );
        let duplicate = BatchOp::Transfer(
            Transfer::new_immediate(
                TransferId::new(7),
                AccountId::new(1),
                AccountId::new(2),
                Amount::new(10),
                1,
            )
            .unwrap(),
        );

        assert!(ledger.prepare_batch(&[first, duplicate]).is_err());
        assert_eq!(
            ledger, before,
            "failed prepare must leave the ledger untouched"
        );
    }

    #[test]
    fn prepare_batch_supports_mixed_operations_and_atomic_rollback() {
        let mut ledger = Ledger::new(Scale::usdc());
        let before = ledger.clone();

        let ops = vec![
            BatchOp::CreateAccount {
                id: AccountId::new(10),
                account_type: AccountType::Asset,
                flags: AccountFlags::bank_asset(),
                scale: Scale::usdc(),
                timestamp: 1,
            },
            BatchOp::CreateAccount {
                id: AccountId::new(20),
                account_type: AccountType::Liability,
                flags: AccountFlags::customer(),
                scale: Scale::usdc(),
                timestamp: 1,
            },
            BatchOp::Pending(
                Transfer::new_pending(
                    TransferId::new(100),
                    AccountId::new(10),
                    AccountId::new(20),
                    Amount::new(500),
                    2,
                )
                .unwrap(),
            ),
            BatchOp::PostPending {
                pending_id: TransferId::new(100),
                post_transfer_id: TransferId::new(101),
                amount: Amount::new(500),
                timestamp: 3,
            },
        ];

        let events = ledger.prepare_batch(&ops).expect("mixed batch prepare");
        assert_eq!(events.len(), 4);
        assert_eq!(
            ledger, before,
            "speculative prepare must not alter ledger state"
        );

        ledger.commit_events(&events).expect("commit mixed batch");
        assert_eq!(ledger.journal().len(), 4);
        assert_eq!(
            ledger
                .get_account(AccountId::new(10))
                .unwrap()
                .balance
                .debits_posted,
            Amount::new(500)
        );
        assert_eq!(
            ledger
                .get_account(AccountId::new(20))
                .unwrap()
                .balance
                .credits_posted,
            Amount::new(500)
        );
    }

    #[test]
    fn every_event_kind_rolls_back_atomically_when_a_later_event_fails() {
        struct Case {
            name: &'static str,
            make: fn() -> (Ledger, LedgerEvent),
        }

        let failing_event = || LedgerEvent::AccountCreated {
            id: AccountId::new(1),
            account_type: AccountType::Asset,
            flags: AccountFlags::bank_asset(),
            scale: Scale::usdc(),
            timestamp: 10_000_000,
        };

        let cases: &[Case] = &[
            Case {
                name: "AccountCreated",
                make: || {
                    let before = seeded();
                    let event = LedgerEvent::AccountCreated {
                        id: AccountId::new(3),
                        account_type: AccountType::Liability,
                        flags: AccountFlags::customer(),
                        scale: Scale::usdc(),
                        timestamp: 1,
                    };
                    (before, event)
                },
            },
            Case {
                name: "AccountClosed",
                make: || {
                    let mut before = seeded();
                    before
                        .create_account(
                            AccountId::new(3),
                            AccountType::Liability,
                            AccountFlags::customer(),
                            Scale::usdc(),
                            1,
                        )
                        .unwrap();
                    let event = LedgerEvent::AccountClosed {
                        id: AccountId::new(3),
                        timestamp: 2,
                    };
                    (before, event)
                },
            },
            Case {
                name: "TransferPosted",
                make: || {
                    let before = seeded();
                    let event = LedgerEvent::TransferPosted {
                        transfer: Transfer::new_immediate(
                            TransferId::new(7),
                            AccountId::new(1),
                            AccountId::new(2),
                            Amount::new(10),
                            1,
                        )
                        .unwrap(),
                    };
                    (before, event)
                },
            },
            Case {
                name: "TransferPendingCreated",
                make: || {
                    let before = seeded();
                    let event = LedgerEvent::TransferPendingCreated {
                        transfer: Transfer::new_pending(
                            TransferId::new(7),
                            AccountId::new(1),
                            AccountId::new(2),
                            Amount::new(10),
                            1,
                        )
                        .unwrap(),
                    };
                    (before, event)
                },
            },
            Case {
                name: "TransferPendingPosted",
                make: || {
                    let mut before = seeded();
                    before
                        .create_pending(
                            Transfer::new_pending(
                                TransferId::new(3),
                                AccountId::new(1),
                                AccountId::new(2),
                                Amount::new(100),
                                1,
                            )
                            .unwrap(),
                        )
                        .unwrap();
                    let event = LedgerEvent::TransferPendingPosted {
                        pending_id: TransferId::new(3),
                        post_transfer_id: TransferId::new(8),
                        amount: Amount::new(100),
                        timestamp: 2,
                    };
                    (before, event)
                },
            },
            Case {
                name: "TransferPendingVoided",
                make: || {
                    let mut before = seeded();
                    before
                        .create_pending(
                            Transfer::new_pending(
                                TransferId::new(3),
                                AccountId::new(1),
                                AccountId::new(2),
                                Amount::new(100),
                                1,
                            )
                            .unwrap(),
                        )
                        .unwrap();
                    let event = LedgerEvent::TransferPendingVoided {
                        pending_id: TransferId::new(3),
                        amount: Amount::new(100),
                        timestamp: 2,
                    };
                    (before, event)
                },
            },
        ];

        for case in cases {
            let (before, kind_event) = (case.make)();

            let mut alone = before.clone();
            alone
                .commit_events(std::slice::from_ref(&kind_event))
                .unwrap_or_else(|err| panic!("{}: kind event must apply: {err}", case.name));

            let mut attempt = before.clone();
            let pair = [kind_event, failing_event()];
            let err = attempt
                .commit_events(&pair)
                .expect_err(&format!("{}: second event must fail", case.name));
            assert_eq!(
                attempt, before,
                "{}: failed batch ({err}) must roll back the kind event",
                case.name
            );
        }
    }

    #[test]
    fn prepare_batch_results_isolates_failures_and_preserves_intra_batch_visibility() {
        let mut ledger = Ledger::new(Scale::usdc());
        let ops = vec![
            BatchOp::CreateAccount {
                id: AccountId::new(1),
                account_type: AccountType::Asset,
                flags: AccountFlags::bank_asset(),
                scale: Scale::usdc(),
                timestamp: 1,
            },
            BatchOp::CreateAccount {
                id: AccountId::new(2),
                account_type: AccountType::Liability,
                flags: AccountFlags::customer(),
                scale: Scale::usdc(),
                timestamp: 2,
            },
            BatchOp::Transfer(
                Transfer::new_immediate(
                    TransferId::new(10),
                    AccountId::new(1),
                    AccountId::new(2),
                    Amount::new(500),
                    3,
                )
                .unwrap(),
            ),
            BatchOp::Transfer(
                Transfer::new_immediate(
                    TransferId::new(11),
                    AccountId::new(2),
                    AccountId::new(1),
                    Amount::new(999_999),
                    4,
                )
                .unwrap(),
            ),
            BatchOp::Transfer(
                Transfer::new_immediate(
                    TransferId::new(12),
                    AccountId::new(99),
                    AccountId::new(1),
                    Amount::new(10),
                    5,
                )
                .unwrap(),
            ),
            BatchOp::CloseAccount {
                id: AccountId::new(2),
                timestamp: 6,
            },
        ];

        let results = ledger.prepare_batch_results(&ops);
        assert_eq!(results.len(), 6);
        assert!(results[0].is_ok());
        assert!(results[1].is_ok());
        assert!(results[2].is_ok());
        assert!(matches!(
            results[3].as_ref().unwrap_err(),
            LedgerError::InsufficientFunds { .. }
        ));
        assert!(matches!(
            results[4].as_ref().unwrap_err(),
            LedgerError::AccountNotFound(_)
        ));
        assert!(results[5].is_ok());

        assert_eq!(ledger.journal().len(), 0);
        assert!(ledger.get_account(AccountId::new(1)).is_err());

        let successful_events: Vec<LedgerEvent> = results
            .into_iter()
            .filter_map(Result::ok)
            .flatten()
            .collect();
        ledger
            .commit_events(&successful_events)
            .expect("commit successful events");

        assert_eq!(ledger.journal().len(), 4);
        let acc2 = ledger.get_account(AccountId::new(2)).unwrap();
        assert!(acc2.flags.is_closed);
        assert_eq!(acc2.balance.credits_posted, Amount::new(500));
    }

    #[test]
    fn prepare_transactions_isolates_multi_op_atomic_failures() {
        let mut ledger = Ledger::new(Scale::usdc());
        let txs = vec![
            vec![BatchOp::CreateAccount {
                id: AccountId::new(1),
                account_type: AccountType::Asset,
                flags: AccountFlags::bank_asset(),
                scale: Scale::usdc(),
                timestamp: 1,
            }],
            vec![
                BatchOp::CreateAccount {
                    id: AccountId::new(2),
                    account_type: AccountType::Liability,
                    flags: AccountFlags::customer(),
                    scale: Scale::usdc(),
                    timestamp: 2,
                },
                BatchOp::Transfer(
                    Transfer::new_immediate(
                        TransferId::new(10),
                        AccountId::new(1),
                        AccountId::new(2),
                        Amount::new(1_000),
                        3,
                    )
                    .unwrap(),
                ),
            ],
            vec![
                BatchOp::CreateAccount {
                    id: AccountId::new(3),
                    account_type: AccountType::Liability,
                    flags: AccountFlags::customer(),
                    scale: Scale::usdc(),
                    timestamp: 4,
                },
                BatchOp::Transfer(
                    Transfer::new_immediate(
                        TransferId::new(11),
                        AccountId::new(2),
                        AccountId::new(3),
                        Amount::new(999_999),
                        5,
                    )
                    .unwrap(),
                ),
            ],
            vec![BatchOp::Transfer(
                Transfer::new_immediate(
                    TransferId::new(12),
                    AccountId::new(2),
                    AccountId::new(1),
                    Amount::new(500),
                    6,
                )
                .unwrap(),
            )],
        ];

        let results = ledger.prepare_transactions(&txs);
        assert_eq!(results.len(), 4);
        assert!(results[0].is_ok());
        assert!(results[1].is_ok());
        assert!(matches!(
            results[2].as_ref().unwrap_err(),
            LedgerError::InsufficientFunds { .. }
        ));
        assert!(results[3].is_ok());

        assert_eq!(ledger.journal().len(), 0);

        let successful_events: Vec<LedgerEvent> = results
            .into_iter()
            .filter_map(Result::ok)
            .flatten()
            .collect();
        ledger
            .commit_events(&successful_events)
            .expect("commit events");

        assert!(ledger.get_account(AccountId::new(1)).is_ok());
        assert!(ledger.get_account(AccountId::new(2)).is_ok());
        assert!(ledger.get_account(AccountId::new(3)).is_err());
        assert_eq!(
            ledger
                .get_account(AccountId::new(2))
                .unwrap()
                .balance
                .credits_posted,
            Amount::new(1_000)
        );
        assert_eq!(
            ledger
                .get_account(AccountId::new(2))
                .unwrap()
                .balance
                .debits_posted,
            Amount::new(500)
        );
    }
}
