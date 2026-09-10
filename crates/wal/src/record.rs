//! Binary record framing for the write-ahead log.
//!
//! Each batch is stored as one framed record:
//!
//! ```text
//!   +--------+---------+-------+-------------------------+-----------+
//!   | magic  |version  |flags  | seq (u64 LE)             | len (u32) |
//!   |  (u32) |  (u8)   | (u8)  |                          |           |
//!   | 4 bytes| 1 byte  | 1 byte|  8 bytes                 |  4 bytes  |
//!   +--------+---------+-------+-------------------------+-----------+
//!   +-----------------------------+   +-------------------+
//!   | payload (len bytes)         |   | crc32c (u32 LE)   |
//!   +-----------------------------+   +-------------------+
//! ```
//!
//! The checksum is stored after the payload it covers, so a torn write or bit
//! corruption anywhere in a frame fails that frame's checksum. Recovery then
//! stops at the record's start and keeps every earlier record intact.

use std::io::BufRead;

use crate::error::{HeaderError, WalError};

/// First four bytes of every wal frame; hexdump value is `LETW`.
pub const MAGIC: u32 = u32::from_le_bytes(*b"LETW");

/// On-disk format version. Bump when the framing layout changes.
pub const VERSION: u8 = 1;

/// Header length: magic, version, flags, seq, payload length.
pub const HEADER_LEN: usize = 18;

/// Trailer length holding the frame checksum.
pub const TRAILER_LEN: usize = 4;

/// Framing bytes added to every record.
pub const FRAME_OVERHEAD: usize = HEADER_LEN + TRAILER_LEN;

/// Max payload bytes a single record may carry; bounds memory while scanning
/// during recovery.
pub const DEFAULT_MAX_PAYLOAD_LEN: usize = 256 * 1024;

/// Byte offset of the u64 sequence number within a header.
const SEQ_OFFSET: usize = 6;

/// Byte offset of the u32 payload length within a header.
const LEN_OFFSET: usize = 14;

/// A verified record replayed from the log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    /// Batch number; 0, 1, 2, ... in the committed log.
    pub seq: u64,
    /// The batch payload the caller appended.
    pub payload: Vec<u8>,
}

/// Total framed length in bytes for a payload of `payload_len`.
#[inline]
#[must_use]
pub const fn frame_len(payload_len: usize) -> usize {
    payload_len + FRAME_OVERHEAD
}

/// Little-endian u32 at `offset`. Callers must have verified `offset + 4`
/// fits in the slice first.
fn read_u32_le(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

/// Little-endian u64 at `offset`. Callers must have verified `offset + 8`
/// fits in the slice first.
fn read_u64_le(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
        bytes[offset + 4],
        bytes[offset + 5],
        bytes[offset + 6],
        bytes[offset + 7],
    ])
}

/// Fill `buf` from `reader` in a loop, returning how many bytes were read.
/// Short reads are transparently topped up; a return below `buf.len()` means
/// the reader hit its end.
fn read_upto<R: BufRead>(reader: &mut R, buf: &mut [u8]) -> Result<usize, std::io::Error> {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        }
    }
    Ok(filled)
}

/// One step of a streaming scan over a wal file.
#[derive(Debug, PartialEq, Eq)]
pub enum ScanStep {
    /// A fully verified frame: header, payload, and checksum all checked.
    Record {
        /// Batch number of the frame.
        seq: u64,
        /// Payload length in bytes.
        len: usize,
        /// The frame's payload; `Some` only when the scan asked to fetch it.
        payload: Option<Vec<u8>>,
    },
    /// Clean end of the file: the scan read zero bytes at a frame boundary.
    End,
    /// A torn, corrupted, or mis-sized frame. The committed prefix ended just
    /// before this frame's start.
    Broken,
}

