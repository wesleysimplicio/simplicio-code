//! Internal, bounded formats used by Simplicio Code.
//!
//! JSON is deliberately absent from this crate. JSON compatibility belongs in
//! the caller's explicit edge adapter and must be normalized before entering
//! these types. HBI is byte-compatible with Runtime HBI v1; schema names are
//! converted to the Runtime schema identity by `schema_identity`.

mod atomic;
pub mod hbi;
pub mod hbp;
pub mod migration;
pub mod toml_config;

pub use atomic::write_atomically;
pub use hbi::{
    HBI_ALIGNMENT, HBI_ENDIAN_LITTLE, HBI_FIXED_HEADER_LEN, HBI_MAGIC, HBI_MAX_SECTIONS,
    HBI_SECTION_ENTRY_LEN, HBI_VERSION, HbiError, HbiMetadata, HbiReader, HbiSection,
    HbiSectionInfo, HbiSectionWithFlags, encode_hbi, encode_hbi_with_identity, schema_identity,
};
pub use hbp::{HBP_MAGIC, HBP_VERSION, HbpError, HbpRecord, decode_hbp, encode_hbp};
pub use migration::{MigrationOutcome, migrate_bytes_atomically};
pub use toml_config::{CodeConfig, ConfigError, RuntimeConfig, parse_code_config};
