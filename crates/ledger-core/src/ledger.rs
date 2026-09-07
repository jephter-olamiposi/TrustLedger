//! In-memory double-entry ledger.

use std::collections::BTreeMap;

use crate::account::{Account, AccountFlags, AccountType};
use crate::amount::{Amount, Scale};
use crate::error::LedgerError;
use crate::id::{AccountId, TransferId};
use crate::journal::LedgerEvent;
use crate::transfer::{Transfer, TransferState};

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

    /// Re-run the full invariant check right after a mutation in debug/test
    /// builds, so a drifting invariant is caught the moment it lands instead of
    /// at the next explicit [`Self::verify_invariants`]. Compiled out entirely
    /// in release builds; the ledger's own tests and proptests additionally
    /// verify after every action.
    #[cfg(debug_assertions)]
    fn check_after_mutation(&self) -> Result<(), LedgerError> {
        self.verify_invariants()
    }

    /// Create and immediately settle a transfer between two accounts.
    ///
    /// Follows the same compute-then-write ordering as [`Self::post_pending`]:
    /// every fallible operation completes in a read-only pass before the first
    /// write, so a failure can never leave a half-applied balance change.
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

        // Compute phase: read-only, every `?` completes before the first write.
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

        // Write phase: pure assignments; nothing here can fail.
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
    /// Either all transfers succeed, or the entire batch rolls back on failure.
    ///
    /// # Errors
    ///
    /// Returns [`LedgerError`] on the first transfer that fails validation or balance limits.
    pub fn apply_batch(&mut self, transfers: &[Transfer]) -> Result<(), LedgerError> {
        let mut checkpoint = self.clone();
        for transfer in transfers {
            checkpoint.create_transfer(transfer.clone())?;
        }
        *self = checkpoint;

        #[cfg(debug_assertions)]
        self.check_after_mutation()?;

        Ok(())
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

        // Compute phase: read-only, every `?` completes before the first write.
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

        // Write phase: pure assignments; nothing here can fail.
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

    /// Check that a new transfer's identity and shape are valid: the ID is unused,
    /// the account pair is distinct, the amount is non-zero, and the
    /// cross-field shape is legal. The ledger re-checks these at the mutation
    /// boundary because a `Transfer` can also arrive via deserialization,
    /// bypassing the constructors.
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

    /// Check a posted transfer's hold link: if it claims a `pending_id`, the
    /// hold must exist and be [`TransferState::Posted`] already. Mirrors the
    /// structural check so a forged or deserialized transfer is rejected the
    /// moment it is applied rather than at a later `verify_invariants`.
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

        // Extract primitives first so the pending entry is not borrowed while we
        // hold mutable access to the accounts map.
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

        // Compute phase: every fallible operation runs before the first write, so
        // a failure can never leave a half-applied balance change. All lookups
        // target accounts validated to exist when the hold was created.
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

        // Write phase: pure assignments; nothing here can fail.
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

        // Record the state transition only after all effects are committed.
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
            match event {
                LedgerEvent::AccountCreated {
                    id,
                    account_type,
                    flags,
                    scale,
                    timestamp,
                } => {
                    ledger.create_account(*id, *account_type, *flags, *scale, *timestamp)?;
                }
                LedgerEvent::AccountClosed { id, timestamp } => {
                    ledger.close_account(*id, *timestamp)?;
                }
                LedgerEvent::TransferPosted { transfer } => {
                    ledger.create_transfer(transfer.clone())?;
                }
                LedgerEvent::TransferPendingCreated { transfer } => {
                    ledger.create_pending(transfer.clone())?;
                }
                LedgerEvent::TransferPendingPosted {
                    pending_id,
                    post_transfer_id,
                    amount,
                    timestamp,
                } => {
                    ledger.post_pending(*pending_id, *post_transfer_id, *amount, *timestamp)?;
                }
                LedgerEvent::TransferPendingVoided {
                    pending_id,
                    amount,
                    timestamp,
                } => {
                    // The voided amount is a duplicate of the pending
                    // reservation; a mismatch means a corrupted journal that
                    // must fail loudly instead of replaying to wrong state.
                    let pending = ledger
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
                    ledger.void_pending(*pending_id, *timestamp)?;
                }
            }
        }

        Ok(ledger)
    }
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
}
