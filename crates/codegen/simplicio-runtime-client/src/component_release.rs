//! Release provenance, compatibility negotiation, and atomic bundle slots.
//!
//! This module is deliberately local and deterministic: it never resolves a
//! floating version, downloads an artifact, starts a daemon, or changes a
//! session directory. A caller must provide the already-installed manifest
//! and the Runtime must announce matching provenance before it is trusted.

use base64::Engine as _;
use ring::signature::UnparsedPublicKey;
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
pub const RELEASE_EVENT_SCHEMA: &str = "simplicio.release-event/v1";
const RELEASE_EVENT_STATE_SCHEMA: &str = "simplicio.release-event-state/v1";
pub const CODE_VERSIONS_SCHEMA: &str = "simplicio.code-versions/v1";
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

/// Signed, immutable input to the Code release train. The event carries the
/// complete pinned manifest and its digest; Code never resolves a floating
/// release or invents an event locally.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseEvent {
    pub schema: String,
    pub event_id: String,
    pub producer: String,
    pub sequence: u64,
    pub manifest: BundleManifest,
    pub bundle_digest: String,
}

impl ReleaseEvent {
    pub fn validate(&self) -> Result<(), ReleaseError> {
        if self.schema != RELEASE_EVENT_SCHEMA {
            return Err(ReleaseError::InvalidEvent(format!(
                "schema must be {RELEASE_EVENT_SCHEMA}"
            )));
        }
        for (field, value) in [("event_id", &self.event_id), ("producer", &self.producer)] {
            if value.trim().is_empty() || value.len() > 256 || value.chars().any(char::is_whitespace)
            {
                return Err(ReleaseError::InvalidEvent(format!(
                    "{field} must be a non-empty single token"
                )));
            }
        }
        if self.sequence == 0 {
            return Err(ReleaseError::InvalidEvent(
                "sequence must be greater than zero".into(),
            ));
        }
        self.manifest.validate()?;
        for component in &self.manifest.components {
            if let Some(range) = self
                .manifest
                .compatibility
                .protocol_ranges
                .get(protocol_family(&component.protocol))
                && !range.accepts(&component.protocol)
            {
                return Err(ReleaseError::Incompatible(format!(
                    "{} protocol {} is outside the supported range {}..{}",
                    component.name, component.protocol, range.min, range.max
                )));
            }
        }
        if !is_sha256_digest(&self.bundle_digest) {
            return Err(ReleaseError::InvalidEvent(
                "bundle_digest must be a lowercase SHA-256 digest".into(),
            ));
        }
        if self.manifest.digest()? != self.bundle_digest {
            return Err(ReleaseError::InvalidEvent(
                "bundle_digest does not match the canonical manifest".into(),
            ));
        }
        Ok(())
    }

    pub fn canonical_json(&self) -> Result<Vec<u8>, ReleaseError> {
        self.validate()?;
        let value = serde_json::to_value(self).map_err(ReleaseError::Json)?;
        canonical_json(&value).map_err(ReleaseError::Json)
    }
}

/// Signed envelope received from an external release publisher. `key_id` is
/// selected only from the caller-provided trust set; no key is accepted from
/// the event itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedReleaseEvent {
    pub key_id: String,
    pub signature: String,
    pub payload: ReleaseEvent,
}