/// Read and verify the next frame from `reader`.
///
/// `payload` is only materialized when `fetch_payload` is true; recovery
/// passes false so a scan holds at most one frame's payload-buffer-worth of
/// memory. When `fetch_payload` is false the payload region is still read and
/// checksummed, so `Broken` is still detected.
///
/// # Errors
///
/// Returns the underlying [`std::io::Error`] on filesystem read failure. A
/// malformed frame is not an error: it is reported as [`ScanStep::Broken`].
pub fn next_record<R: BufRead>(
    reader: &mut R,
    max_payload_len: usize,
    fetch_payload: bool,
) -> Result<ScanStep, std::io::Error> {
    let mut head = [0u8; HEADER_LEN];
    let head_len = read_upto(reader, &mut head)?;
    if head_len == 0 {
        return Ok(ScanStep::End);
    }
    if head_len < HEADER_LEN {
        return Ok(ScanStep::Broken);
    }

    if read_u32_le(&head, 0) != MAGIC || head[4] != VERSION {
        return Ok(ScanStep::Broken);
    }
    let seq = read_u64_le(&head, SEQ_OFFSET);
    let len = read_u32_le(&head, LEN_OFFSET) as usize;
    if len > max_payload_len {
        return Ok(ScanStep::Broken);
    }

    let mut crc = crc32c::crc32c(&head);
    let mut payload = if fetch_payload {
        Some(Vec::with_capacity(len))
    } else {
        None
    };

    let mut chunk = [0u8; 64 * 1024];
    let mut remaining = len;
    while remaining > 0 {
        let want = remaining.min(chunk.len());
        let got = read_upto(reader, &mut chunk[..want])?;
        if got == 0 {
            return Ok(ScanStep::Broken);
        }
        let bytes = &chunk[..got];
        crc = crc32c::crc32c_append(crc, bytes);
        if let Some(payload) = payload.as_mut() {
            payload.extend_from_slice(bytes);
        }
        remaining -= got;
    }

    let mut trailer = [0u8; TRAILER_LEN];
    if read_upto(reader, &mut trailer)? < TRAILER_LEN {
        return Ok(ScanStep::Broken);
    }
    if crc != read_u32_le(&trailer, 0) {
        return Ok(ScanStep::Broken);
    }

    Ok(ScanStep::Record { seq, len, payload })
}

/// Encode one record frame into `dest`, reusing its allocated capacity.
///
/// # Errors
///
/// Returns [`WalError::PayloadTooLarge`] when `payload.len()` exceeds `max_payload_len`.
pub fn encode_into(
    seq: u64,
    payload: &[u8],
    max_payload_len: usize,
    dest: &mut Vec<u8>,
) -> Result<(), WalError> {
    encode_frame_into(MAGIC, VERSION, seq, payload, max_payload_len, dest)
}

/// Append one encoded record frame to `dest`, reusing and growing its capacity as needed.
///
/// Unlike [`encode_into`], this does not clear `dest` before encoding, allowing
/// callers to assemble multiple frames into a contiguous buffer for atomic
/// batched writing.
///
/// # Errors
///
/// Returns [`WalError::PayloadTooLarge`] when `payload.len()` exceeds `max_payload_len`.
pub fn encode_append(
    seq: u64,
    payload: &[u8],
    max_payload_len: usize,
    dest: &mut Vec<u8>,
) -> Result<(), WalError> {
    encode_frame_append(MAGIC, VERSION, seq, payload, max_payload_len, dest)
}

/// Encode one record frame. Returns [`WalError::PayloadTooLarge`] if the
/// payload exceeds `max_payload_len`.
///
/// # Errors
///
/// Returns [`WalError::PayloadTooLarge`] when `payload.len()` exceeds
/// `max_payload_len`.
pub fn encode(seq: u64, payload: &[u8], max_payload_len: usize) -> Result<Vec<u8>, WalError> {
    let mut frame = Vec::with_capacity(frame_len(payload.len()));
    encode_into(seq, payload, max_payload_len, &mut frame)?;
    Ok(frame)
}

