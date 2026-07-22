//! HBI v1 container primitives.
//!
//! The layout is intentionally small and language-neutral:
//!
//! ```text
//! header (64 bytes) | UTF-8 schema id | section directory | section bytes
//! ```
//!
//! All integers are little-endian. Every section has a full BLAKE3 checksum;
//! offsets and lengths are checked before any section slice is exposed. This
//! is the Code-side adapter until Runtime #3494 publishes the ecosystem HBI
//! conformance vectors.

use std::ops::Range;

pub const HBI_MAGIC: [u8; 4] = *b"HBI\0";
pub const HBI_VERSION: u16 = 1;
const HEADER_SIZE: usize = 64;
const DIRECTORY_ENTRY_SIZE: usize = 56;
const MAX_SCHEMA_BYTES: usize = 4096;
const MAX_SECTIONS: usize = 1_000_000;
const MAX_TOTAL_BYTES: usize = 256 * 1024 * 1024;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum HbiError {
    #[error("HBI is truncated: need {needed} bytes, have {actual}")]
    Truncated { needed: usize, actual: usize },
    #[error("invalid HBI magic")]
    BadMagic,
    #[error("unsupported HBI version {0}")]
    UnsupportedVersion(u16),
    #[error("invalid HBI header length {0}")]
    BadHeaderLength(u32),
    #[error("HBI schema id is too large")]
    SchemaTooLarge,
    #[error("HBI has too many sections")]
    TooManySections,
    #[error("HBI total length {declared} does not match input length {actual}")]
    TotalLength { declared: usize, actual: usize },
    #[error("HBI section {index} is out of bounds")]
    SectionOutOfBounds { index: usize },
    #[error("HBI sections overlap")]
    OverlappingSections,
    #[error("HBI schema fingerprint mismatch")]
    SchemaFingerprintMismatch,
    #[error("HBI section {index} checksum mismatch")]
    SectionChecksumMismatch { index: usize },
    #[error("HBI schema is not valid UTF-8")]
    InvalidSchema,
    #[error("HBI total length exceeds the safety limit")]
    TooLarge,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HbiSection {
    pub kind: u32,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Copy)]
struct DirectoryEntry {
    kind: u32,
    offset: usize,
    length: usize,
    checksum: [u8; 32],
}

/// Encodes one HBI file with a schema id and independently checksummed sections.
pub fn encode_hbi(schema: &str, sections: &[HbiSection]) -> Result<Vec<u8>, HbiError> {
    let schema_bytes = schema.as_bytes();
    if schema_bytes.len() > MAX_SCHEMA_BYTES {
        return Err(HbiError::SchemaTooLarge);
    }
    if sections.len() > MAX_SECTIONS {
        return Err(HbiError::TooManySections);
    }
    let header_len = HEADER_SIZE
        .checked_add(schema_bytes.len())
        .and_then(|n| n.checked_add(sections.len().checked_mul(DIRECTORY_ENTRY_SIZE)?))
        .ok_or(HbiError::TooLarge)?;
    let payload_len = sections.iter().try_fold(0usize, |sum, section| {
        sum.checked_add(section.bytes.len()).ok_or(HbiError::TooLarge)
    })?;
    let total_len = header_len.checked_add(payload_len).ok_or(HbiError::TooLarge)?;
    if total_len > MAX_TOTAL_BYTES {
        return Err(HbiError::TooLarge);
    }

    let mut output = vec![0u8; total_len];
    output[..4].copy_from_slice(&HBI_MAGIC);
    output[4..6].copy_from_slice(&HBI_VERSION.to_le_bytes());
    output[8..12].copy_from_slice(&(header_len as u32).to_le_bytes());
    output[12..20].copy_from_slice(&(total_len as u64).to_le_bytes());
    output[20..24].copy_from_slice(&(sections.len() as u32).to_le_bytes());
    output[24..26].copy_from_slice(&(schema_bytes.len() as u16).to_le_bytes());
    let fingerprint = blake3::hash(schema_bytes);
    output[28..60].copy_from_slice(fingerprint.as_bytes());
    output[HEADER_SIZE..HEADER_SIZE + schema_bytes.len()].copy_from_slice(schema_bytes);

    let mut directory_offset = HEADER_SIZE + schema_bytes.len();
    let mut payload_offset = header_len;
    for section in sections {
        let checksum = blake3::hash(&section.bytes);
        output[directory_offset..directory_offset + 4].copy_from_slice(&section.kind.to_le_bytes());
        output[directory_offset + 8..directory_offset + 16]
            .copy_from_slice(&(payload_offset as u64).to_le_bytes());
        output[directory_offset + 16..directory_offset + 24]
            .copy_from_slice(&(section.bytes.len() as u64).to_le_bytes());
        output[directory_offset + 24..directory_offset + 56].copy_from_slice(checksum.as_bytes());
        output[payload_offset..payload_offset + section.bytes.len()].copy_from_slice(&section.bytes);
        directory_offset += DIRECTORY_ENTRY_SIZE;
        payload_offset += section.bytes.len();
    }
    Ok(output)
}

pub struct HbiReader<'a> {
    bytes: &'a [u8],
    schema: &'a str,
    sections: Vec<DirectoryEntry>,
}

