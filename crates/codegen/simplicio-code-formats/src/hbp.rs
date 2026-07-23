//! Runtime-compatible HBP v1 append-only receipt records.
//!
//! Code writes the generic `code.record` topic through this adapter. The wire
//! format is Runtime's `HbpInbox`: `HBP1`, version/flags, length-prefixed UTF-8
//! fields, genesis-linked SHA-256 row hashes, and no JSON fallback.

use sha2::{Digest, Sha256};

pub const HBP_MAGIC: [u8; 4] = *b"HBP1";
pub const HBP_VERSION: u16 = 1;
pub const HBP_SCHEMA: &str = "simplicio.hbp/v1";
pub const HBP_GENESIS: &str = "genesis";
const HEADER_LEN: usize = 8;
const MAX_FIELD_BYTES: usize = 4 * 1024 * 1024;
const MAX_RECORD_BYTES: usize = 16 * 1024 * 1024;
const MAX_LEDGER_BYTES: usize = 64 * 1024 * 1024;
const CODE_TOPIC: &str = "code.record";
const CODE_PROVENANCE: &str = "simplicio-code";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HbpRecord {
    pub sequence: u64,
    pub payload: Vec<u8>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum HbpError {
    #[error("HBP record {0} is truncated")]
    Truncated(u64),
    #[error("invalid HBP magic")]
    BadMagic,
    #[error("unsupported HBP version {0}")]
    UnsupportedVersion(u16),
    #[error("HBP record length is invalid")]
    InvalidLength,
    #[error("HBP hash chain is broken at record {0}")]
    BrokenChain(u64),
    #[error("HBP payload exceeds the safety limit")]
    TooLarge,
    #[error("HBP sequence is not contiguous: expected {expected}, found {actual}")]
    NonContiguousSequence { expected: u64, actual: u64 },
    #[error("HBP field is not valid UTF-8")]
    InvalidUtf8,
    #[error("HBP field is too large")]
    FieldTooLarge,
    #[error("HBP crypto-token marker is invalid")]
    InvalidTokenMarker,
    #[error("HBP payload is not valid hexadecimal")]
    InvalidHex,
    #[error("HBP has trailing bytes")]
    TrailingBytes,
}

pub fn row_content_hash(
    sequence: u64,
    previous_hash: &str,
    topic: &str,
    payload: &str,
    provenance: &str,
    crypto_token: Option<&str>,
) -> String {
    let mut hasher = Sha256::new();
    for field in [
        sequence.to_string(),
        previous_hash.to_owned(),
        topic.to_owned(),
        payload.to_owned(),
        provenance.to_owned(),
        crypto_token.unwrap_or("").to_owned(),
    ] {
        hasher.update((field.len() as u64).to_le_bytes());
        hasher.update(field.as_bytes());
    }
    hex_encode(&hasher.finalize())
}

pub fn encode_hbp(records: &[HbpRecord]) -> Result<Vec<u8>, HbpError> {
    let mut output = Vec::with_capacity(HEADER_LEN);
    output.extend_from_slice(&HBP_MAGIC);
    output.extend_from_slice(&HBP_VERSION.to_le_bytes());
    output.extend_from_slice(&0u16.to_le_bytes());
    let mut previous_hash = HBP_GENESIS.to_owned();
    for (index, record) in records.iter().enumerate() {
        let expected = index as u64;
        if record.sequence != expected {
            return Err(HbpError::NonContiguousSequence {
                expected,
                actual: record.sequence,
            });
        }
        let payload = hex_encode(&record.payload);
        let hash = row_content_hash(
            expected,
            &previous_hash,
            CODE_TOPIC,
            &payload,
            CODE_PROVENANCE,
            None,
        );
        let body = encode_body(expected, &payload, &previous_hash, &hash)?;
        if body.len() > MAX_RECORD_BYTES {
            return Err(HbpError::TooLarge);
        }
        let len = u32::try_from(body.len()).map_err(|_| HbpError::TooLarge)?;
        output.extend_from_slice(&len.to_le_bytes());
        output.extend_from_slice(&body);
        previous_hash = hash;
        if output.len() > MAX_LEDGER_BYTES {
            return Err(HbpError::TooLarge);
        }
    }
    Ok(output)
}

pub fn decode_hbp(bytes: &[u8]) -> Result<Vec<HbpRecord>, HbpError> {
    if bytes.len() > MAX_LEDGER_BYTES {
        return Err(HbpError::TooLarge);
    }
    if bytes.len() < HEADER_LEN {
        return Err(HbpError::Truncated(0));
    }
    if bytes[..4] != HBP_MAGIC {
        return Err(HbpError::BadMagic);
    }
    let version = u16::from_le_bytes([bytes[4], bytes[5]]);
    if version != HBP_VERSION {
        return Err(HbpError::UnsupportedVersion(version));
    }
    if bytes[6] != 0 || bytes[7] != 0 {
        return Err(HbpError::UnsupportedVersion(u16::MAX));
    }
    let mut cursor = HEADER_LEN;
    let mut previous_hash = HBP_GENESIS.to_owned();
    let mut records = Vec::new();
    while cursor < bytes.len() {
        let sequence = records.len() as u64;
        if bytes.len() - cursor < 4 {
            return Err(HbpError::Truncated(sequence));
        }
        let length = u32::from_le_bytes(bytes[cursor..cursor + 4].try_into().unwrap()) as usize;
        cursor += 4;
        if length == 0 || length > MAX_RECORD_BYTES {
            return Err(HbpError::InvalidLength);
        }
        let end = cursor.checked_add(length).ok_or(HbpError::InvalidLength)?;
        if end > bytes.len() {
            return Err(HbpError::Truncated(sequence));
        }
        let (actual_sequence, payload, prev, hash, topic, provenance) =
            decode_body(&bytes[cursor..end])?;
        if actual_sequence != sequence {
            return Err(HbpError::NonContiguousSequence {
                expected: sequence,
                actual: actual_sequence,
            });
        }
        if prev != previous_hash || topic != CODE_TOPIC || provenance != CODE_PROVENANCE {
            return Err(HbpError::BrokenChain(sequence));
        }
        let recomputed = row_content_hash(sequence, &prev, &topic, &payload, &provenance, None);
        if recomputed != hash {
            return Err(HbpError::BrokenChain(sequence));
        }
        let decoded = hex_decode(&payload)?;
        records.push(HbpRecord {
            sequence,
            payload: decoded,
        });
        previous_hash = hash;
        cursor = end;
    }
    Ok(records)
}

fn encode_body(
    sequence: u64,
    payload: &str,
    previous_hash: &str,
    hash: &str,
) -> Result<Vec<u8>, HbpError> {
    let mut body = Vec::new();
    body.extend_from_slice(&sequence.to_le_bytes());
    body.extend_from_slice(&0u64.to_le_bytes());
    put_string(&mut body, CODE_TOPIC)?;
    put_string(&mut body, payload)?;
    put_string(&mut body, CODE_PROVENANCE)?;
    body.push(0);
    put_string(&mut body, previous_hash)?;
    put_string(&mut body, hash)?;
    Ok(body)
}

fn decode_body(body: &[u8]) -> Result<(u64, String, String, String, String, String), HbpError> {
    let mut cursor = 0;
    let sequence = take_u64(body, &mut cursor)?;
    let _timestamp = take_u64(body, &mut cursor)?;
    let topic = take_string(body, &mut cursor)?;
    let payload = take_string(body, &mut cursor)?;
    let provenance = take_string(body, &mut cursor)?;
    let marker = take(body, &mut cursor, 1)?[0];
    if marker != 0 {
        return Err(HbpError::InvalidTokenMarker);
    }
    let previous_hash = take_string(body, &mut cursor)?;
    let hash = take_string(body, &mut cursor)?;
    if cursor != body.len() {
        return Err(HbpError::TrailingBytes);
    }
    Ok((sequence, payload, previous_hash, hash, topic, provenance))
}

fn put_string(out: &mut Vec<u8>, value: &str) -> Result<(), HbpError> {
    if value.len() > MAX_FIELD_BYTES {
        return Err(HbpError::FieldTooLarge);
    }
    out.extend_from_slice(&(value.len() as u32).to_le_bytes());
    out.extend_from_slice(value.as_bytes());
    Ok(())
}

fn take<'a>(bytes: &'a [u8], cursor: &mut usize, count: usize) -> Result<&'a [u8], HbpError> {
    let end = cursor.checked_add(count).ok_or(HbpError::InvalidLength)?;
    if end > bytes.len() {
        return Err(HbpError::Truncated(0));
    }
    let value = &bytes[*cursor..end];
    *cursor = end;
    Ok(value)
}
fn take_u64(bytes: &[u8], cursor: &mut usize) -> Result<u64, HbpError> {
    Ok(u64::from_le_bytes(
        take(bytes, cursor, 8)?.try_into().unwrap(),
    ))
}
fn take_string(bytes: &[u8], cursor: &mut usize) -> Result<String, HbpError> {
    let length = u32::from_le_bytes(take(bytes, cursor, 4)?.try_into().unwrap()) as usize;
    if length > MAX_FIELD_BYTES {
        return Err(HbpError::FieldTooLarge);
    }
    String::from_utf8(take(bytes, cursor, length)?.to_vec()).map_err(|_| HbpError::InvalidUtf8)
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0xf) as usize] as char);
    }
    output
}
fn hex_decode(value: &str) -> Result<Vec<u8>, HbpError> {
    if value.len() % 2 != 0 {
        return Err(HbpError::InvalidHex);
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = (pair[0] as char).to_digit(16).ok_or(HbpError::InvalidHex)?;
            let low = (pair[1] as char).to_digit(16).ok_or(HbpError::InvalidHex)?;
            Ok((high * 16 + low) as u8)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_wire_round_trip_and_hash_chain() {
        let records = vec![
            HbpRecord {
                sequence: 0,
                payload: b"one".to_vec(),
            },
            HbpRecord {
                sequence: 1,
                payload: vec![0, 1, 255],
            },
        ];
        let bytes = encode_hbp(&records).unwrap();
        assert_eq!(&bytes[..4], b"HBP1");
        assert_eq!(decode_hbp(&bytes).unwrap(), records);
    }

    #[test]
    fn tamper_truncate_and_future_version_fail_closed() {
        let bytes = encode_hbp(&[HbpRecord {
            sequence: 0,
            payload: b"one".to_vec(),
        }])
        .unwrap();
        let mut tampered = bytes.clone();
        *tampered.last_mut().unwrap() ^= 1;
        assert_eq!(decode_hbp(&tampered), Err(HbpError::BrokenChain(0)));
        assert!(matches!(
            decode_hbp(&bytes[..bytes.len() - 1]),
            Err(HbpError::Truncated(0))
        ));
        let mut future = bytes;
        future[4] = 2;
        assert_eq!(decode_hbp(&future), Err(HbpError::UnsupportedVersion(2)));
    }
}