/// Encode a frame for consumers that share the wal layout but need their own
/// magic (the checkpoint snapshot).
pub(crate) fn encode_frame(
    magic: u32,
    version: u8,
    seq: u64,
    payload: &[u8],
    max_payload_len: usize,
) -> Result<Vec<u8>, WalError> {
    let mut frame = Vec::with_capacity(frame_len(payload.len()));
    encode_frame_into(magic, version, seq, payload, max_payload_len, &mut frame)?;
    Ok(frame)
}

/// Append a frame into `dest` for consumers that share the wal layout but need their own magic.
pub(crate) fn encode_frame_append(
    magic: u32,
    version: u8,
    seq: u64,
    payload: &[u8],
    max_payload_len: usize,
    dest: &mut Vec<u8>,
) -> Result<(), WalError> {
    if payload.len() > max_payload_len {
        return Err(WalError::PayloadTooLarge {
            actual: payload.len(),
            max: max_payload_len,
        });
    }

    let start = dest.len();
    let total_len = frame_len(payload.len());
    dest.resize(start + total_len, 0);

    dest[start..start + 4].copy_from_slice(&magic.to_le_bytes());
    dest[start + 4] = version;
    dest[start + 5] = 0;
    dest[start + SEQ_OFFSET..start + SEQ_OFFSET + 8].copy_from_slice(&seq.to_le_bytes());
    dest[start + LEN_OFFSET..start + LEN_OFFSET + 4]
        .copy_from_slice(&(payload.len() as u32).to_le_bytes());

    let payload_start = start + HEADER_LEN;
    dest[payload_start..payload_start + payload.len()].copy_from_slice(payload);

    let checksum = crc32c::crc32c(&dest[start..start + total_len - TRAILER_LEN]);
    let crc_offset = start + total_len - TRAILER_LEN;
    dest[crc_offset..start + total_len].copy_from_slice(&checksum.to_le_bytes());
    Ok(())
}

/// Encode a frame into `dest` for consumers that share the wal layout but need their own magic.
pub(crate) fn encode_frame_into(
    magic: u32,
    version: u8,
    seq: u64,
    payload: &[u8],
    max_payload_len: usize,
    dest: &mut Vec<u8>,
) -> Result<(), WalError> {
    dest.clear();
    encode_frame_append(magic, version, seq, payload, max_payload_len, dest)
}

/// Validate a complete frame (header + payload + trailer) and return its
/// sequence number and payload length.
///
/// # Errors
///
/// Returns [`WalError::InvalidHeader`] if the header is malformed or
/// describes a record larger than `max_payload_len`, and
/// [`WalError::ChecksumMismatch`] if the trailer does not cover the frame.
pub fn decode_frame(frame: &[u8], max_payload_len: usize) -> Result<(u64, usize), WalError> {
    decode_frame_with(MAGIC, VERSION, frame, max_payload_len)
}

/// Decode a frame using an explicit magic and version, for consumers that
/// share the layout but use their own magic.
pub(crate) fn decode_frame_with(
    magic: u32,
    version: u8,
    frame: &[u8],
    max_payload_len: usize,
) -> Result<(u64, usize), WalError> {
    let len = frame.len();
    if len < FRAME_OVERHEAD {
        return Err(WalError::InvalidHeader {
            offset: 0,
            kind: HeaderError::TruncatedFrame,
        });
    }

    let offset = HEADER_LEN as u64;
    let actual_magic = read_u32_le(frame, 0);
    if actual_magic != magic {
        return Err(WalError::InvalidHeader {
            offset,
            kind: HeaderError::BadMagic,
        });
    }
    let actual_version = frame[4];
    if actual_version != version {
        return Err(WalError::InvalidHeader {
            offset,
            kind: HeaderError::UnsupportedVersion(actual_version),
        });
    }
    let payload_len = read_u32_le(frame, LEN_OFFSET) as usize;
    if payload_len > max_payload_len {
        return Err(WalError::InvalidHeader {
            offset,
            kind: HeaderError::PayloadTooLarge,
        });
    }
    if len != frame_len(payload_len) {
        return Err(WalError::InvalidHeader {
            offset,
            kind: HeaderError::TruncatedFrame,
        });
    }

    let seq = read_u64_le(frame, SEQ_OFFSET);
    let checksum = read_u32_le(frame, len - TRAILER_LEN);
    let computed = crc32c::crc32c(&frame[..len - TRAILER_LEN]);
    if computed != checksum {
        return Err(WalError::ChecksumMismatch { offset });
    }

    Ok((seq, payload_len))
}

