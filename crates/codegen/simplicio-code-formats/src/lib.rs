//! Internal, bounded formats used by Simplicio Code.
//!
//! JSON is deliberately absent from this crate. JSON compatibility belongs in
//! the caller's explicit edge adapter and must be normalized before entering
//! these types. HBI is implemented here as a versioned byte container; it is
//! not a claim that the external Runtime HBI v1 contract is already available.

mod atomic;
pub mod hbi;
pub mod hbp;
pub mod migration;
pub mod toml_config;

pub use atomic::write_atomically;
pub use hbi::{HBI_MAGIC, HBI_VERSION, HbiError, HbiReader, HbiSection, encode_hbi};
pub use hbp::{HBP_MAGIC, HBP_VERSION, HbpError, HbpRecord, decode_hbp, encode_hbp};
pub use migration::{MigrationOutcome, migrate_bytes_atomically};
pub use toml_config::{CodeConfig, ConfigError, RuntimeConfig, parse_code_config};
