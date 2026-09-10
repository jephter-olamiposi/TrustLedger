//! Cryptographic hashing primitives and domain-separated hash operators for Merkle Mountain Ranges.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;

/// Domain separation prefix for leaf nodes: `0x00`.
///
/// Prevents second pre-image attacks where an internal node could be passed
/// as a leaf (RFC 6962).
pub const LEAF_PREFIX: u8 = 0x00;

/// Domain separation prefix for internal tree nodes: `0x01`.
///
/// Internal nodes hash `0x01 || left_child || right_child`.
pub const NODE_PREFIX: u8 = 0x01;

/// Domain separation prefix for bagging mountain peaks: `0x02`.
///
/// Peak bagging folds peaks from right to left using `0x02 || left_peak || right_peak`.
pub const PEAK_PREFIX: u8 = 0x02;

/// A 32-byte cryptographic hash produced by SHA-256.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize)]
pub struct Hash32(pub [u8; 32]);

/// Error returned when parsing a [`Hash32`] from an invalid hex string.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum HexError {
    /// Hex string length was not 64 characters (32 bytes).
    #[error("invalid hex length: expected 64 characters, got {0}")]
    InvalidLength(usize),

    /// Hex string contained an invalid hexadecimal character.
    #[error("invalid hex character '{char}' at index {index}")]
    InvalidCharacter {
        /// The invalid character encountered.
        char: char,
        /// Byte offset in the input string.
        index: usize,
    },
}

impl Hash32 {
    /// All-zero 32-byte hash, used as the root of an empty tree.
    pub const ZERO: Self = Self([0u8; 32]);

    /// Create a new [`Hash32`] from a 32-byte array.
    #[inline]
    #[must_use]
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Return a reference to the underlying 32-byte array.
    #[inline]
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Convert this hash into a lowercase 64-character hexadecimal string.
    #[must_use]
    pub fn to_hex(&self) -> String {
        let mut out = String::with_capacity(64);
        for byte in &self.0 {
            use std::fmt::Write;
            let _ = write!(out, "{byte:02x}");
        }
        out
    }

    /// Parse a [`Hash32`] from a 64-character hexadecimal string.
    ///
    /// # Errors
    ///
    /// Returns [`HexError::InvalidLength`] if the string is not exactly 64 characters.
    /// Returns [`HexError::InvalidCharacter`] if any character is not valid hex.
    pub fn from_hex(s: &str) -> Result<Self, HexError> {
        if s.len() != 64 {
            return Err(HexError::InvalidLength(s.len()));
        }

        let mut bytes = [0u8; 32];
        let input = s.as_bytes();

        for i in 0..32 {
            let hi = decode_hex_nibble(input[2 * i], 2 * i)?;
            let lo = decode_hex_nibble(input[2 * i + 1], 2 * i + 1)?;
            bytes[i] = (hi << 4) | lo;
        }

        Ok(Self(bytes))
    }
}

/// Helper to decode a single ASCII hex nibble without external crate dependencies.
fn decode_hex_nibble(byte: u8, index: usize) -> Result<u8, HexError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(HexError::InvalidCharacter {
            char: byte as char,
            index,
        }),
    }
}

impl AsRef<[u8]> for Hash32 {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl From<[u8; 32]> for Hash32 {
    #[inline]
    fn from(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

impl From<Hash32> for [u8; 32] {
    #[inline]
    fn from(hash: Hash32) -> Self {
        hash.0
    }
}

impl fmt::Display for Hash32 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in &self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for Hash32 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Hash32({})", self.to_hex())
    }
}

/// Compute a leaf hash with domain separation prefix `0x00`.
///
/// Output: `Sha256(0x00 || data)`.
#[must_use]
pub fn hash_leaf(data: &[u8]) -> Hash32 {
    let mut hasher = Sha256::new();
    hasher.update([LEAF_PREFIX]);
    hasher.update(data);
    Hash32(hasher.finalize().into())
}

/// Combine two internal nodes into a parent hash with domain separation prefix `0x01`.
///
/// Output: `Sha256(0x01 || left || right)`.
#[must_use]
pub fn combine_nodes(left: &Hash32, right: &Hash32) -> Hash32 {
    let mut hasher = Sha256::new();
    hasher.update([NODE_PREFIX]);
    hasher.update(left.as_bytes());
    hasher.update(right.as_bytes());
    Hash32(hasher.finalize().into())
}

/// Combine two mountain peaks with domain separation prefix `0x02`.
///
/// Output: `Sha256(0x02 || left_peak || right_peak)`.
#[must_use]
pub fn combine_peaks(left_peak: &Hash32, right_peak: &Hash32) -> Hash32 {
    let mut hasher = Sha256::new();
    hasher.update([PEAK_PREFIX]);
    hasher.update(left_peak.as_bytes());
    hasher.update(right_peak.as_bytes());
    Hash32(hasher.finalize().into())
}

/// Bag all peaks into a single 32-byte Merkle root using right-to-left folding.
///
/// - If `peaks` is empty, returns [`Hash32::ZERO`].
/// - If `peaks` has exactly 1 peak, returns that peak directly.
/// - If `peaks` has multiple peaks, folds right-to-left:
///   `acc = combine_peaks(peaks[n-2], peaks[n-1])`, continuing up to `peaks[0]`.
#[must_use]
pub fn bag_peaks(peaks: &[Hash32]) -> Hash32 {
    match peaks.len() {
        0 => Hash32::ZERO,
        1 => peaks[0],
        len => {
            // Fold right-to-left: the smallest mountain on the right merges into its left neighbor.
            let mut acc = peaks[len - 1];
            for peak in peaks[..len - 1].iter().rev() {
                acc = combine_peaks(peak, &acc);
            }
            acc
        }
    }
}