#[cfg(test)]
mod tests {
    use std::io::{BufReader, Cursor};

    use super::*;

    fn frame_stream(bytes: &[u8]) -> impl BufRead + '_ {
        BufReader::with_capacity(7, Cursor::new(bytes))
    }

    #[test]
    fn frame_roundtrip_preserves_payload() {
        let payload = b"settle 1000000 usdc".to_vec();
        let encoded = encode(7, &payload, DEFAULT_MAX_PAYLOAD_LEN).unwrap();
        let (seq, len) = decode_frame(&encoded, DEFAULT_MAX_PAYLOAD_LEN).unwrap();
        assert_eq!((seq, len), (7, payload.len()));
        assert_eq!(
            &encoded[HEADER_LEN..HEADER_LEN + payload.len()],
            &payload[..]
        );
    }

    #[test]
    fn empty_payload_is_a_valid_record() {
        let encoded = encode(0, &[], DEFAULT_MAX_PAYLOAD_LEN).unwrap();
        assert_eq!(encoded.len(), FRAME_OVERHEAD);
        assert_eq!(
            decode_frame(&encoded, DEFAULT_MAX_PAYLOAD_LEN).unwrap(),
            (0, 0)
        );
    }

    #[test]
    fn zero_padded_frame_fails_checksum() {
        let payload = b"money".to_vec();
        let mut encoded = encode(3, &payload, DEFAULT_MAX_PAYLOAD_LEN).unwrap();
        encoded[0] = 0;
        assert!(matches!(
            decode_frame(&encoded, DEFAULT_MAX_PAYLOAD_LEN),
            Err(WalError::InvalidHeader {
                kind: HeaderError::BadMagic,
                ..
            })
        ));
    }

    #[test]
    fn over_max_payload_is_rejected() {
        let payload = vec![0u8; DEFAULT_MAX_PAYLOAD_LEN + 1];
        assert!(matches!(
            encode(0, &payload, DEFAULT_MAX_PAYLOAD_LEN),
            Err(WalError::PayloadTooLarge { .. })
        ));
        let encoded = encode(0, &payload, usize::MAX).unwrap();
        assert!(matches!(
            decode_frame(&encoded, DEFAULT_MAX_PAYLOAD_LEN),
            Err(WalError::InvalidHeader {
                kind: HeaderError::PayloadTooLarge,
                ..
            })
        ));
    }

    #[test]
    fn truncated_frame_is_rejected() {
        let payload = vec![0xabu8; 100];
        let encoded = encode(1, &payload, DEFAULT_MAX_PAYLOAD_LEN).unwrap();
        let truncated = &encoded[..encoded.len() - TRAILER_LEN];
        assert!(matches!(
            decode_frame(truncated, DEFAULT_MAX_PAYLOAD_LEN),
            Err(WalError::InvalidHeader {
                kind: HeaderError::TruncatedFrame,
                ..
            })
        ));
    }

    #[test]
    fn detection_of_single_bit_flip_in_payload() {
        let payload = b"1000000".to_vec();
        let mut encoded = encode(9, &payload, DEFAULT_MAX_PAYLOAD_LEN).unwrap();
        let flip = HEADER_LEN + payload.len() / 2;
        encoded[flip] ^= 0x01;
        assert!(matches!(
            decode_frame(&encoded, DEFAULT_MAX_PAYLOAD_LEN),
            Err(WalError::ChecksumMismatch { .. })
        ));
    }

    #[test]
    fn frame_len_is_exact() {
        let payload = vec![0u8; 42];
        let encoded = encode(0, &payload, DEFAULT_MAX_PAYLOAD_LEN).unwrap();
        assert_eq!(encoded.len(), frame_len(42));
    }

    #[test]
    fn streaming_scan_matches_decode_frame() {
        let payload = b"stream me".to_vec();
        let encoded = encode(11, &payload, DEFAULT_MAX_PAYLOAD_LEN).unwrap();
        let mut reader = frame_stream(&encoded);
        assert_eq!(
            next_record(&mut reader, DEFAULT_MAX_PAYLOAD_LEN, true).unwrap(),
            ScanStep::Record {
                seq: 11,
                len: payload.len(),
                payload: Some(payload.clone()),
            }
        );
        assert_eq!(
            next_record(&mut reader, DEFAULT_MAX_PAYLOAD_LEN, false).unwrap(),
            ScanStep::End
        );
    }

    #[test]
    fn streaming_scan_reads_small_buffers() {
        let mut encoded = Vec::new();
        for i in 0..20u64 {
            let payload = vec![i as u8; (i as usize) % 4096];
            encoded.extend(encode(i, &payload, DEFAULT_MAX_PAYLOAD_LEN).unwrap());
        }
        let mut reader = frame_stream(&encoded);
        for i in 0..20u64 {
            match next_record(&mut reader, DEFAULT_MAX_PAYLOAD_LEN, true).unwrap() {
                ScanStep::Record { seq, payload, .. } => {
                    assert_eq!(seq, i);
                    assert_eq!(
                        payload.expect("fetched"),
                        vec![i as u8; (i as usize) % 4096]
                    );
                }
                _ => panic!("expected record {i}"),
            }
        }
        assert_eq!(
            next_record(&mut reader, DEFAULT_MAX_PAYLOAD_LEN, false).unwrap(),
            ScanStep::End
        );
    }

    #[test]
    fn streaming_scan_detects_broken_frames() {
        let good = encode(1, b"payload", DEFAULT_MAX_PAYLOAD_LEN).unwrap();

        let torn = &good[..good.len() - 1];
        assert_eq!(
            next_record(&mut frame_stream(torn), DEFAULT_MAX_PAYLOAD_LEN, true).unwrap(),
            ScanStep::Broken
        );

        let garbage = b"this is not a frame";
        assert_eq!(
            next_record(&mut frame_stream(garbage), DEFAULT_MAX_PAYLOAD_LEN, true).unwrap(),
            ScanStep::Broken
        );

        let mut flipped = good.clone();
        let middle = flipped.len() / 2;
        flipped[middle] ^= 0x01;
        assert_eq!(
            next_record(&mut frame_stream(&flipped), DEFAULT_MAX_PAYLOAD_LEN, true).unwrap(),
            ScanStep::Broken
        );
    }

    #[test]
    fn streaming_scan_rejects_oversized_declared_length() {
        let mut encoded = encode(1, &[], DEFAULT_MAX_PAYLOAD_LEN).unwrap();
        encoded[LEN_OFFSET..LEN_OFFSET + 4]
            .copy_from_slice(&(DEFAULT_MAX_PAYLOAD_LEN as u32 + 1).to_le_bytes());
        assert_eq!(
            next_record(&mut frame_stream(&encoded), DEFAULT_MAX_PAYLOAD_LEN, true).unwrap(),
            ScanStep::Broken
        );
    }

    #[test]
    fn streaming_scan_verifies_checksum_without_payload_copy() {
        let payload = vec![0x5au8; 8192];
        let encoded = encode(7, &payload, DEFAULT_MAX_PAYLOAD_LEN).unwrap();
        assert_eq!(
            next_record(&mut frame_stream(&encoded), DEFAULT_MAX_PAYLOAD_LEN, false).unwrap(),
            ScanStep::Record {
                seq: 7,
                len: payload.len(),
                payload: None,
            }
        );
    }
}
