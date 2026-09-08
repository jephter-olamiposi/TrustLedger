//! Codec for journal event batches persisted through the write-ahead log.

use thiserror::Error;

use crate::journal::LedgerEvent;

/// Errors encoding or decoding a journal event batch.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CodecError {
    /// Serialization of a batch failed.
    #[error("journal event batch failed to encode: {0}")]
    Encode(String),
    /// Deserialization of a persisted batch failed.
    #[error("journal event bytes failed to decode: {0}")]
    Decode(String),
}

/// Serialize a journal event batch with postcard for the wal / snapshots.
///
/// Stable since postcard 1.1; the wal frame carries its own schema version
/// byte, separate from any future codec change.
///
/// # Errors
///
/// Returns [`CodecError::Encode`] if serialization fails.
pub fn encode_events(events: &[LedgerEvent]) -> Result<Vec<u8>, CodecError> {
    postcard::to_allocvec(events).map_err(|err| CodecError::Encode(err.to_string()))
}

/// Deserialize a batch previously written by [`encode_events`].
///
/// Rejects any trailing bytes after a single batch, so a torn batch never
/// silently becomes a shorter (wrong) history.
///
/// # Errors
///
/// Returns [`CodecError::Decode`] if the bytes do not describe a complete,
/// valid batch.
pub fn decode_events(bytes: &[u8]) -> Result<Vec<LedgerEvent>, CodecError> {
    let (events, remainder) = postcard::take_from_bytes::<Vec<LedgerEvent>>(bytes)
        .map_err(|err| CodecError::Decode(err.to_string()))?;
    if !remainder.is_empty() {
        return Err(CodecError::Decode(format!(
            "unexpected trailing leftover bytes ({} bytes)",
            remainder.len()
        )));
    }
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::{AccountFlags, AccountType};
    use crate::amount::{Amount, Scale};
    use crate::id::{AccountId, TransferId};

    fn sample_events() -> Vec<LedgerEvent> {
        vec![
            LedgerEvent::AccountCreated {
                id: AccountId::new(1),
                account_type: AccountType::Asset,
                flags: AccountFlags::bank_asset(),
                scale: Scale::usdc(),
                timestamp: 0,
            },
            LedgerEvent::TransferPosted {
                transfer: crate::transfer::Transfer::new_immediate(
                    TransferId::new(2),
                    AccountId::new(1),
                    AccountId::new(3),
                    Amount::new(1_000_000),
                    0,
                )
                .expect("valid transfer"),
            },
        ]
    }

    #[test]
    fn roundtrip_preserves_events() {
        let events = sample_events();
        let encoded = encode_events(&events).unwrap();
        assert_eq!(decode_events(&encoded).unwrap(), events);
    }

    #[test]
    fn empty_batch_roundtrips() {
        let encoded = encode_events(&[]).unwrap();
        assert_eq!(decode_events(&encoded).unwrap(), vec![]);
    }

    #[test]
    fn garbage_bytes_are_rejected() {
        assert!(matches!(
            decode_events(b"not a valid postcard batch").unwrap_err(),
            CodecError::Decode(_)
        ));
    }

    #[test]
    fn truncated_batch_is_rejected() {
        let encoded = encode_events(&sample_events()).unwrap();
        let truncated = &encoded[..encoded.len() - 1];
        assert!(matches!(
            decode_events(truncated).unwrap_err(),
            CodecError::Decode(_)
        ));
    }

    #[test]
    fn trailing_garbage_bytes_are_rejected() {
        let mut encoded = encode_events(&sample_events()).unwrap();
        encoded.push(0x42);
        assert!(matches!(
            decode_events(&encoded).unwrap_err(),
            CodecError::Decode(_)
        ));
    }
}
