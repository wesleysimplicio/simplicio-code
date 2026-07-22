pub mod auto_update;
pub mod manifest_verify;
mod minimum_version;
pub mod version;

pub use auto_update::UpdateStatus;
pub use manifest_verify::{
    ArtifactEntry, ReleaseManifest, find_artifact, verify_artifact_checksum,
    verify_manifest_signature,
};
pub use minimum_version::enforce_minimum_version_or_exit;
pub use version::{UpdateConfig, channel_label, channel_name, write_version_cache};
