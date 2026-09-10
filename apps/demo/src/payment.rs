//! Payment domain model, lifecycle state machine, and double-entry ledger integration.

use ledger_core::account::{AccountFlags, AccountType};
use ledger_core::amount::{Amount, Scale};
use ledger_core::id::{AccountId, TransferId};
use ledger_core::transfer::Transfer;
use ledger_core::Ledger;
use serde::{Deserialize, Serialize};

use crate::error::PaymentError;

/// Lifecycle state of a payment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PaymentState {
    /// Funds reserved on payer's account via a pending hold.
    Authorized,
    /// Hold posted; merchant settlement obligation confirmed.
    Captured,
    /// Pending hold cancelled; reserved funds released back to customer.
    Voided,
    /// Captured payment reversed via counter-transfer.
    Refunded,
}

/// A payment record tracking lifecycle transitions and underlying ledger transfer IDs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Payment {
    /// Unique payment identifier.
    pub id: u128,
    /// Identifier of the merchant receiving payment.
    pub merchant_id: u128,
    /// Identifier of the customer making payment.
    pub customer_id: u128,
    /// Gross payment amount in decimal units.
    pub amount: u128,
    /// Processing fee charged by the platform.
    pub fee_amount: u128,
    /// Current lifecycle state.
    pub state: PaymentState,
    /// Associated ledger pending hold transfer ID.
    pub pending_transfer_id: Option<u128>,
    /// Associated ledger posted settlement transfer ID.
    pub posted_transfer_id: Option<u128>,
    /// Sequence number of the on-chain settlement batch if committed.
    pub settlement_batch_seq: Option<u64>,
    /// Timestamp of initial authorization.
    pub created_at: u64,
    /// Timestamp of last lifecycle state change.
    pub updated_at: u64,
}

impl Payment {
    /// Create a new authorized payment record.
    #[must_use]
    pub const fn new_authorized(
        id: u128,
        merchant_id: u128,
        customer_id: u128,
        amount: u128,
        fee_amount: u128,
        pending_transfer_id: u128,
        timestamp: u64,
    ) -> Self {
        Self {
            id,
            merchant_id,
            customer_id,
            amount,
            fee_amount,
            state: PaymentState::Authorized,
            pending_transfer_id: Some(pending_transfer_id),
            posted_transfer_id: None,
            settlement_batch_seq: None,
            created_at: timestamp,
            updated_at: timestamp,
        }
    }

    /// Transition from [`PaymentState::Authorized`] to [`PaymentState::Captured`].
    ///
    /// # Errors
    ///
    /// Returns [`PaymentError::InvalidStateTransition`] if the payment is not in Authorized state.
    pub fn transition_to_captured(
        &mut self,
        posted_transfer_id: u128,
        timestamp: u64,
    ) -> Result<(), PaymentError> {
        if self.state != PaymentState::Authorized {
            return Err(PaymentError::InvalidStateTransition {
                payment_id: self.id,
                from: format!("{:?}", self.state),
                to: "Captured".to_string(),
            });
        }

        self.state = PaymentState::Captured;
        self.posted_transfer_id = Some(posted_transfer_id);
        self.updated_at = timestamp;
        Ok(())
    }

    /// Transition from [`PaymentState::Authorized`] to [`PaymentState::Voided`].
    ///
    /// # Errors
    ///
    /// Returns [`PaymentError::InvalidStateTransition`] if the payment is not in Authorized state.
    pub fn transition_to_voided(&mut self, timestamp: u64) -> Result<(), PaymentError> {
        if self.state != PaymentState::Authorized {
            return Err(PaymentError::InvalidStateTransition {
                payment_id: self.id,
                from: format!("{:?}", self.state),
                to: "Voided".to_string(),
            });
        }

        self.state = PaymentState::Voided;
        self.updated_at = timestamp;
        Ok(())
    }

    /// Transition from [`PaymentState::Captured`] to [`PaymentState::Refunded`].
    ///
    /// # Errors
    ///
    /// Returns [`PaymentError::InvalidStateTransition`] if the payment is not in Captured state.
    pub fn transition_to_refunded(&mut self, timestamp: u64) -> Result<(), PaymentError> {
        if self.state != PaymentState::Captured {
            return Err(PaymentError::InvalidStateTransition {
                payment_id: self.id,
                from: format!("{:?}", self.state),
                to: "Refunded".to_string(),
            });
        }

        self.state = PaymentState::Refunded;
        self.updated_at = timestamp;
        Ok(())
    }
}

/// Helper mapping domain IDs to strictly typed ledger [`AccountId`]s.
pub struct AccountDirectory;

impl AccountDirectory {
    /// Platform fee revenue account ID.
    pub const FEE_REVENUE: AccountId = AccountId::new(999_999);
    /// Bank asset vault reserve account ID.
    pub const VAULT_ASSET: AccountId = AccountId::new(100);

    /// Compute customer deposit account ID.
    #[inline]
    #[must_use]
    pub const fn customer(customer_id: u128) -> AccountId {
        AccountId::new(1_000_000 + customer_id)
    }

    /// Compute merchant settlement liability account ID.
    #[inline]
    #[must_use]
    pub const fn merchant(merchant_id: u128) -> AccountId {
        AccountId::new(2_000_000 + merchant_id)
    }

    /// Initialize core accounts in the ledger (vault, fee revenue, customer, merchant).
    ///
    /// # Errors
    ///
    /// Returns [`PaymentError::Ledger`] if account creation fails.
    pub fn setup_accounts(
        ledger: &mut Ledger,
        scale: Scale,
        merchants: &[u128],
        customers: &[u128],
        initial_customer_deposit: u128,
        timestamp: u64,
    ) -> Result<(), PaymentError> {
        // Vault asset account
        ledger.create_account(
            Self::VAULT_ASSET,
            AccountType::Asset,
            AccountFlags::bank_asset(),
            scale,
            timestamp,
        )?;

        // Fee revenue account
        ledger.create_account(
            Self::FEE_REVENUE,
            AccountType::Revenue,
            AccountFlags::fee_revenue(),
            scale,
            timestamp,
        )?;

        // Setup merchants
        for &m_id in merchants {
            ledger.create_account(
                Self::merchant(m_id),
                AccountType::Liability,
                AccountFlags::customer(),
                scale,
                timestamp,
            )?;
        }

        // Setup customers and seed initial deposits from vault
        for (i, &c_id) in customers.iter().enumerate() {
            let acc_id = Self::customer(c_id);
            ledger.create_account(
                acc_id,
                AccountType::Liability,
                AccountFlags::customer(),
                scale,
                timestamp,
            )?;

            if initial_customer_deposit > 0 {
                let deposit_transfer = Transfer::new_immediate(
                    TransferId::new(10_000 + i as u128),
                    Self::VAULT_ASSET,
                    acc_id,
                    Amount::new(initial_customer_deposit),
                    timestamp,
                )?;
                ledger.create_transfer(deposit_transfer)?;
            }
        }

        Ok(())
    }
}
