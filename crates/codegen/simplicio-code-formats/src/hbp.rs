//! HBP v1 append-only, hash-chained records.

pub const HBP_MAGIC: [u8; 4] = *b"HBP\0";
pub const HBP_VERSION: u16 = 1;
const HEADER_SIZE: usize = 56;
const HASH_SIZE: usize = 32;
const MAX_RECORD_BYTES: usize = 64 * 1024 * 1024;

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
}

pub fn encode_hbp(records: &[HbpRecord]) -> Result<Vec<u8>, HbpError> {
    let mut output = Vec::new();
    let mut previous = [0u8; HASH_SIZE];
    for record in records {
        if record.payload.len() > MAX_RECORD_BYTES { return Err(HbpError::TooLarge); }
        let mut header = [0u8; HEADER_SIZE];
        header[..4].copy_from_slice(&HBP_MAGIC);
        header[4..6].copy_from_slice(&HBP_VERSION.to_le_bytes());
        header[8..16].copy_from_slice(&record.sequence.to_le_bytes());
        header[16..24].copy_from_slice(&(record.payload.len() as u64).to_le_bytes());
        header[24..56].copy_from_slice(&previous);
        let mut hasher = blake3::Hasher::new();
        hasher.update(&header);
        hasher.update(&record.payload);
        let hash = hasher.finalize();
        output.extend_from_slice(&header);
        output.extend_from_slice(&record.payload);
        output.extend_from_slice(hash.as_bytes());
        previous.copy_from_slice(hash.as_bytes());
    }
    Ok(output)
}

pub fn decode_hbp(bytes: &[u8]) -> Result<Vec<HbpRecord>, HbpError> {
    let mut offset = 0;
    let mut previous = [0u8; HASH_SIZE];
    let mut records = Vec::new();
    while offset < bytes.len() {
        if bytes.len() - offset < HEADER_SIZE { return Err(HbpError::Truncated(records.len() as u64)); }
        let header = &bytes[offset..offset + HEADER_SIZE];
        if &header[..4] != HBP_MAGIC.as_slice() { return Err(HbpError::BadMagic); }
        let version = u16::from_le_bytes(header[4..6].try_into().unwrap());
        if version != HBP_VERSION { return Err(HbpError::UnsupportedVersion(version)); }
        let sequence = u64::from_le_bytes(header[8..16].try_into().unwrap());
        let length = u64::from_le_bytes(header[16..24].try_into().unwrap()) as usize;
        if length > MAX_RECORD_BYTES { return Err(HbpError::TooLarge); }
        if header[24..56] != previous { return Err(HbpError::BrokenChain(sequence)); }
        let end = offset.checked_add(HEADER_SIZE).and_then(|n| n.checked_add(length)).and_then(|n| n.checked_add(HASH_SIZE)).ok_or(HbpError::InvalidLength)?;
        if end > bytes.len() { return Err(HbpError::Truncated(sequence)); }
        let payload = &bytes[offset + HEADER_SIZE..offset + HEADER_SIZE + length];
        let expected = &bytes[end - HASH_SIZE..end];
        let mut hasher = blake3::Hasher::new();
        hasher.update(header);
        hasher.update(payload);
        if hasher.finalize().as_bytes() != expected { return Err(HbpError::BrokenChain(sequence)); }
        previous.copy_from_slice(expected);
        records.push(HbpRecord { sequence, payload: payload.to_vec() });
        offset = end;
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_chain_round_trip_and_tamper_detection() {
        let records = vec![HbpRecord { sequence: 0, payload: b"one".to_vec() }, HbpRecord { sequence: 1, payload: b"two".to_vec() }];
        let mut bytes = encode_hbp(&records).unwrap();
        assert_eq!(decode_hbp(&bytes).unwrap(), records);
        bytes[HEADER_SIZE] ^= 1;
        assert!(matches!(decode_hbp(&bytes), Err(HbpError::BrokenChain(0))));
    }
}