impl SignedReleaseEvent {
    pub fn verify(&self, trusted_keys: &[(&str, &[u8])]) -> Result<&ReleaseEvent, ReleaseError> {
        if self.key_id.trim().is_empty() {
            return Err(ReleaseError::Signature("key_id is required".into()));
        }
        let (_, public_key) = trusted_keys
            .iter()
            .find(|(key_id, _)| *key_id == self.key_id)
            .ok_or_else(|| ReleaseError::UnknownKey(self.key_id.clone()))?;
        let signature = base64::engine::general_purpose::STANDARD
            .decode(self.signature.trim())
            .map_err(|_| ReleaseError::Signature("signature is not valid base64".into()))?;
        let payload = self.payload.canonical_json()?;
        UnparsedPublicKey::new(&ring::signature::ED25519, public_key)
            .verify(&payload, &signature)
            .map_err(|_| ReleaseError::Signature("signature does not verify".into()))?;
        Ok(&self.payload)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReleaseEventOutcome {
    Promoted(BundleReceipt),
    Duplicate(BundleReceipt),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ReleaseEventRecord {
    event_id: String,
    producer: String,
    sequence: u64,
    bundle_digest: String,
    active_digest: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ReleaseEventState {
    schema: String,
    events: Vec<ReleaseEventRecord>,
}

#[derive(Debug, thiserror::Error)]
pub enum ReleaseError {
    #[error("invalid release manifest: {0}")]
    InvalidManifest(String),
    #[error("incompatible release: {0}")]
    Incompatible(String),
    #[error("invalid release event: {0}")]
    InvalidEvent(String),
    #[error("release event signature error: {0}")]
    Signature(String),
    #[error("release event key is not trusted: {0}")]
    UnknownKey(String),
    #[error("release event conflicts with an already applied event: {0}")]
    EventConflict(String),
    #[error("release event is stale: {0}")]
    StaleEvent(String),
    #[error("release event state is invalid: {0}")]
    EventState(String),
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
        let _lock = UpdateLock::acquire(&self.root)?;
        self.promote_locked(digest, &mut canary)
    }

    fn promote_locked<F>(&self, digest: &str, canary: &mut F) -> Result<BundleReceipt, ReleaseError>
    where
        F: FnMut(&Path) -> Result<(), String>,
    {
        if !is_sha256_digest(digest) {
            return Err(ReleaseError::InvalidManifest(
                "bundle slot must be named by a lowercase SHA-256 digest".into(),
            ));
        }
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

    /// Verify and apply one externally signed release event. The event is
    /// checked before staging, deduplicated by durable event id, ordered by
    /// producer sequence, and promoted only through this BundleStore.
    pub fn ingest_release_event<F>(
        &self,
        event: &SignedReleaseEvent,
        trusted_keys: &[(&str, &[u8])],
        source: &Path,
        mut canary: F,
    ) -> Result<ReleaseEventOutcome, ReleaseError>
    where
        F: FnMut(&Path) -> Result<(), String>,
    {
        let payload = event.verify(trusted_keys)?;
        let _lock = UpdateLock::acquire(&self.root)?;
        let state_path = self.root.join("release-event-state.json");
        let state_exists = state_path.is_file();
        let mut state = self.load_event_state()?;
        if !state_exists && self.root.join("slots/active").is_dir() {
            return Err(ReleaseError::EventState(
                "release event history is missing for the active bundle".into(),
            ));
        }
        if let Some(previous) = state.events.iter().find(|record| record.event_id == payload.event_id) {
            if previous.bundle_digest == payload.bundle_digest
                && previous.producer == payload.producer
                && previous.sequence == payload.sequence
            {
                return Ok(ReleaseEventOutcome::Duplicate(self.current_receipt()?));
            }
            return Err(ReleaseError::EventConflict(payload.event_id.clone()));
        }
        if let Some(previous) = state
            .events
            .iter()
            .filter(|record| record.producer == payload.producer)
            .max_by_key(|record| record.sequence)
            && payload.sequence <= previous.sequence
        {
            return Err(ReleaseError::StaleEvent(format!(
                "producer {} sequence {} is not newer than {}",
                payload.producer, payload.sequence, previous.sequence
            )));
        }
        self.verify_active_receipt()?;
        let active_digest = self
            .root
            .join("slots/active")
            .is_dir()
            .then(|| read_manifest(&self.root.join("slots/active")))
            .transpose()?
            .map(|manifest| manifest.digest())
            .transpose()?;
        if active_digest.as_deref() == Some(payload.bundle_digest.as_str()) {
            let receipt = self.current_receipt()?;
            state.events.push(ReleaseEventRecord {
                event_id: payload.event_id.clone(),
                producer: payload.producer.clone(),
                sequence: payload.sequence,
                bundle_digest: payload.bundle_digest.clone(),
                active_digest: payload.bundle_digest.clone(),
            });
            self.persist_event_state(&state)?;
            return Ok(ReleaseEventOutcome::Duplicate(receipt));
        }
        let staged = self.stage(&payload.manifest, source)?;
        if staged != payload.bundle_digest {
            return Err(ReleaseError::InvalidEvent(
                "staged manifest digest differs from signed event".into(),
            ));
        }
        let receipt = self.promote_locked(&staged, &mut canary)?;
        state.events.push(ReleaseEventRecord {
            event_id: payload.event_id.clone(),
            producer: payload.producer.clone(),
            sequence: payload.sequence,
            bundle_digest: payload.bundle_digest.clone(),
            active_digest: receipt.active_digest.clone(),
        });
        self.persist_event_state(&state)?;
        Ok(ReleaseEventOutcome::Promoted(receipt))
    }

    fn load_event_state(&self) -> Result<ReleaseEventState, ReleaseError> {
        let path = self.root.join("release-event-state.json");
        if !path.exists() {
            return Ok(ReleaseEventState {
                schema: RELEASE_EVENT_STATE_SCHEMA.into(),
                events: Vec::new(),
            });
        }
        let state: ReleaseEventState = serde_json::from_slice(&fs::read(path)?)
            .map_err(|error| ReleaseError::EventState(error.to_string()))?;
        if state.schema != RELEASE_EVENT_STATE_SCHEMA {
            return Err(ReleaseError::EventState(
                "unsupported release event state schema".into(),
            ));
        }
        for record in &state.events {
            if record.event_id.trim().is_empty()
                || record.producer.trim().is_empty()
                || record.sequence == 0
                || !is_sha256_digest(&record.bundle_digest)
                || !is_sha256_digest(&record.active_digest)
            {
                return Err(ReleaseError::EventState(
                    "release event state contains an invalid record".into(),
                ));
            }
        }
        Ok(state)
    }

    fn persist_event_state(&self, state: &ReleaseEventState) -> Result<(), ReleaseError> {
        let temporary = self.root.join(".release-event-state.tmp");
        fs::create_dir_all(&self.root)?;
        fs::write(&temporary, serde_json::to_vec_pretty(state)?)?;
        fs::rename(temporary, self.root.join("release-event-state.json"))?;
        Ok(())
    }

    fn current_receipt(&self) -> Result<BundleReceipt, ReleaseError> {
        let path = self.root.join("bundle-receipt.json");
        if path.is_file() {
            let receipt: BundleReceipt = serde_json::from_slice(&fs::read(path)?)?;
            if receipt.schema != BUNDLE_RECEIPT_SCHEMA || !is_sha256_digest(&receipt.active_digest) {
                return Err(ReleaseError::EventState("invalid bundle receipt".into()));
            }
            return Ok(receipt);
        }
        let active = read_manifest(&self.root.join("slots/active"))?;
        Ok(BundleReceipt {
            schema: BUNDLE_RECEIPT_SCHEMA.into(),
            previous_digest: None,
            active_digest: active.digest()?,
        })
    }

    fn verify_active_receipt(&self) -> Result<(), ReleaseError> {
        let active = self.root.join("slots/active");
        if !active.is_dir() {
            return Ok(());
        }
        let active_digest = read_manifest(&active)?.digest()?;
        let receipt = self.current_receipt()?;
        if receipt.active_digest != active_digest {
            return Err(ReleaseError::EventState(
                "bundle receipt does not match the active manifest".into(),
            ));
        }
        Ok(())
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
    /// installed bundle. If a pinned manifest was persisted at the bundle
    /// root, compare against it; otherwise retain the historical installed
    /// bundle report.
    pub fn versions_json(&self) -> Result<serde_json::Value, ReleaseError> {
        let active = self.root.join("slots/active");
        if !active.is_dir() {
            return Ok(missing_bundle_report("active bundle is missing"));
        }
        let manifest = match read_manifest(&active) {
            Ok(manifest) => manifest,
            Err(error) => return Ok(blocked_report(format!("installed bundle: {error}"))),
        };
        let pinned = self.root.join("component-release.json");
        if pinned.is_file() {
            return self.doctor_versions_json(&pinned);
        }
        version_report(&manifest, &manifest, Vec::new())
    }

    /// Read a pinned `component-release/v1` manifest and compare it with the
    /// active installed bundle. This is intentionally local and read-only:
    /// it never resolves a release, downloads an artifact, or starts Runtime.
    /// Any missing, malformed, or incompatible state produces a non-ready
    /// report so callers can fail closed without parsing human diagnostics.
    pub fn doctor_versions_json(
        &self,
        pinned_manifest: &Path,
    ) -> Result<serde_json::Value, ReleaseError> {
        let pinned = match read_manifest_file(pinned_manifest) {
            Ok(manifest) => manifest,
            Err(error) => return Ok(blocked_report(format!("pinned manifest: {error}"))),
        };
        let active = self.root.join("slots/active");
        if !active.is_dir() {
            return Ok(blocked_report("active bundle is missing"));
        }
        let installed = match read_manifest(&active) {
            Ok(manifest) => manifest,
            Err(error) => return Ok(blocked_report(format!("installed bundle: {error}"))),
        };

        let mut drift = Vec::new();
        if pinned.bundle_version != installed.bundle_version {
            drift.push("bundle_version".to_owned());
        }
        if pinned.compatibility.code_protocol != installed.compatibility.code_protocol {
            drift.push("compatibility.code_protocol".to_owned());
        }
        if pinned.compatibility.protocol_ranges != installed.compatibility.protocol_ranges {
            drift.push("compatibility.protocol_ranges".to_owned());
        }
        for name in REQUIRED_COMPONENTS {
            let expected = pinned.component(name);
            let actual = installed.component(name);
            match (expected, actual) {
                (Some(expected), Some(actual)) => {
                    if expected.version != actual.version {
                        drift.push(format!("{name}.version"));
                    }
                    if expected.artifact_digest != actual.artifact_digest {
                        drift.push(format!("{name}.digest"));
                    }
                    if expected.protocol != actual.protocol {
                        drift.push(format!("{name}.protocol"));
                    }
                    if let Some(range) = pinned
                        .compatibility
                        .protocol_ranges
                        .get(protocol_family(&actual.protocol))
                        && !range.accepts(&actual.protocol)
                    {
                        drift.push(format!("{name}.protocol.incompatible"));
                    }
                }
                (Some(_), None) => drift.push(format!("{name}.missing")),
                (None, Some(_)) => drift.push(format!("{name}.unexpected")),
                (None, None) => drift.push(format!("{name}.missing")),
            }
        }

        let pinned_digest = pinned.digest()?;
        let installed_digest = installed.digest()?;
        if pinned_digest != installed_digest {
            drift.push("manifest_digest".to_owned());
        }
        version_report(&pinned, &installed, drift)
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
    read_manifest_file(&slot.join("component-release.json"))
}

fn read_manifest_file(path: &Path) -> Result<BundleManifest, ReleaseError> {
    let bytes = fs::read(path)?;
    let manifest: BundleManifest = serde_json::from_slice(&bytes)?;
    manifest.validate()?;
    Ok(manifest)
}

fn component_summary(component: Option<&ComponentRelease>) -> serde_json::Value {
    component.map_or(serde_json::Value::Null, |component| {
        let mut summary = BTreeMap::<String, serde_json::Value>::new();
        summary.insert("digest".into(), serde_json::json!(component.artifact_digest));
        summary.insert("protocol".into(), serde_json::json!(component.protocol));
        summary.insert("version".into(), serde_json::json!(component.version));
        serde_json::Value::Object(summary.into_iter().collect())
    })
}

fn manifest_summary(manifest: &BundleManifest) -> Result<serde_json::Value, ReleaseError> {
    let mut components = BTreeMap::<String, serde_json::Value>::new();
    for name in REQUIRED_COMPONENTS {
        components.insert(name.to_owned(), component_summary(manifest.component(name)));
    }
    let mut summary = BTreeMap::<String, serde_json::Value>::new();
    summary.insert("bundle_version".into(), serde_json::json!(manifest.bundle_version));
    summary.insert("components".into(), serde_json::json!(components));
    summary.insert(
        "manifest_digest".into(),
        serde_json::json!(manifest.digest()?),
    );
    summary.insert(
        "protocol".into(),
        serde_json::json!(manifest.compatibility.code_protocol),
    );
    Ok(serde_json::Value::Object(summary.into_iter().collect()))
}

fn version_report(
    pinned: &BundleManifest,
    installed: &BundleManifest,
    drift: Vec<String>,
) -> Result<serde_json::Value, ReleaseError> {
    let blocked = drift.iter().any(|entry| {
        entry.ends_with(".incompatible")
            || entry.ends_with(".missing")
            || entry.ends_with(".unexpected")
            || entry.starts_with("compatibility.")
    });
    let status = if blocked {
        "blocked"
    } else if drift.is_empty() {
        "ready"
    } else {
        "drift"
    };
    let next_action = if drift.is_empty() {
        "none"
    } else {
        "install or promote the pinned compatible bundle"
    };
    let mut report = BTreeMap::<String, serde_json::Value>::new();
    report.insert("drift".into(), serde_json::json!(drift));
    report.insert("installed".into(), serde_json::to_value(installed)?);
    report.insert(
        "installed_summary".into(),
        manifest_summary(installed)?,
    );
    report.insert(
        "manifest_digest".into(),
        serde_json::json!(installed.digest()?),
    );
    report.insert("next_action".into(), serde_json::json!(next_action));
    report.insert("pinned".into(), serde_json::to_value(pinned)?);
    report.insert("pinned_summary".into(), manifest_summary(pinned)?);
    report.insert("ready".into(), serde_json::json!(drift.is_empty()));
    report.insert("schema".into(), serde_json::json!(CODE_VERSIONS_SCHEMA));
    report.insert("status".into(), serde_json::json!(status));
    Ok(serde_json::Value::Object(report.into_iter().collect()))
}

fn blocked_report(reason: impl Into<String>) -> serde_json::Value {
    let reason = reason.into();
    let mut report = BTreeMap::<String, serde_json::Value>::new();
    report.insert("drift".into(), serde_json::json!([reason]));
    report.insert("installed".into(), serde_json::Value::Null);
    report.insert(
        "next_action".into(),
        serde_json::json!("install or promote the pinned compatible bundle"),
    );
    report.insert("pinned".into(), serde_json::Value::Null);
    report.insert("ready".into(), serde_json::json!(false));
    report.insert("schema".into(), serde_json::json!(CODE_VERSIONS_SCHEMA));
    report.insert("status".into(), serde_json::json!("blocked"));
    serde_json::Value::Object(report.into_iter().collect())
}

fn missing_bundle_report(reason: &str) -> serde_json::Value {
    blocked_report(reason)
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
    use ring::signature::KeyPair;

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

    #[test]
    fn doctor_versions_report_is_deterministic_and_exposes_component_drift() {
        let temp = tempfile::tempdir().unwrap();
        let store = BundleStore::new(temp.path().join("bundles"));
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("runtime.bin"), "runtime").unwrap();

        let pinned = manifest("RuntimeProtocol/v1");
        let pinned_path = temp.path().join("pinned.json");
        fs::write(&pinned_path, serde_json::to_vec(&pinned).unwrap()).unwrap();

        let mut installed = pinned.clone();
        installed.bundle_version = "0.3.1".into();
        installed
            .components
            .iter_mut()
            .find(|component| component.name == "runtime")
            .unwrap()
            .version = "0.3.1".into();
        installed
            .components
            .iter_mut()
            .find(|component| component.name == "runtime")
            .unwrap()
            .artifact_digest = "d".repeat(64);
        installed
            .components
            .iter_mut()
            .find(|component| component.name == "runtime")
            .unwrap()
            .protocol = "RuntimeProtocol/v2".into();
        let digest = store.stage(&installed, &source).unwrap();
        store.promote(&digest, |_| Ok(())).unwrap();

        let first = store.doctor_versions_json(&pinned_path).unwrap();
        let second = store.doctor_versions_json(&pinned_path).unwrap();
        assert_eq!(serde_json::to_vec(&first).unwrap(), serde_json::to_vec(&second).unwrap());
        assert_eq!(first["status"], "drift");
        assert_eq!(first["ready"], false);
        assert!(first["drift"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("runtime.version")));
        assert!(first["drift"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("runtime.digest")));
        assert!(first["drift"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("runtime.protocol")));
    }

    #[test]
    fn doctor_versions_fails_closed_for_missing_or_incompatible_bundle() {
        let temp = tempfile::tempdir().unwrap();
        let store = BundleStore::new(temp.path().join("bundles"));
        let mut pinned = manifest("RuntimeProtocol/v1");
        pinned.compatibility.protocol_ranges.insert(
            "RuntimeProtocol".into(),
            ProtocolRange { min: 1, max: 1 },
        );
        let pinned_path = temp.path().join("pinned.json");
        fs::write(&pinned_path, serde_json::to_vec(&pinned).unwrap()).unwrap();

        let missing = store.doctor_versions_json(&pinned_path).unwrap();
        assert_eq!(missing["status"], "blocked");
        assert_eq!(missing["ready"], false);

        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        let mut incompatible = pinned.clone();
        incompatible
            .components
            .iter_mut()
            .find(|component| component.name == "runtime")
            .unwrap()
            .protocol = "RuntimeProtocol/v2".into();
        let digest = store.stage(&incompatible, &source).unwrap();
        store.promote(&digest, |_| Ok(())).unwrap();

        let report = store.doctor_versions_json(&pinned_path).unwrap();
        assert_eq!(report["status"], "blocked");
        assert_eq!(report["ready"], false);
        assert!(report["drift"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("runtime.protocol.incompatible")));
    }

    fn signed_event(
        manifest: BundleManifest,
        event_id: &str,
        sequence: u64,
        key_pair: &ring::signature::Ed25519KeyPair,
    ) -> SignedReleaseEvent {
        let bundle_digest = manifest.digest().unwrap();
        let payload = ReleaseEvent {
            schema: RELEASE_EVENT_SCHEMA.into(),
            event_id: event_id.into(),
            producer: "runtime-release".into(),
            sequence,
            manifest,
            bundle_digest,
        };
        let signature = key_pair.sign(&payload.canonical_json().unwrap());
        SignedReleaseEvent {
            key_id: "release-key-1".into(),
            signature: base64::engine::general_purpose::STANDARD.encode(signature.as_ref()),
            payload,
        }
    }

    fn test_key_pair() -> ring::signature::Ed25519KeyPair {
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = ring::signature::Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
        ring::signature::Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap()
    }

    #[test]
    fn signed_event_requires_trusted_key_and_exact_payload() {
        let key_pair = test_key_pair();
        let event = signed_event(manifest("RuntimeProtocol/v1"), "evt-1", 1, &key_pair);
        let trusted = [("release-key-1", key_pair.public_key().as_ref())];
        event.verify(&trusted).unwrap();

        let mut tampered = event.clone();
        tampered.payload.event_id = "evt-tampered".into();
        assert!(matches!(
            tampered.verify(&trusted),
            Err(ReleaseError::Signature(_))
        ));
        assert!(matches!(
            event.verify(&[("other-key", key_pair.public_key().as_ref())]),
            Err(ReleaseError::UnknownKey(_))
        ));
    }

    #[test]
    fn release_event_promotion_is_durable_and_deduplicated() {
        let key_pair = test_key_pair();
        let trusted = [("release-key-1", key_pair.public_key().as_ref())];
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("runtime.bin"), "runtime").unwrap();
        let store = BundleStore::new(temp.path().join("bundles"));
        let event = signed_event(manifest("RuntimeProtocol/v1"), "evt-1", 1, &key_pair);
        let canary_calls = std::cell::Cell::new(0);
        let promoted = store
            .ingest_release_event(&event, &trusted, &source, |_| {
                canary_calls.set(canary_calls.get() + 1);
                Ok(())
            })
            .unwrap();
        assert!(matches!(promoted, ReleaseEventOutcome::Promoted(_)));
        assert_eq!(canary_calls.get(), 1);

        let duplicate = store
            .ingest_release_event(&event, &trusted, &source, |_| {
                canary_calls.set(canary_calls.get() + 1);
                Ok(())
            })
            .unwrap();
        assert!(matches!(duplicate, ReleaseEventOutcome::Duplicate(_)));
        assert_eq!(canary_calls.get(), 1, "duplicate events must not canary twice");
        assert!(temp.path().join("bundles/release-event-state.json").is_file());
    }

    #[test]
    fn stale_event_and_active_receipt_drift_fail_closed() {
        let key_pair = test_key_pair();
        let trusted = [("release-key-1", key_pair.public_key().as_ref())];
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        let store = BundleStore::new(temp.path().join("bundles"));
        let first = signed_event(manifest("RuntimeProtocol/v1"), "evt-2", 2, &key_pair);
        store
            .ingest_release_event(&first, &trusted, &source, |_| Ok(()))
            .unwrap();

        let stale = signed_event(manifest("RuntimeProtocol/v1"), "evt-1", 1, &key_pair);
        assert!(matches!(
            store.ingest_release_event(&stale, &trusted, &source, |_| Ok(())),
            Err(ReleaseError::StaleEvent(_))
        ));

        let receipt_path = temp.path().join("bundles/bundle-receipt.json");
        let mut receipt: serde_json::Value =
            serde_json::from_slice(&fs::read(&receipt_path).unwrap()).unwrap();
        receipt["active_digest"] = serde_json::Value::String("a".repeat(64));
        fs::write(&receipt_path, serde_json::to_vec(&receipt).unwrap()).unwrap();
        let next = signed_event(manifest("RuntimeProtocol/v1"), "evt-3", 3, &key_pair);
        assert!(matches!(
            store.ingest_release_event(&next, &trusted, &source, |_| Ok(())),
            Err(ReleaseError::EventState(_))
        ));
    }
}
