//! Release provenance, compatibility negotiation, and atomic bundle slots.
//!
//! This module is deliberately local and deterministic: it never resolves a
//! floating version, downloads an artifact, starts a daemon, or changes a
//! session directory. A caller must provide the already-installed manifest
//! and the Runtime must announce matching provenance before it is trusted.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io,
    path::{Path, PathBuf},
};

pub use crate::generated::ComponentRelease;

pub const COMPONENT_RELEASE_SCHEMA: &str = "simplicio.component-release/v1";
pub const COMPATIBILITY_HANDSHAKE_SCHEMA: &str = "simplicio.compatibility-handshake/v1";
pub const BUNDLE_RECEIPT_SCHEMA: &str = "simplicio.bundle-receipt/v1";
pub const REQUIRED_COMPONENTS: [&str; 4] = ["agent-contracts", "code", "loop-hub", "runtime"];

pub type ReleaseIdentity = ComponentRelease;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolRange {
    pub min: u32,
    pub max: u32,
}

impl ProtocolRange {
    pub fn accepts(&self, protocol: &str) -> bool {
        protocol_major(protocol)
            .is_some_and(|major| self.min <= major && major <= self.max)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompatibilityContract {
    pub code_protocol: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub protocol_ranges: BTreeMap<String, ProtocolRange>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleManifest {
    pub schema: String,
    pub bundle_version: String,
    pub components: Vec<ComponentRelease>,
    pub compatibility: CompatibilityContract,
}

impl BundleManifest {
    pub fn validate(&self) -> Result<(), ReleaseError> {
        if self.schema != COMPONENT_RELEASE_SCHEMA {
            return Err(ReleaseError::InvalidManifest(format!(
                "schema must be {COMPONENT_RELEASE_SCHEMA}"
            )));
        }
        reject_floating("bundle_version", &self.bundle_version)?;
        let mut names = BTreeSet::new();
        for component in &self.components {
            if !REQUIRED_COMPONENTS.contains(&component.name.as_str()) {
                return Err(ReleaseError::InvalidManifest(format!(
                    "unknown component {}",
                    component.name
                )));
            }
            if !names.insert(component.name.clone()) {
                return Err(ReleaseError::InvalidManifest(format!(
                    "duplicate component {}",
                    component.name
                )));
            }
            component.validate()?;
        }
        for required in REQUIRED_COMPONENTS {
            if !names.contains(required) {
                return Err(ReleaseError::InvalidManifest(format!(
                    "missing component {required}"
                )));
            }
        }
        if self.compatibility.code_protocol.trim().is_empty() {
            return Err(ReleaseError::InvalidManifest(
                "compatibility.code_protocol is required".into(),
            ));
        }
        for (family, range) in &self.compatibility.protocol_ranges {
            if range.min > range.max || family.trim().is_empty() {
                return Err(ReleaseError::InvalidManifest(format!(
                    "invalid protocol range for {family}"
                )));
            }
        }
        Ok(())
    }

    pub fn component(&self, name: &str) -> Option<&ComponentRelease> {
        self.components.iter().find(|component| component.name == name)
    }

    /// SHA-256 over canonical JSON. BTreeMap fields and recursively sorted
    /// JSON objects make the result independent of input key order.
    pub fn digest(&self) -> Result<String, ReleaseError> {
        self.validate()?;
        let value = serde_json::to_value(self).map_err(ReleaseError::Json)?;
        let canonical = canonical_json(&value).map_err(ReleaseError::Json)?;
        Ok(hex_digest(&canonical))
    }

    pub fn canonical_json(&self) -> Result<Vec<u8>, ReleaseError> {
        self.validate()?;
        let value = serde_json::to_value(self).map_err(ReleaseError::Json)?;
        canonical_json(&value).map_err(ReleaseError::Json)
    }
}

impl ComponentRelease {
    fn validate(&self) -> Result<(), ReleaseError> {
        reject_floating(&format!("{}.version", self.name), &self.version)?;
        if self.commit.len() < 7
            || self.commit.len() > 40
            || !self.commit.bytes().all(|byte| byte.is_ascii_hexdigit())
            || self.commit.bytes().any(|byte| byte.is_ascii_uppercase())
        {
            return Err(ReleaseError::InvalidManifest(format!(
                "{}.commit must be lowercase hexadecimal",
                self.name
            )));
        }
        if self.artifact_digest.len() != 64
            || !self.artifact_digest.bytes().all(|byte| byte.is_ascii_hexdigit())
            || self.artifact_digest.bytes().any(|byte| byte.is_ascii_uppercase())
        {
            return Err(ReleaseError::InvalidManifest(format!(
                "{}.artifact_digest must be a SHA-256 digest",
                self.name
            )));
        }
        if self.protocol.trim().is_empty() {
            return Err(ReleaseError::InvalidManifest(format!(
                "{}.protocol is required",
                self.name
            )));
        }
        if let Some(digest) = &self.generated_client_digest
            && (digest.len() != 64
                || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
                || digest.bytes().any(|byte| byte.is_ascii_uppercase()))
        {
            return Err(ReleaseError::InvalidManifest(format!(
                "{}.generated_client_digest must be a SHA-256 digest",
                self.name
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompatibilityHandshake {
    pub schema: String,
    pub component: ComponentRelease,
    pub bundle_digest: String,
    #[serde(default)]
    pub capabilities: BTreeSet<String>,
}

impl CompatibilityHandshake {
    pub fn from_runtime(component: &ReleaseIdentity, capabilities: &BTreeSet<String>) -> Self {
        Self {
            schema: COMPATIBILITY_HANDSHAKE_SCHEMA.into(),
            component: component.clone(),
            bundle_digest: String::new(),
            capabilities: capabilities.clone(),
        }
    }

    pub fn from_manifest(manifest: &BundleManifest, capabilities: BTreeSet<String>) -> Result<Self, ReleaseError> {
        Ok(Self {
            schema: COMPATIBILITY_HANDSHAKE_SCHEMA.into(),
            component: manifest
                .component("code")
                .cloned()
                .ok_or_else(|| ReleaseError::InvalidManifest("code component is missing".into()))?,
            bundle_digest: manifest.digest()?,
            capabilities,
        })
    }

    pub fn verify_against(&self, manifest: &BundleManifest) -> Result<(), ReleaseError> {
        manifest.validate()?;
        if self.schema != COMPATIBILITY_HANDSHAKE_SCHEMA {
            return Err(ReleaseError::Incompatible("unsupported handshake schema".into()));
        }
        let expected = manifest.component("runtime").ok_or_else(|| {
            ReleaseError::InvalidManifest("runtime component is missing".into())
        })?;
        if self.component.name != expected.name
            || self.component.version != expected.version
            || self.component.commit != expected.commit
            || self.component.artifact_digest != expected.artifact_digest
        {
            return Err(ReleaseError::Incompatible(format!(
                "Runtime release does not match pinned version/commit/digest (expected {}, {})",
                expected.version, expected.artifact_digest
            )));
        }
        if let Some(range) = manifest
            .compatibility
            .protocol_ranges
            .get(protocol_family(&self.component.protocol))
            && !range.accepts(&self.component.protocol)
        {
            return Err(ReleaseError::Incompatible(format!(
                "protocol {} is outside the supported range {}..{}",
                self.component.protocol, range.min, range.max
            )));
        }
        if !self.bundle_digest.is_empty() && self.bundle_digest != manifest.digest()? {
            return Err(ReleaseError::Incompatible(
                "handshake bundle digest differs from installed manifest".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleReceipt {
    pub schema: String,
    pub previous_digest: Option<String>,
    pub active_digest: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ReleaseError {
    #[error("invalid release manifest: {0}")]
    InvalidManifest(String),
    #[error("incompatible release: {0}")]
    Incompatible(String),
    #[error("bundle canary rejected: {0}")]
    CanaryRejected(String),
    #[error("bundle update is already in progress")]
    UpdateBusy,
    #[error("bundle has no previous active slot")]
    NoPreviousBundle,
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("filesystem error: {0}")]
    Io(#[from] io::Error),
}

/// Filesystem-backed inactive/active/previous slots. Only the slots and a
/// lock marker are touched; session/config directories are outside this
/// lifecycle and therefore survive promotion and rollback unchanged.
#[derive(Debug, Clone)]
pub struct BundleStore {
    root: PathBuf,
}

impl BundleStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn stage(&self, manifest: &BundleManifest, source: &Path) -> Result<String, ReleaseError> {
        manifest.validate()?;
        if !source.is_dir() {
            return Err(ReleaseError::Io(io::Error::new(
                io::ErrorKind::NotFound,
                format!("bundle source is not a directory: {}", source.display()),
            )));
        }
        let digest = manifest.digest()?;
        let slots = self.root.join("slots");
        fs::create_dir_all(&slots)?;
        let temporary = slots.join(format!(".staging-{digest}"));
        let final_slot = slots.join(&digest);
        if final_slot.exists() {
            return Ok(digest);
        }
        remove_dir_if_present(&temporary)?;
        copy_directory(source, &temporary)?;
        fs::write(
            temporary.join("component-release.json"),
            serde_json::to_vec_pretty(manifest)?,
        )?;
        fs::rename(temporary, final_slot)?;
        Ok(digest)
    }

    pub fn promote<F>(&self, digest: &str, mut canary: F) -> Result<BundleReceipt, ReleaseError>
    where
        F: FnMut(&Path) -> Result<(), String>,
    {
        if !is_sha256_digest(digest) {
            return Err(ReleaseError::InvalidManifest(
                "bundle slot must be named by a lowercase SHA-256 digest".into(),
            ));
        }
        let _lock = UpdateLock::acquire(&self.root)?;
        let slots = self.root.join("slots");
        let candidate = slots.join(digest);
        let manifest = read_manifest(&candidate)?;
        if manifest.digest()? != digest {
            return Err(ReleaseError::InvalidManifest(
                "staged slot name does not match its manifest digest".into(),
            ));
        }
        canary(&candidate).map_err(ReleaseError::CanaryRejected)?;

        fs::create_dir_all(&slots)?;
        let active = slots.join("active");
        let previous = slots.join("previous");
        remove_dir_if_present(&previous)?;
        let previous_digest = if active.exists() {
            let old = read_manifest(&active)?.digest()?;
            fs::rename(&active, &previous)?;
            Some(old)
        } else {
            None
        };
        fs::rename(&candidate, &active)?;
        let receipt = BundleReceipt {
            schema: BUNDLE_RECEIPT_SCHEMA.into(),
            previous_digest,
            active_digest: digest.into(),
        };
        fs::create_dir_all(&self.root)?;
        fs::write(
            self.root.join("bundle-receipt.json"),
            serde_json::to_vec_pretty(&receipt)?,
        )?;
        Ok(receipt)
    }

    pub fn rollback(&self) -> Result<BundleReceipt, ReleaseError> {
        let _lock = UpdateLock::acquire(&self.root)?;
        let slots = self.root.join("slots");
        let active = slots.join("active");
        let previous = slots.join("previous");
        if !previous.is_dir() {
            return Err(ReleaseError::NoPreviousBundle);
        }
        let failed = slots.join(".rollback-active");
        remove_dir_if_present(&failed)?;
        if active.exists() {
            fs::rename(&active, &failed)?;
        }
        fs::rename(&previous, &active)?;
        let active_digest = read_manifest(&active)?.digest()?;
        let previous_digest = if failed.exists() {
            let digest = read_manifest(&failed)?.digest()?;
            fs::rename(failed, &previous)?;
            Some(digest)
        } else {
            None
        };
        let receipt = BundleReceipt {
            schema: BUNDLE_RECEIPT_SCHEMA.into(),
            previous_digest,
            active_digest,
        };
        fs::write(
            self.root.join("bundle-receipt.json"),
            serde_json::to_vec_pretty(&receipt)?,
        )?;
        Ok(receipt)
    }

    /// Repair the only non-destructive interrupted-promotion state: the old
    /// active slot was moved to `previous` but the candidate was not installed.
    /// The candidate is left untouched for inspection or a later retry.
    pub fn recover(&self) -> Result<bool, ReleaseError> {
        let _lock = UpdateLock::acquire(&self.root)?;
        let slots = self.root.join("slots");
        let active = slots.join("active");
        let previous = slots.join("previous");
        if active.exists() || !previous.is_dir() {
            return Ok(false);
        }
        fs::rename(previous, active)?;
        Ok(true)
    }

    /// Machine-readable equivalent of `code doctor/versions --json` for an
    /// installed bundle. No network lookup is performed; missing active state
    /// is reported as drift with an actionable next step.
    pub fn versions_json(&self) -> Result<serde_json::Value, ReleaseError> {
        let active = self.root.join("slots/active");
        if !active.is_dir() {
            return Ok(serde_json::json!({
                "schema": "simplicio.code-versions/v1",
                "status": "drift",
                "installed": serde_json::Value::Null,
                "manifest_digest": serde_json::Value::Null,
                "next_action": "stage and canary a pinned component-release/v1 bundle"
            }));
        }
        let manifest = read_manifest(&active)?;
        let digest = manifest.digest()?;
        Ok(serde_json::json!({
            "schema": "simplicio.code-versions/v1",
            "status": "ready",
            "installed": manifest,
            "manifest_digest": digest,
            "next_action": "none"
        }))
    }

    pub fn active_manifest(&self) -> Result<BundleManifest, ReleaseError> {
        read_manifest(&self.root.join("slots/active"))
    }
}

struct UpdateLock {
    path: PathBuf,
}

impl UpdateLock {
    fn acquire(root: &Path) -> Result<Self, ReleaseError> {
        fs::create_dir_all(root)?;
        let path = root.join(".update.lock");
        fs::create_dir(&path).map_err(|error| {
            if error.kind() == io::ErrorKind::AlreadyExists {
                ReleaseError::UpdateBusy
            } else {
                ReleaseError::Io(error)
            }
        })?;
        Ok(Self { path })
    }
}

impl Drop for UpdateLock {
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.path);
    }
}

fn reject_floating(field: &str, value: &str) -> Result<(), ReleaseError> {
    if value.trim().is_empty() || matches!(value, "latest" | "main" | "dev") {
        return Err(ReleaseError::InvalidManifest(format!(
            "{field} must be a pinned value"
        )));
    }
    Ok(())
}

fn protocol_family(protocol: &str) -> &str {
    protocol.rsplit_once("/v").map_or(protocol, |(family, _)| family)
}

fn protocol_major(protocol: &str) -> Option<u32> {
    protocol.rsplit_once("/v")?.1.parse().ok()
}

fn is_sha256_digest(value: &str) -> bool {
    value.len() == 64
        && value.bytes().all(|byte| byte.is_ascii_hexdigit())
        && value.bytes().all(|byte| !byte.is_ascii_uppercase())
}

fn canonical_json(value: &serde_json::Value) -> Result<Vec<u8>, serde_json::Error> {
    match value {
        serde_json::Value::Object(object) => {
            let sorted = object
                .iter()
                .map(|(key, value)| Ok((key.clone(), serde_json::from_slice(&canonical_json(value)?)?)))
                .collect::<Result<BTreeMap<_, _>, serde_json::Error>>()?;
            serde_json::to_vec(&sorted)
        }
        serde_json::Value::Array(values) => {
            let values = values
                .iter()
                .map(canonical_json)
                .map(|bytes| bytes.and_then(|bytes| serde_json::from_slice(&bytes)))
                .collect::<Result<Vec<serde_json::Value>, serde_json::Error>>()?;
            serde_json::to_vec(&values)
        }
        _ => serde_json::to_vec(value),
    }
}

fn hex_digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn read_manifest(slot: &Path) -> Result<BundleManifest, ReleaseError> {
    let bytes = fs::read(slot.join("component-release.json"))?;
    let manifest: BundleManifest = serde_json::from_slice(&bytes)?;
    manifest.validate()?;
    Ok(manifest)
}

fn remove_dir_if_present(path: &Path) -> Result<(), io::Error> {
    if path.is_dir() {
        fs::remove_dir_all(path)?;
    }
    Ok(())
}

fn copy_directory(source: &Path, destination: &Path) -> Result<(), io::Error> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            copy_directory(&source_path, &destination_path)?;
        } else if file_type.is_file() {
            fs::copy(source_path, destination_path)?;
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "bundle sources may not contain symlinks or special files",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(runtime_protocol: &str) -> BundleManifest {
        BundleManifest {
            schema: COMPONENT_RELEASE_SCHEMA.into(),
            bundle_version: "0.3.0-beta.2".into(),
            components: REQUIRED_COMPONENTS
                .iter()
                .map(|name| ComponentRelease {
                    name: (*name).into(),
                    version: "0.3.0".into(),
                    commit: "a".repeat(40),
                    protocol: if *name == "runtime" {
                        runtime_protocol.into()
                    } else {
                        format!("{name}/v1")
                    },
                    artifact_digest: "b".repeat(64),
                    generated_client_digest: Some("c".repeat(64)),
                })
                .collect(),
            compatibility: CompatibilityContract {
                code_protocol: "CoordinatorProtocol/v1".into(),
                protocol_ranges: BTreeMap::from([(
                    "RuntimeProtocol".into(),
                    ProtocolRange { min: 1, max: 2 },
                )]),
            },
        }
    }

    #[test]
    fn manifest_digest_is_independent_of_json_key_order() {
        let manifest = manifest("RuntimeProtocol/v1");
        let json = manifest.canonical_json().unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&json).unwrap();
        assert_eq!(manifest.digest().unwrap(), hex_digest(&json));
        assert_eq!(parsed["schema"], COMPONENT_RELEASE_SCHEMA);
    }

    #[test]
    fn handshake_accepts_n_and_n_minus_one_but_rejects_n_plus_one() {
        let manifest = manifest("RuntimeProtocol/v1");
        let mut identity = manifest.component("runtime").unwrap().clone();
        let mut handshake = CompatibilityHandshake::from_runtime(&identity, &BTreeSet::new());
        handshake.verify_against(&manifest).unwrap();

        identity.protocol = "RuntimeProtocol/v2".into();
        handshake = CompatibilityHandshake::from_runtime(&identity, &BTreeSet::new());
        handshake.verify_against(&manifest).unwrap();

        identity.protocol = "RuntimeProtocol/v3".into();
        handshake = CompatibilityHandshake::from_runtime(&identity, &BTreeSet::new());
        assert!(matches!(
            handshake.verify_against(&manifest),
            Err(ReleaseError::Incompatible(_))
        ));
    }

    #[test]
    fn wrong_digest_is_rejected_before_bundle_promotion() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("runtime.bin"), "runtime").unwrap();
        let store = BundleStore::new(temp.path().join("bundles"));
        let manifest = manifest("RuntimeProtocol/v1");
        let digest = store.stage(&manifest, &source).unwrap();
        let manifest_path = temp
            .path()
            .join("bundles/slots")
            .join(&digest)
            .join("component-release.json");
        let mut json: serde_json::Value = serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        json["components"][0]["artifact_digest"] = serde_json::Value::String("d".repeat(64));
        fs::write(&manifest_path, serde_json::to_vec(&json).unwrap()).unwrap();
        assert!(matches!(
            store.promote(&digest, |_| Ok(())),
            Err(ReleaseError::InvalidManifest(_))
        ));
        assert!(!temp.path().join("bundles/slots/active").exists());
    }

    #[test]
    fn canary_promotion_and_rollback_preserve_sessions() {
        let temp = tempfile::tempdir().unwrap();
        let store = BundleStore::new(temp.path().join("bundles"));
        fs::create_dir_all(temp.path().join("bundles/sessions")) .unwrap();
        fs::write(temp.path().join("bundles/sessions/session.json"), "keep").unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("runtime.bin"), "runtime").unwrap();

        let first = manifest("RuntimeProtocol/v1");
        let first_digest = store.stage(&first, &source).unwrap();
        store.promote(&first_digest, |_| Ok(())).unwrap();

        let mut second = first.clone();
        second.bundle_version = "0.3.1".into();
        let second_digest = store.stage(&second, &source).unwrap();
        let rejected = store.promote(&second_digest, |_| Err("probe failed".into()));
        assert!(matches!(rejected, Err(ReleaseError::CanaryRejected(_))));
        assert_eq!(store.active_manifest().unwrap().bundle_version, "0.3.0-beta.2");

        store.promote(&second_digest, |_| Ok(())).unwrap();
        assert_eq!(store.active_manifest().unwrap().bundle_version, "0.3.1");
        store.rollback().unwrap();
        assert_eq!(store.active_manifest().unwrap().bundle_version, "0.3.0-beta.2");
        assert_eq!(fs::read_to_string(temp.path().join("bundles/sessions/session.json")).unwrap(), "keep");
    }
}
