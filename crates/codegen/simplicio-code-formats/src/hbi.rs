//! Runtime-compatible HBI v1 container primitives.
//!
//! The wire layout is intentionally identical to Runtime's `src/hbi` module:
//! fixed 112-byte header, 56-byte section directory entries, eight-byte
//! alignment, SHA-256 section checksums, and a SHA-256 aggregate integrity
//! stream. JSON is not part of this format.

use sha2::{Digest, Sha256};

pub const HBI_MAGIC: [u8; 8] = *b"HBI\0v1\0\0";
pub const HBI_VERSION: u16 = 1;
pub const HBI_ENDIAN_LITTLE: u8 = 1;
pub const HBI_ALIGNMENT: usize = 8;
pub const HBI_FIXED_HEADER_LEN: usize = 112;
pub const HBI_SECTION_ENTRY_LEN: usize = 56;
pub const HBI_MAX_SECTIONS: u32 = 4096;

const OFF_VERSION: usize = 8;
const OFF_HEADER_LEN: usize = 10;
const OFF_ENDIANNESS: usize = 12;
const OFF_ALIGNMENT: usize = 13;
const OFF_FLAGS: usize = 16;
const OFF_SECTION_COUNT: usize = 20;
const OFF_TOTAL_LEN: usize = 24;
const OFF_SCHEMA_ID: usize = 32;
const OFF_SCHEMA_FINGERPRINT: usize = 48;
const OFF_CONTENT_CHECKSUM: usize = 80;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum HbiError {
    #[error("truncated HBI {0}")]
    Truncated(&'static str),
    #[error("invalid HBI magic")]
    InvalidMagic,
    #[error("unsupported HBI version {0}")]
    UnsupportedVersion(u16),
    #[error("unsupported HBI endianness marker {0}")]
    UnsupportedEndianness(u8),
    #[error("invalid HBI alignment {0}")]
    InvalidAlignment(u8),
    #[error("HBI reserved header bytes must be zero")]
    NonZeroReserved,
    #[error("invalid HBI header length {0}")]
    InvalidHeaderLength(u16),
    #[error("HBI section count {0} exceeds limit")]
    TooManySections(u32),
    #[error("HBI total length {declared} does not match {actual}")]
    LengthMismatch { declared: u64, actual: usize },
    #[error("HBI length arithmetic overflow")]
    ArithmeticOverflow,
    #[error("HBI section offset {0} is not aligned")]
    MisalignedOffset(u64),
    #[error("HBI section at {offset} with length {length} is out of bounds")]
    OutOfBounds { offset: u64, length: u64 },
    #[error("HBI sections overlap")]
    OverlappingSections,
    #[error("HBI alignment padding is not zero")]
    NonZeroPadding,
    #[error("duplicate HBI section kind {0}")]
    DuplicateSectionKind(u32),
    #[error("HBI {0} checksum mismatch")]
    ChecksumMismatch(&'static str),
    #[error("HBI schema fingerprint mismatch")]
    SchemaFingerprintMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HbiSection {
    pub kind: u32,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HbiSectionWithFlags {
    pub kind: u32,
    pub flags: u32,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HbiSectionInfo {
    pub kind: u32,
    pub flags: u32,
    pub offset: u64,
    pub length: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HbiMetadata {
    pub flags: u32,
    pub schema_id: [u8; 16],
    pub schema_fingerprint: [u8; 32],
    pub total_len: u64,
}

/// Derive the stable Runtime HBI identity used by Code's string-named schemas.
/// The schema name is the canonical descriptor for Code-owned payloads.
pub fn schema_identity(schema: &str) -> ([u8; 16], [u8; 32]) {
    let digest = Sha256::digest(schema.as_bytes());
    let mut id = [0u8; 16];
    let mut fingerprint = [0u8; 32];
    id.copy_from_slice(&digest[..16]);
    fingerprint.copy_from_slice(&digest);
    (id, fingerprint)
}

pub fn encode_hbi(schema: &str, sections: &[HbiSection]) -> Result<Vec<u8>, HbiError> {
    let (schema_id, schema_fingerprint) = schema_identity(schema);
    let sections = sections
        .iter()
        .map(|section| HbiSectionWithFlags {
            kind: section.kind,
            flags: 0,
            bytes: section.bytes.clone(),
        })
        .collect::<Vec<_>>();
    encode_hbi_with_identity(schema_id, schema_fingerprint, 0, &sections)
}

/// Encode an HBI vector with the exact Runtime identity fields. This is the
/// cross-language seam used by conformance tests and integrations that already
/// own a 16-byte schema ID and 32-byte canonical schema fingerprint.
pub fn encode_hbi_with_identity(
    schema_id: [u8; 16],
    schema_fingerprint: [u8; 32],
    flags: u32,
    sections: &[HbiSectionWithFlags],
) -> Result<Vec<u8>, HbiError> {
    if sections.len() > HBI_MAX_SECTIONS as usize {
        return Err(HbiError::TooManySections(sections.len() as u32));
    }
    let mut sections = sections.to_vec();
    sections.sort_by_key(|section| section.kind);
    for pair in sections.windows(2) {
        if pair[0].kind == pair[1].kind {
            return Err(HbiError::DuplicateSectionKind(pair[0].kind));
        }
    }
    let directory_len = sections
        .len()
        .checked_mul(HBI_SECTION_ENTRY_LEN)
        .ok_or(HbiError::ArithmeticOverflow)?;
    let body_start = HBI_FIXED_HEADER_LEN
        .checked_add(directory_len)
        .ok_or(HbiError::ArithmeticOverflow)?;
    let mut total_len = body_start;
    for section in &sections {
        total_len = align_up(total_len)?;
        total_len = total_len
            .checked_add(section.bytes.len())
            .ok_or(HbiError::ArithmeticOverflow)?;
    }
    total_len = align_up(total_len)?;
    let total_len_u64 = u64::try_from(total_len).map_err(|_| HbiError::ArithmeticOverflow)?;
    let mut bytes = vec![0u8; total_len];
    bytes[..8].copy_from_slice(&HBI_MAGIC);
    put_u16(&mut bytes, OFF_VERSION, HBI_VERSION);
    put_u16(&mut bytes, OFF_HEADER_LEN, HBI_FIXED_HEADER_LEN as u16);
    bytes[OFF_ENDIANNESS] = HBI_ENDIAN_LITTLE;
    bytes[OFF_ALIGNMENT] = HBI_ALIGNMENT as u8;
    put_u32(&mut bytes, OFF_FLAGS, flags);
    put_u32(&mut bytes, OFF_SECTION_COUNT, sections.len() as u32);
    put_u64(&mut bytes, OFF_TOTAL_LEN, total_len_u64);
    bytes[OFF_SCHEMA_ID..OFF_SCHEMA_ID + 16].copy_from_slice(&schema_id);
    bytes[OFF_SCHEMA_FINGERPRINT..OFF_SCHEMA_FINGERPRINT + 32].copy_from_slice(&schema_fingerprint);

    let mut cursor = body_start;
    let mut aggregate = Sha256::new();
    aggregate.update(HBI_VERSION.to_le_bytes());
    aggregate.update((HBI_FIXED_HEADER_LEN as u16).to_le_bytes());
    aggregate.update([HBI_ENDIAN_LITTLE, HBI_ALIGNMENT as u8]);
    aggregate.update(flags.to_le_bytes());
    aggregate.update((sections.len() as u32).to_le_bytes());
    aggregate.update(total_len_u64.to_le_bytes());
    aggregate.update(schema_id);
    aggregate.update(schema_fingerprint);
    for (index, section) in sections.iter().enumerate() {
        cursor = align_up(cursor)?;
        let offset = u64::try_from(cursor).map_err(|_| HbiError::ArithmeticOverflow)?;
        let length =
            u64::try_from(section.bytes.len()).map_err(|_| HbiError::ArithmeticOverflow)?;
        let entry = HBI_FIXED_HEADER_LEN + index * HBI_SECTION_ENTRY_LEN;
        put_u32(&mut bytes, entry, section.kind);
        put_u32(&mut bytes, entry + 4, section.flags);
        put_u64(&mut bytes, entry + 8, offset);
        put_u64(&mut bytes, entry + 16, length);
        let checksum = Sha256::digest(&section.bytes);
        bytes[entry + 24..entry + 56].copy_from_slice(&checksum);
        bytes[cursor..cursor + section.bytes.len()].copy_from_slice(&section.bytes);
        aggregate.update(section.kind.to_le_bytes());
        aggregate.update(section.flags.to_le_bytes());
        aggregate.update(offset.to_le_bytes());
        aggregate.update(length.to_le_bytes());
        aggregate.update(&section.bytes);
        cursor = cursor
            .checked_add(section.bytes.len())
            .ok_or(HbiError::ArithmeticOverflow)?;
    }
    bytes[OFF_CONTENT_CHECKSUM..OFF_CONTENT_CHECKSUM + 32].copy_from_slice(&aggregate.finalize());
    Ok(bytes)
}

pub struct HbiReader<'a> {
    bytes: &'a [u8],
    metadata: HbiMetadata,
    sections: Vec<HbiSectionInfo>,
}

impl<'a> HbiReader<'a> {
    pub fn open(bytes: &'a [u8]) -> Result<Self, HbiError> {
        if bytes.len() < HBI_FIXED_HEADER_LEN {
            return Err(HbiError::Truncated("header"));
        }
        if bytes[..8] != HBI_MAGIC {
            return Err(HbiError::InvalidMagic);
        }
        let version = get_u16(bytes, OFF_VERSION)?;
        if version != HBI_VERSION {
            return Err(HbiError::UnsupportedVersion(version));
        }
        let header_len = get_u16(bytes, OFF_HEADER_LEN)? as usize;
        if header_len != HBI_FIXED_HEADER_LEN || header_len % HBI_ALIGNMENT != 0 {
            return Err(HbiError::InvalidHeaderLength(header_len as u16));
        }
        if bytes[OFF_ENDIANNESS] != HBI_ENDIAN_LITTLE {
            return Err(HbiError::UnsupportedEndianness(bytes[OFF_ENDIANNESS]));
        }
        if bytes[OFF_ALIGNMENT] as usize != HBI_ALIGNMENT {
            return Err(HbiError::InvalidAlignment(bytes[OFF_ALIGNMENT]));
        }
        if bytes[14] != 0 || bytes[15] != 0 {
            return Err(HbiError::NonZeroReserved);
        }
        let section_count = get_u32(bytes, OFF_SECTION_COUNT)?;
        if section_count > HBI_MAX_SECTIONS {
            return Err(HbiError::TooManySections(section_count));
        }
        let declared_len = get_u64(bytes, OFF_TOTAL_LEN)?;
        if declared_len > usize::MAX as u64 || declared_len as usize != bytes.len() {
            return Err(HbiError::LengthMismatch {
                declared: declared_len,
                actual: bytes.len(),
            });
        }
        let directory_len = (section_count as usize)
            .checked_mul(HBI_SECTION_ENTRY_LEN)
            .ok_or(HbiError::ArithmeticOverflow)?;
        let directory_end = header_len
            .checked_add(directory_len)
            .ok_or(HbiError::ArithmeticOverflow)?;
        if directory_end > bytes.len() {
            return Err(HbiError::Truncated("section directory"));
        }
        let mut sections = Vec::with_capacity(section_count as usize);
        for index in 0..section_count as usize {
            let entry = header_len + index * HBI_SECTION_ENTRY_LEN;
            let kind = get_u32(bytes, entry)?;
            let flags = get_u32(bytes, entry + 4)?;
            let offset = get_u64(bytes, entry + 8)?;
            let length = get_u64(bytes, entry + 16)?;
            if offset % HBI_ALIGNMENT as u64 != 0 {
                return Err(HbiError::MisalignedOffset(offset));
            }
            let end = offset
                .checked_add(length)
                .ok_or(HbiError::ArithmeticOverflow)?;
            if offset < directory_end as u64 || end > declared_len {
                return Err(HbiError::OutOfBounds { offset, length });
            }
            let checksum = &bytes[entry + 24..entry + 56];
            if Sha256::digest(&bytes[offset as usize..end as usize]).as_slice() != checksum {
                return Err(HbiError::ChecksumMismatch("section"));
            }
            sections.push(HbiSectionInfo {
                kind,
                flags,
                offset,
                length,
            });
        }
        let mut ordered = sections.clone();
        ordered.sort_by_key(|section| section.offset);
        let mut previous_end = directory_end as u64;
        for section in ordered {
            if section.offset < previous_end {
                return Err(HbiError::OverlappingSections);
            }
            if bytes[previous_end as usize..section.offset as usize]
                .iter()
                .any(|byte| *byte != 0)
            {
                return Err(HbiError::NonZeroPadding);
            }
            previous_end = section
                .offset
                .checked_add(section.length)
                .ok_or(HbiError::ArithmeticOverflow)?;
        }
        if bytes[previous_end as usize..].iter().any(|byte| *byte != 0) {
            return Err(HbiError::NonZeroPadding);
        }
        let mut aggregate = Sha256::new();
        aggregate.update(version.to_le_bytes());
        aggregate.update((header_len as u16).to_le_bytes());
        aggregate.update([HBI_ENDIAN_LITTLE, HBI_ALIGNMENT as u8]);
        aggregate.update(get_u32(bytes, OFF_FLAGS)?.to_le_bytes());
        aggregate.update(section_count.to_le_bytes());
        aggregate.update(declared_len.to_le_bytes());
        aggregate.update(&bytes[OFF_SCHEMA_ID..OFF_SCHEMA_ID + 16]);
        aggregate.update(&bytes[OFF_SCHEMA_FINGERPRINT..OFF_SCHEMA_FINGERPRINT + 32]);
        for section in &sections {
            aggregate.update(section.kind.to_le_bytes());
            aggregate.update(section.flags.to_le_bytes());
            aggregate.update(section.offset.to_le_bytes());
            aggregate.update(section.length.to_le_bytes());
            aggregate.update(
                &bytes[section.offset as usize..(section.offset + section.length) as usize],
            );
        }
        if aggregate.finalize().as_slice()
            != &bytes[OFF_CONTENT_CHECKSUM..OFF_CONTENT_CHECKSUM + 32]
        {
            return Err(HbiError::ChecksumMismatch("content"));
        }
        let mut schema_id = [0u8; 16];
        schema_id.copy_from_slice(&bytes[OFF_SCHEMA_ID..OFF_SCHEMA_ID + 16]);
        let mut schema_fingerprint = [0u8; 32];
        schema_fingerprint
            .copy_from_slice(&bytes[OFF_SCHEMA_FINGERPRINT..OFF_SCHEMA_FINGERPRINT + 32]);
        Ok(Self {
            bytes,
            metadata: HbiMetadata {
                flags: get_u32(bytes, OFF_FLAGS)?,
                schema_id,
                schema_fingerprint,
                total_len: declared_len,
            },
            sections,
        })
    }

    pub fn metadata(&self) -> HbiMetadata {
        self.metadata
    }
    pub fn sections(&self) -> &[HbiSectionInfo] {
        &self.sections
    }
    pub fn section_count(&self) -> usize {
        self.sections.len()
    }
    pub fn section(&self, index: usize) -> Option<(u32, &'a [u8])> {
        let section = self.sections.get(index)?;
        Some((
            section.kind,
            &self.bytes[section.offset as usize..(section.offset + section.length) as usize],
        ))
    }
    pub fn schema_matches(&self, schema: &str) -> bool {
        let (_, fingerprint) = schema_identity(schema);
        self.metadata.schema_fingerprint == fingerprint
    }
    pub fn verify_schema_fingerprint(&self, expected: &[u8; 32]) -> Result<(), HbiError> {
        if &self.metadata.schema_fingerprint == expected {
            Ok(())
        } else {
            Err(HbiError::SchemaFingerprintMismatch)
        }
    }
}

fn align_up(value: usize) -> Result<usize, HbiError> {
    let remainder = value % HBI_ALIGNMENT;
    if remainder == 0 {
        Ok(value)
    } else {
        value
            .checked_add(HBI_ALIGNMENT - remainder)
            .ok_or(HbiError::ArithmeticOverflow)
    }
}

fn get_u16(bytes: &[u8], offset: usize) -> Result<u16, HbiError> {
    let value = bytes
        .get(offset..offset + 2)
        .ok_or(HbiError::Truncated("header field"))?;
    Ok(u16::from_le_bytes([value[0], value[1]]))
}
fn get_u32(bytes: &[u8], offset: usize) -> Result<u32, HbiError> {
    let value = bytes
        .get(offset..offset + 4)
        .ok_or(HbiError::Truncated("header field"))?;
    Ok(u32::from_le_bytes(value.try_into().unwrap()))
}
fn get_u64(bytes: &[u8], offset: usize) -> Result<u64, HbiError> {
    let value = bytes
        .get(offset..offset + 8)
        .ok_or(HbiError::Truncated("header field"))?;
    Ok(u64::from_le_bytes(value.try_into().unwrap()))
}
fn put_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}
fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
fn put_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<u8> {
        encode_hbi(
            "example-schema/v1",
            &[
                HbiSection {
                    kind: 2,
                    bytes: b"blob".to_vec(),
                },
                HbiSection {
                    kind: 1,
                    bytes: b"strings".to_vec(),
                },
            ],
        )
        .unwrap()
    }

    #[test]
    fn runtime_wire_round_trip_is_deterministic() {
        let first = fixture();
        assert_eq!(first, fixture());
        let reader = HbiReader::open(&first).unwrap();
        assert!(reader.schema_matches("example-schema/v1"));
        assert_eq!(reader.section(0).unwrap().1, b"strings");
        assert_eq!(reader.metadata().total_len as usize, first.len());
    }

    #[test]
    fn rejects_truncation_magic_version_checksum_and_overlap() {
        let bytes = fixture();
        assert!(matches!(
            HbiReader::open(&bytes[..HBI_FIXED_HEADER_LEN - 1]),
            Err(HbiError::Truncated(_))
        ));
        let mut bad_magic = bytes.clone();
        bad_magic[0] = b'X';
        assert!(matches!(
            HbiReader::open(&bad_magic),
            Err(HbiError::InvalidMagic)
        ));
        let mut bad_version = bytes.clone();
        bad_version[OFF_VERSION] = 2;
        assert!(matches!(
            HbiReader::open(&bad_version),
            Err(HbiError::UnsupportedVersion(2))
        ));
        let mut corrupt = bytes.clone();
        let payload_offset = u64::from_le_bytes(
            corrupt[HBI_FIXED_HEADER_LEN + 8..HBI_FIXED_HEADER_LEN + 16]
                .try_into()
                .unwrap(),
        ) as usize;
        corrupt[payload_offset] ^= 1;
        assert!(matches!(
            HbiReader::open(&corrupt),
            Err(HbiError::ChecksumMismatch("section"))
        ));
        let mut overlap = bytes;
        let first = u64::from_le_bytes(
            overlap[HBI_FIXED_HEADER_LEN + 8..HBI_FIXED_HEADER_LEN + 16]
                .try_into()
                .unwrap(),
        );
        overlap[HBI_FIXED_HEADER_LEN + HBI_SECTION_ENTRY_LEN + 8
            ..HBI_FIXED_HEADER_LEN + HBI_SECTION_ENTRY_LEN + 16]
            .copy_from_slice(&first.to_le_bytes());
        assert!(HbiReader::open(&overlap).is_err());
    }

    #[test]
    fn matches_runtime_golden_vector_digest() {
        let encoded = encode_hbi_with_identity(
            *b"example-schema!!",
            [7; 32],
            3,
            &[
                HbiSectionWithFlags {
                    kind: 2,
                    flags: 0,
                    bytes: b"blob".to_vec(),
                },
                HbiSectionWithFlags {
                    kind: 1,
                    flags: 1,
                    bytes: b"strings".to_vec(),
                },
            ],
        )
        .unwrap();
        assert_eq!(encoded.len(), 240);
        assert_eq!(
            Sha256::digest(&encoded).as_slice(),
            &[
                0xb2, 0x59, 0x9e, 0x60, 0x64, 0x59, 0x7f, 0xe7, 0x4f, 0xac, 0x2f, 0x73, 0x11, 0x8c,
                0xc9, 0xb2, 0x78, 0xf2, 0xd6, 0x62, 0x3d, 0x34, 0x94, 0x91, 0xea, 0x3f, 0xba, 0xb5,
                0x82, 0xa5, 0x65, 0x6d,
            ]
        );
        let reader = HbiReader::open(&encoded).unwrap();
        assert_eq!(reader.section(0).unwrap(), (1, &b"strings"[..]));
        assert_eq!(reader.section(1).unwrap(), (2, &b"blob"[..]));
    }
}
