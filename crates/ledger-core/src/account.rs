//! Accounts, balance limits, and balances.

use serde::{Deserialize, Serialize};

use crate::amount::{Amount, Scale};
use crate::error::LedgerError;
use crate::id::AccountId;

/// Account type in double-entry bookkeeping.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AccountType {
    /// Assets: things owned (cash, receivables). Normal balance is debit.
    Asset,
    /// Liabilities: things owed (customer deposits). Normal balance is credit.
    Liability,
    /// Equity: capital and retained earnings. Normal balance is credit.
    Equity,
    /// Revenue: income earned (fees). Normal balance is credit.
    Revenue,
    /// Expenses: costs incurred (network fees). Normal balance is debit.
    Expense,
}

/// Rules that control what an account can do.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AccountFlags {
    /// Disallow negative balance (no overdrafts).
    pub debits_must_not_exceed_credits: bool,
    /// Disallow asset reserves from dropping below zero.
    pub credits_must_not_exceed_debits: bool,
    /// If true, reject all new transfers.
    pub is_closed: bool,
}

impl AccountFlags {
    /// Settings for customer deposit accounts: no overdrafts.
    #[inline]
    #[must_use]
    pub const fn customer() -> Self {
        Self {
            debits_must_not_exceed_credits: true,
            credits_must_not_exceed_debits: false,
            is_closed: false,
        }
    }

    /// Settings for bank vaults: reserves cannot go negative.
    #[inline]
    #[must_use]
    pub const fn bank_asset() -> Self {
        Self {
            debits_must_not_exceed_credits: false,
            credits_must_not_exceed_debits: true,
            is_closed: false,
        }
    }

    /// Settings for fee revenue accounts: unlimited credits.
    #[inline]
    #[must_use]
    pub const fn fee_revenue() -> Self {
        Self {
            debits_must_not_exceed_credits: false,
            credits_must_not_exceed_debits: false,
            is_closed: false,
        }
    }

    /// Settings with no limits, used for internal clearing.
    #[inline]
    #[must_use]
    pub const fn unrestricted() -> Self {
        Self {
            debits_must_not_exceed_credits: false,
            credits_must_not_exceed_debits: false,
            is_closed: false,
        }
    }
}

/// Balances for an account, tracking posted totals and pending holds.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Balance {
    /// Total settled debits.
    pub debits_posted: Amount,
    /// Total settled credits.
    pub credits_posted: Amount,
    /// Debits currently held in pending transfers.
    pub debits_pending: Amount,
    /// Credits currently held in pending transfers.
    pub credits_pending: Amount,
}

impl Balance {
    /// Create empty balances starting at zero.
    #[inline]
    #[must_use]
    pub const fn new() -> Self {
        Self {
            debits_posted: Amount::ZERO,
            credits_posted: Amount::ZERO,
            debits_pending: Amount::ZERO,
            credits_pending: Amount::ZERO,
        }
    }

    /// Calculate net balance based on account type.
    ///
    /// Flattens the debit/credit components into one signed number for
    /// reporting and reconciliation. The ledger itself never uses this: money
    /// is tracked as separate unsigned components so a balance can never go
    /// negative mid-derivation.
    ///
    /// # Errors
    ///
    /// Returns [`LedgerError::SignedBalanceOverflow`] if a balance exceeds `i128::MAX`.
    pub fn net_settled(&self, account_type: AccountType) -> Result<i128, LedgerError> {
        let debits = i128::try_from(self.debits_posted.as_u128())
            .map_err(|_| LedgerError::SignedBalanceOverflow)?;
        let credits = i128::try_from(self.credits_posted.as_u128())
            .map_err(|_| LedgerError::SignedBalanceOverflow)?;

        Ok(match account_type {
            AccountType::Asset | AccountType::Expense => debits - credits,
            AccountType::Liability | AccountType::Equity | AccountType::Revenue => credits - debits,
        })
    }

    /// Spendable balance for customer accounts (credits minus debits posted and pending).
    ///
    /// Returns zero if debits equal or exceed credits.
    ///
    /// # Errors
    ///
    /// Returns [`LedgerError::ArithmeticOverflow`] if debit summation overflows.
    pub fn available_liability(&self) -> Result<Amount, LedgerError> {
        let total_debits = self.debits_posted.checked_add(self.debits_pending)?;
        let debits = total_debits.as_u128();
        if self.credits_posted.as_u128() >= debits {
            Ok(Amount::new(self.credits_posted.as_u128() - debits))
        } else {
            Ok(Amount::ZERO)
        }
    }

    /// Spendable balance for asset accounts (debits minus credits posted and pending).
    ///
    /// Returns zero if credits equal or exceed debits.
    ///
    /// # Errors
    ///
    /// Returns [`LedgerError::ArithmeticOverflow`] if credit summation overflows.
    pub fn available_asset(&self) -> Result<Amount, LedgerError> {
        let total_credits = self.credits_posted.checked_add(self.credits_pending)?;
        let credits = total_credits.as_u128();
        if self.debits_posted.as_u128() >= credits {
            Ok(Amount::new(self.debits_posted.as_u128() - credits))
        } else {
            Ok(Amount::ZERO)
        }
    }
}

/// A ledger account.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    /// Unique account ID.
    pub id: AccountId,
    /// Account type (asset, liability, etc.).
    pub account_type: AccountType,
    /// Account rules and limits.
    pub flags: AccountFlags,
    /// Currency decimal scale.
    pub scale: Scale,
    /// Current balances.
    pub balance: Balance,
}

impl Account {
    /// Create an account with zero balance.
    #[inline]
    #[must_use]
    pub const fn new(
        id: AccountId,
        account_type: AccountType,
        flags: AccountFlags,
        scale: Scale,
    ) -> Self {
        Self {
            id,
            account_type,
            flags,
            scale,
            balance: Balance::new(),
        }
    }

    /// Check if this account can be debited by this amount.
    ///
    /// # Errors
    ///
    /// Returns [`LedgerError::AccountClosed`] if closed.
    /// Returns [`LedgerError::InsufficientFunds`] if the debit would breach balance flags.
    pub fn ensure_can_debit(&self, amount: Amount) -> Result<(), LedgerError> {
        if self.flags.is_closed {
            return Err(LedgerError::AccountClosed(self.id));
        }

        if self.flags.debits_must_not_exceed_credits {
            let total_debits = self
                .balance
                .debits_posted
                .checked_add(self.balance.debits_pending)?
                .checked_add(amount)?;

            if total_debits > self.balance.credits_posted {
                let available = self.balance.available_liability()?;
                return Err(LedgerError::InsufficientFunds {
                    account_id: self.id,
                    requested: amount,
                    available,
                });
            }
        }

        Ok(())
    }

    /// Check if this account can be credited by this amount.
    ///
    /// # Errors
    ///
    /// Returns [`LedgerError::AccountClosed`] if closed.
    /// Returns [`LedgerError::InsufficientFunds`] if the credit would breach balance flags.
    pub fn ensure_can_credit(&self, amount: Amount) -> Result<(), LedgerError> {
        if self.flags.is_closed {
            return Err(LedgerError::AccountClosed(self.id));
        }

        if self.flags.credits_must_not_exceed_debits {
            let total_credits = self
                .balance
                .credits_posted
                .checked_add(self.balance.credits_pending)?
                .checked_add(amount)?;

            if total_credits > self.balance.debits_posted {
                let available = self.balance.available_asset()?;
                return Err(LedgerError::InsufficientFunds {
                    account_id: self.id,
                    requested: amount,
                    available,
                });
            }
        }

        Ok(())
    }
}