impl<'a> HbiReader<'a> {
    pub fn open(bytes: &'a [u8]) -> Result<Self, HbiError> {
        if bytes.len() < HEADER_SIZE {
            return Err(HbiError::Truncated { needed: HEADER_SIZE, actual: bytes.len() });
        }
        if bytes[..4] != HBI_MAGIC {
            return Err(HbiError::BadMagic);
        }
        let version = u16::from_le_bytes([bytes[4], bytes[5]]);
        if version != HBI_VERSION {
            return Err(HbiError::UnsupportedVersion(version));
        }
        let header_len = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
        let total_len = u64::from_le_bytes(bytes[12..20].try_into().unwrap()) as usize;
        let section_count = u32::from_le_bytes(bytes[20..24].try_into().unwrap()) as usize;
        let schema_len = u16::from_le_bytes(bytes[24..26].try_into().unwrap()) as usize;
        if schema_len > MAX_SCHEMA_BYTES { return Err(HbiError::SchemaTooLarge); }
        if section_count > MAX_SECTIONS { return Err(HbiError::TooManySections); }
        if total_len > MAX_TOTAL_BYTES { return Err(HbiError::TooLarge); }
        let expected_header = HEADER_SIZE
            .checked_add(schema_len)
            .and_then(|n| n.checked_add(section_count.checked_mul(DIRECTORY_ENTRY_SIZE)?))
            .ok_or(HbiError::TooLarge)?;
        if header_len != expected_header || header_len > bytes.len() {
            return Err(HbiError::BadHeaderLength(header_len as u32));
        }
        if total_len != bytes.len() { return Err(HbiError::TotalLength { declared: total_len, actual: bytes.len() }); }
        let schema_range = HEADER_SIZE..HEADER_SIZE + schema_len;
        let schema_bytes = &bytes[schema_range];
        let schema = std::str::from_utf8(schema_bytes).map_err(|_| HbiError::InvalidSchema)?;
        let fingerprint = blake3::hash(schema_bytes);
        if &bytes[28..60] != fingerprint.as_bytes() { return Err(HbiError::SchemaFingerprintMismatch); }

        let mut sections = Vec::with_capacity(section_count);
        let mut ranges: Vec<Range<usize>> = Vec::with_capacity(section_count);
        let mut directory_offset = HEADER_SIZE + schema_len;
        for index in 0..section_count {
            let end = directory_offset + DIRECTORY_ENTRY_SIZE;
            if end > header_len { return Err(HbiError::Truncated { needed: end, actual: header_len }); }
            let kind = u32::from_le_bytes(bytes[directory_offset..directory_offset + 4].try_into().unwrap());
            let offset = u64::from_le_bytes(bytes[directory_offset + 8..directory_offset + 16].try_into().unwrap()) as usize;
            let length = u64::from_le_bytes(bytes[directory_offset + 16..directory_offset + 24].try_into().unwrap()) as usize;
            let end_offset = offset.checked_add(length).ok_or(HbiError::SectionOutOfBounds { index })?;
            if offset < header_len || end_offset > total_len { return Err(HbiError::SectionOutOfBounds { index }); }
            let checksum: [u8; 32] = bytes[directory_offset + 24..directory_offset + 56].try_into().unwrap();
            let range = offset..end_offset;
            if ranges.iter().any(|prior| prior.start < range.end && range.start < prior.end) { return Err(HbiError::OverlappingSections); }
            if blake3::hash(&bytes[range.clone()]).as_bytes() != checksum.as_ref() { return Err(HbiError::SectionChecksumMismatch { index }); }
            ranges.push(range);
            sections.push(DirectoryEntry { kind, offset, length, checksum });
            directory_offset = end;
        }
        Ok(Self { bytes, schema, sections })
    }

    pub fn schema(&self) -> &'a str { self.schema }

    pub fn section_count(&self) -> usize { self.sections.len() }

    pub fn section(&self, index: usize) -> Option<(u32, &'a [u8])> {
        let entry = self.sections.get(index)?;
        let _ = entry.length;
        let _ = entry.checksum;
        Some((entry.kind, &self.bytes[entry.offset..entry.offset + entry.length]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_checksum_validation() {
        let bytes = encode_hbi("simplicio.map-result/v1", &[HbiSection { kind: 1, bytes: b"payload".to_vec() }]).unwrap();
        let reader = HbiReader::open(&bytes).unwrap();
        assert_eq!(reader.schema(), "simplicio.map-result/v1");
        assert_eq!(reader.section(0).unwrap().1, b"payload");
        let mut corrupt = bytes;
        *corrupt.last_mut().unwrap() ^= 1;
        assert!(matches!(HbiReader::open(&corrupt), Err(HbiError::SectionChecksumMismatch { .. })));
    }

    #[test]
    fn rejects_truncation_and_overlapping_sections() {
        let bytes = encode_hbi("schema", &[HbiSection { kind: 1, bytes: b"a".to_vec() }, HbiSection { kind: 2, bytes: b"b".to_vec() }]).unwrap();
        assert!(matches!(HbiReader::open(&bytes[..bytes.len() - 1]), Err(HbiError::TotalLength { .. })));
        let mut overlap = bytes;
        let directory = HEADER_SIZE + "schema".len();
        let first_offset = u64::from_le_bytes(overlap[directory + 8..directory + 16].try_into().unwrap());
        overlap[directory + DIRECTORY_ENTRY_SIZE + 8..directory + DIRECTORY_ENTRY_SIZE + 16].copy_from_slice(&first_offset.to_le_bytes());
        assert!(matches!(HbiReader::open(&overlap), Err(HbiError::OverlappingSections)));
    }
}
