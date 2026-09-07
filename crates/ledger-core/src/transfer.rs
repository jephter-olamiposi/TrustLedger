//! Transfers and their states.

use serde::{Deserialize, Serialize};

use crate::amount::Amount;
use crate::error::LedgerError;
use crate::id::{AccountId, TransferId};

/// State of a transfer.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TransferState {
    /// Funds are reserved, waiting to be posted or cancelled.
    Pending,
    /// Completed transfer; balances have been updated.
    Posted,
    /// Cancelled transfer; held funds are released.
    Voided,
}

/// A transfer between two accounts.
///
/// Fields are private: a transfer can only be constructed through the
/// validated [`Transfer::new_immediate`], [`Transfer::new_pending`], or the
/// crate-internal settlement constructor, so cross-field combinations (e.g. a
/// posted transfer carrying a hold link) are only reachable through the ledger
/// itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transfer {
    id: TransferId,
    debit_account_id: AccountId,
    credit_account_id: AccountId,
    amount: Amount,
    state: TransferState,
    pending_id: Option<TransferId>,
    timestamp: u64,
}

impl Transfer {
    /// Create a transfer that settles immediately.
    ///
    /// # Errors
    ///
    /// Returns [`LedgerError::DebitCreditAccountSame`] if debit and credit accounts match.
    /// Returns [`LedgerError::ZeroAmountTransfer`] if amount is zero.
    pub fn new_immediate(
        id: TransferId,
        debit_account_id: AccountId,
        credit_account_id: AccountId,
        amount: Amount,
        timestamp: u64,
    ) -> Result<Self, LedgerError> {
        Self::validate_invariants(id, debit_account_id, credit_account_id, amount)?;

        Ok(Self {
            id,
            debit_account_id,
            credit_account_id,
            amount,
            state: TransferState::Posted,
            pending_id: None,
            timestamp,
        })
    }

    /// Create a pending transfer that holds funds.
    ///
    /// # Errors
    ///
    /// Returns [`LedgerError::DebitCreditAccountSame`] if debit and credit accounts match.
    /// Returns [`LedgerError::ZeroAmountTransfer`] if amount is zero.
    pub fn new_pending(
        id: TransferId,
        debit_account_id: AccountId,
        credit_account_id: AccountId,
        amount: Amount,
        timestamp: u64,
    ) -> Result<Self, LedgerError> {
        Self::validate_invariants(id, debit_account_id, credit_account_id, amount)?;

        Ok(Self {
            id,
            debit_account_id,
            credit_account_id,
            amount,
            state: TransferState::Pending,
            pending_id: None,
            timestamp,
        })
    }

    /// Create a posted transfer that settles a pending hold.
    ///
    /// Crate-internal: only [`super::Ledger::post_pending`] produces a posted
    /// transfer that carries a hold link.
    ///
    /// # Errors
    ///
    /// Returns [`LedgerError::DebitCreditAccountSame`] if debit and credit accounts match.
    /// Returns [`LedgerError::ZeroAmountTransfer`] if amount is zero.
    pub(crate) fn new_settlement(
        id: TransferId,
        debit_account_id: AccountId,
        credit_account_id: AccountId,
        amount: Amount,
        pending_id: TransferId,
        timestamp: u64,
    ) -> Result<Self, LedgerError> {
        Self::validate_invariants(id, debit_account_id, credit_account_id, amount)?;

        Ok(Self {
            id,
            debit_account_id,
            credit_account_id,
            amount,
            state: TransferState::Posted,
            pending_id: Some(pending_id),
            timestamp,
        })
    }

    /// Unique transfer ID.
    #[inline]
    #[must_use]
    pub const fn id(&self) -> TransferId {
        self.id
    }

    /// Account being debited.
    #[inline]
    #[must_use]
    pub const fn debit_account_id(&self) -> AccountId {
        self.debit_account_id
    }

    /// Account being credited.
    #[inline]
    #[must_use]
    pub const fn credit_account_id(&self) -> AccountId {
        self.credit_account_id
    }

    /// Transfer amount.
    #[inline]
    #[must_use]
    pub const fn amount(&self) -> Amount {
        self.amount
    }

    /// The transfer's current state.
    #[inline]
    #[must_use]
    pub const fn state(&self) -> TransferState {
        self.state
    }

