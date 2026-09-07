//! Account and transfer identifier types.

use core::fmt;
use core::str::FromStr;
use serde::{Deserialize, Serialize};

use crate::error::LedgerError;

/// Unique account ID.
///
/// The inner `u128` is private; IDs are created through [`AccountId::new`]
/// or `From<u128>`, so a raw numeric payload cannot leak into ledger keys.
#[derive(
    Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[repr(transparent)]
pub struct AccountId(u128);

impl AccountId {
    /// Create an account ID from a raw number.
    #[inline]
    #[must_use]
    pub const fn new(id: u128) -> Self {
        Self(id)
    }

    /// Get the raw u128 number.
    #[inline]
    #[must_use]
    pub const fn as_u128(self) -> u128 {
        self.0
    }
}

impl From<u128> for AccountId {
    #[inline]
    fn from(value: u128) -> Self {
        Self(value)
    }
}

impl From<AccountId> for u128 {
    #[inline]
    fn from(id: AccountId) -> Self {
        id.0
    }
}

impl fmt::Display for AccountId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for AccountId {
    type Err = LedgerError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse::<u128>()
            .map(Self)
            .map_err(|_| LedgerError::InvalidIdentifier(s.to_string()))
    }
}

/// Unique transfer ID.
///
/// The inner `u128` is private; IDs are created through [`TransferId::new`]
/// or `From<u128>`.
#[derive(
    Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[repr(transparent)]
pub struct TransferId(u128);

impl TransferId {
    /// Create a transfer ID from a raw number.
    #[inline]
    #[must_use]
    pub const fn new(id: u128) -> Self {
        Self(id)
    }

    /// Get the raw u128 number.
    #[inline]
    #[must_use]
    pub const fn as_u128(self) -> u128 {
        self.0
    }
}

impl From<u128> for TransferId {
    #[inline]
    fn from(value: u128) -> Self {
        Self(value)
    }
}

impl From<TransferId> for u128 {
    #[inline]
    fn from(id: TransferId) -> Self {
        id.0
    }
}

impl fmt::Display for TransferId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for TransferId {
    type Err = LedgerError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse::<u128>()
            .map(Self)
            .map_err(|_| LedgerError::InvalidIdentifier(s.to_string()))
    }
}
