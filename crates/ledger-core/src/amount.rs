//! Fixed-point money amounts and scale types.

use core::fmt;
use serde::{Deserialize, Serialize};

use crate::error::LedgerError;

/// Maximum supported decimal places.
pub const MAX_SCALE: u8 = 18;

/// Decimal scale factor for an amount.
///
/// Typical scales: USD (2), USDC (6), BTC (8), ETH (18).
#[derive(
    Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[repr(transparent)]
pub struct Scale(u8);

impl Scale {
    /// Create a new scale, up to 18 decimal places.
    ///
    /// # Errors
    ///
    /// Returns [`LedgerError::InvalidScale`] if `scale > 18`.
    pub const fn new(scale: u8) -> Result<Self, LedgerError> {
        if scale > MAX_SCALE {
            Err(LedgerError::InvalidScale(scale))
        } else {
            Ok(Self(scale))
        }
    }

    /// US Dollars (2 decimals / cents).
    #[inline]
    #[must_use]
    pub const fn usd() -> Self {
        Self(2)
    }

    /// USDC on Solana (6 decimals).
    #[inline]
    #[must_use]
    pub const fn usdc() -> Self {
        Self(6)
    }

    /// Bitcoin (8 decimals / satoshis).
    #[inline]
    #[must_use]
    pub const fn btc() -> Self {
        Self(8)
    }

    /// Ethereum (18 decimals / wei).
    #[inline]
    #[must_use]
    pub const fn eth() -> Self {
        Self(18)
    }

    /// Get the raw number of decimal places.
    #[inline]
    #[must_use]
    pub const fn as_u8(self) -> u8 {
        self.0
    }

    /// Multiplier for this scale (e.g. 100 for 2 decimals).
    ///
    /// # Errors
    ///
    /// Returns [`LedgerError::ArithmeticOverflow`] if $10^N$ overflows u128.
    pub fn multiplier(self) -> Result<u128, LedgerError> {
        10u128
            .checked_pow(self.0 as u32)
            .ok_or(LedgerError::ArithmeticOverflow)
    }
}

impl fmt::Display for Scale {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "10^-{}", self.0)
    }
}

/// Unsigned money amount in base units (e.g. cents for USD).
///
/// The inner `u128` is private: amounts are only created through
/// [`Amount::new`]/`From<u128>` and mutated through the checked arithmetic
/// methods, so the field cannot be poked past overflow/validation boundaries.
#[derive(
    Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[repr(transparent)]
pub struct Amount(u128);

impl Amount {
    /// Zero amount.
    pub const ZERO: Self = Self(0);

    /// Create an amount from raw base units.
    #[inline]
    #[must_use]
    pub const fn new(units: u128) -> Self {
        Self(units)
    }

    /// Get the raw u128 value.
    #[inline]
    #[must_use]
    pub const fn as_u128(self) -> u128 {
        self.0
    }

    /// Returns true if the amount is zero.
    #[inline]
    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }

    /// Add two amounts, returning an error on overflow.
    ///
    /// # Errors
    ///
    /// Returns [`LedgerError::ArithmeticOverflow`] if the sum exceeds `u128::MAX`.
    pub fn checked_add(self, other: Self) -> Result<Self, LedgerError> {
        self.0
            .checked_add(other.0)
            .map(Self)
            .ok_or(LedgerError::ArithmeticOverflow)
    }

    /// Subtract two amounts, returning an error on underflow.
    ///
    /// # Errors
    ///
    /// Returns [`LedgerError::ArithmeticOverflow`] if `other > self`.
    pub fn checked_sub(self, other: Self) -> Result<Self, LedgerError> {
        self.0
            .checked_sub(other.0)
            .map(Self)
            .ok_or(LedgerError::ArithmeticOverflow)
    }

    /// Multiply by an integer, returning an error on overflow.
    ///
    /// # Errors
    ///
    /// Returns [`LedgerError::ArithmeticOverflow`] if the result exceeds `u128::MAX`.
    pub fn checked_mul(self, factor: u128) -> Result<Self, LedgerError> {
        self.0
            .checked_mul(factor)
            .map(Self)
            .ok_or(LedgerError::ArithmeticOverflow)
    }

    /// Divide by an integer, returning an error on division by zero.
    ///
    /// # Errors
    ///
    /// Returns [`LedgerError::DivisionByZero`] if `divisor` is zero.
    pub fn checked_div(self, divisor: u128) -> Result<Self, LedgerError> {
        self.0
            .checked_div(divisor)
            .map(Self)
            .ok_or(LedgerError::DivisionByZero)
    }

    /// Format as a decimal string (e.g. 100 with scale 2 becomes "1.00").
    #[must_use]
    pub fn format_with_scale(self, scale: Scale) -> String {
        if scale.0 == 0 {
            return self.0.to_string();
        }

        let mult = 10u128.pow(scale.0 as u32);
        let integer_part = self.0 / mult;
        let fractional_part = self.0 % mult;

        format!(
            "{}.{:0width$}",
            integer_part,
            fractional_part,
            width = scale.0 as usize
        )
    }
}

impl From<u128> for Amount {
    #[inline]
    fn from(value: u128) -> Self {
        Self(value)
    }
}

impl From<Amount> for u128 {
    #[inline]
    fn from(amount: Amount) -> Self {
        amount.0
    }
}

impl fmt::Display for Amount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