    /// Original pending transfer ID, if this posts a hold.
    #[inline]
    #[must_use]
    pub const fn pending_id(&self) -> Option<TransferId> {
        self.pending_id
    }

    /// Timestamp of the transfer.
    #[inline]
    #[must_use]
    pub const fn timestamp(&self) -> u64 {
        self.timestamp
    }

    /// Check that the cross-field shape is legal regardless of how this
    /// transfer was produced. A fresh `Pending` hold is a link *target*, so it
    /// cannot itself carry a hold link; only posted settlements may. This
    /// closes the last constructor bypass: a transfer forged via
    /// deserialization is rejected here, crate-wide, before any apply.
    ///
    /// # Errors
    ///
    /// Returns [`LedgerError::InvalidPendingLink`] if a pending transfer carries a hold link.
    pub(crate) fn validate_shape(&self) -> Result<(), LedgerError> {
        if let (TransferState::Pending, Some(pending_id)) = (self.state, self.pending_id) {
            return Err(LedgerError::InvalidPendingLink {
                transfer_id: self.id,
                pending_id,
                actual: TransferState::Pending,
            });
        }
        Ok(())
    }

    /// Advance this transfer to `target`, encoding the legal lifecycle.
    ///
    /// Only `Pending` can move on — to `Posted` (settled) or `Voided`
    /// (canceled); both are terminal states. Crate-internal: the ledger is the
    /// only transitioner and does so after all balance effects are committed.
    ///
    /// # Errors
    ///
    /// Returns [`LedgerError::InvalidTransferState`] if the transition is illegal.
    pub(crate) fn transition_to(&mut self, target: TransferState) -> Result<(), LedgerError> {
        if !matches!(
            (self.state, target),
            (TransferState::Pending, TransferState::Posted)
                | (TransferState::Pending, TransferState::Voided)
        ) {
            return Err(LedgerError::InvalidTransferState {
                id: self.id,
                expected: TransferState::Pending,
                actual: self.state,
            });
        }
        self.state = target;
        Ok(())
    }

    fn validate_invariants(
        id: TransferId,
        debit_account_id: AccountId,
        credit_account_id: AccountId,
        amount: Amount,
    ) -> Result<(), LedgerError> {
        if debit_account_id == credit_account_id {
            return Err(LedgerError::DebitCreditAccountSame(debit_account_id));
        }

        if amount.is_zero() {
            return Err(LedgerError::ZeroAmountTransfer(id));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_hold_cannot_carry_a_hold_link() {
        let forged = Transfer {
            id: TransferId::new(7),
            debit_account_id: AccountId::new(1),
            credit_account_id: AccountId::new(2),
            amount: Amount::new(10),
            state: TransferState::Pending,
            pending_id: Some(TransferId::new(3)),
            timestamp: 0,
        };

        assert_eq!(
            forged.validate_shape().unwrap_err(),
            LedgerError::InvalidPendingLink {
                transfer_id: TransferId::new(7),
                pending_id: TransferId::new(3),
                actual: TransferState::Pending,
            },
        );
    }

    #[test]
    fn posted_and_voided_are_terminal_states() {
        let mut transfer = Transfer::new_pending(
            TransferId::new(1),
            AccountId::new(1),
            AccountId::new(2),
            Amount::new(1),
            0,
        )
        .unwrap();

        transfer.transition_to(TransferState::Posted).unwrap();
        assert_eq!(transfer.state(), TransferState::Posted);

        let err = transfer.transition_to(TransferState::Voided).unwrap_err();
        assert!(matches!(err, LedgerError::InvalidTransferState { .. }));

        let err = transfer.transition_to(TransferState::Pending).unwrap_err();
        assert!(matches!(err, LedgerError::InvalidTransferState { .. }));

        let mut cancel = Transfer::new_pending(
            TransferId::new(1),
            AccountId::new(1),
            AccountId::new(2),
            Amount::new(1),
            0,
        )
        .unwrap();
        cancel.transition_to(TransferState::Voided).unwrap();
        assert_eq!(cancel.state(), TransferState::Voided);

        let err = cancel.transition_to(TransferState::Posted).unwrap_err();
        assert!(matches!(err, LedgerError::InvalidTransferState { .. }));
    }
}
