//! Signed release-manifest verification for the **simplicio-code** release
//! channel (issue simplicio-loop-issues #8).
//!
//! This module is intentionally additive and self-contained: it does not
//! change anything about the existing `auto_update` flow, which still talks
//! to the inherited x.ai / `grok-build` GCS + GitHub infrastructure (see
//! [`crate::version::CLI_BASE_URL_PRIMARY`], [`crate::version::GH_RELEASE_REPO`]).
//! That flow is a large, separately-tested piece of upstream product
//! infrastructure and rewiring it to point at simplicio-code's own GitHub
//! Releases is out of scope for this change; see the PR description for
//! what remains open.
//!
//! What this module *does* provide, ready to be wired into a
//! simplicio-code-specific updater path:
//!
//! - A `ReleaseManifest` JSON schema listing, per platform, the release
//!   artifact filename and its SHA-256 checksum.
//! - Ed25519 signature verification over the raw manifest bytes, so a
//!   tampered manifest (or a manifest signed with the wrong key) is
//!   rejected before any artifact is trusted.
//! - SHA-256 checksum verification for a downloaded artifact against the
//!   value recorded (and signed) in the manifest, so a truncated or
//!   substituted binary is rejected even if the manifest itself is valid.
//!
//! ## Key management
//!
//! This module takes the public key as a parameter rather than embedding
//! one — the tests below each generate a throwaway Ed25519 keypair on the
//! fly and never share a fixture key. Production key custody (generation,
//! HSM- or CI-secret-backed storage, rotation, revocation, and how the
//! trusted public key gets embedded in shipped binaries) is an org/infra
//! decision outside the scope of a single coding session — see the PR
//! description for what remains open.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// A release manifest for one version of `simplicio-code`, listing every
/// published platform artifact and its checksum. This is the payload that
/// gets Ed25519-signed; the signature travels alongside it (e.g. as
/// `manifest.json` + `manifest.json.sig`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseManifest {
    /// e.g. "0.3.0-beta.1"
    pub version: String,
    /// e.g. "beta" or "stable". A beta manifest must never be accepted by a
    /// client that only trusts "stable" — enforced by callers, not this
    /// module, since only the caller knows which channel it asked for.
    pub channel: String,
    pub artifacts: Vec<ArtifactEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactEntry {
    /// e.g. "linux-x86_64", "macos-aarch64", "windows-x86_64"
    pub platform: String,
    pub filename: String,
    /// Lowercase hex-encoded SHA-256 digest of the artifact file.
    pub sha256: String,
}

/// Verify an Ed25519 signature over `manifest_bytes` and, only if it
/// checks out, parse and return the [`ReleaseManifest`].
///
/// `signature_b64` and `public_key_b64` are standard-alphabet base64: a
/// 64-byte raw Ed25519 signature and a 32-byte raw Ed25519 public key,
/// respectively (the format `openssl pkeyutl -sign -rawin` /
/// `openssl pkey -pubout` produce, and what `ring::signature::ED25519`
/// verifies directly).
///
/// Any tampering — a flipped byte in the manifest, a corrupted signature,
/// or a signature made with a different key — is rejected with an error
/// before the manifest is trusted or parsed for use.
pub fn verify_manifest_signature(
    manifest_bytes: &[u8],
    signature_b64: &str,
    public_key_b64: &str,
) -> Result<ReleaseManifest> {
    use base64::Engine as _;
    let engine = base64::engine::general_purpose::STANDARD;

    let signature = engine
        .decode(signature_b64.trim())
        .context("signature is not valid base64")?;
    let public_key = engine
        .decode(public_key_b64.trim())
        .context("public key is not valid base64")?;

    let key = ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, &public_key);
    key.verify(manifest_bytes, &signature).map_err(|_| {
        anyhow::anyhow!("manifest signature verification failed (tampered or wrong key)")
    })?;

    serde_json::from_slice(manifest_bytes).context("signed manifest is not valid JSON")
}

/// Verify that `data` hashes to `expected_sha256_hex` (case-insensitive hex).
/// Used to check a downloaded artifact against the checksum recorded (and
/// signed) in the [`ReleaseManifest`].
pub fn verify_artifact_checksum(data: &[u8], expected_sha256_hex: &str) -> Result<()> {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let actual = hasher.finalize();
    let actual_hex = hex_encode(&actual);

    let expected = expected_sha256_hex.trim().to_ascii_lowercase();
    if actual_hex != expected {
        bail!(
            "checksum mismatch: expected {}, got {} — artifact may be truncated or tampered",
            expected,
            actual_hex
        );
    }
    Ok(())
}

/// Look up and verify the entry for `platform` in an already
/// signature-verified manifest, returning the expected filename + checksum.
pub fn find_artifact<'a>(
    manifest: &'a ReleaseManifest,
    platform: &str,
) -> Result<&'a ArtifactEntry> {
    manifest
        .artifacts
        .iter()
        .find(|a| a.platform == platform)
        .with_context(|| {
            format!(
                "no artifact for platform '{platform}' in manifest v{}",
                manifest.version
            )
        })
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{:02x}", b);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Generates a fresh Ed25519 keypair for a single test using `ring`'s
    /// RNG, returning (pkcs8_bytes, public_key_bytes). Each test gets its
    /// own key so tests can't accidentally rely on a shared fixture key.
    fn generate_test_keypair() -> (ring::signature::Ed25519KeyPair, Vec<u8>) {
        use ring::signature::KeyPair as _;
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 =
            ring::signature::Ed25519KeyPair::generate_pkcs8(&rng).expect("keygen should not fail");
        let keypair = ring::signature::Ed25519KeyPair::from_pkcs8(pkcs8.as_ref())
            .expect("parsing freshly generated pkcs8 should not fail");
        let public_key = keypair.public_key().as_ref().to_vec();
        (keypair, public_key)
    }

    fn sign(keypair: &ring::signature::Ed25519KeyPair, bytes: &[u8]) -> Vec<u8> {
        keypair.sign(bytes).as_ref().to_vec()
    }

    fn b64(bytes: &[u8]) -> String {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    fn sample_manifest_bytes() -> Vec<u8> {
        let manifest = ReleaseManifest {
            version: "0.3.0-beta.1".to_string(),
            channel: "beta".to_string(),
            artifacts: vec![ArtifactEntry {
                platform: "linux-x86_64".to_string(),
                filename: "simplicio-code-0.3.0-beta.1-linux-x86_64".to_string(),
                sha256: "e".repeat(64),
            }],
        };
        serde_json::to_vec(&manifest).unwrap()
    }

    #[test]
    fn valid_manifest_and_signature_verify_and_parse() {
        let (keypair, public_key) = generate_test_keypair();
        let manifest_bytes = sample_manifest_bytes();
        let signature = sign(&keypair, &manifest_bytes);

        let parsed =
            verify_manifest_signature(&manifest_bytes, &b64(&signature), &b64(&public_key))
                .expect("valid signature over unmodified manifest must verify");

        assert_eq!(parsed.version, "0.3.0-beta.1");
        assert_eq!(parsed.artifacts.len(), 1);
    }

    #[test]
    fn tampered_manifest_body_is_rejected() {
        let (keypair, public_key) = generate_test_keypair();
        let manifest_bytes = sample_manifest_bytes();
        let signature = sign(&keypair, &manifest_bytes);

        // Flip one byte in the signed payload (simulates an attacker
        // substituting a different checksum/filename post-signing).
        let mut tampered = manifest_bytes.clone();
        let flip_index = tampered.len() / 2;
        tampered[flip_index] ^= 0xFF;

        let result = verify_manifest_signature(&tampered, &b64(&signature), &b64(&public_key));
        assert!(
            result.is_err(),
            "tampered manifest body must fail verification"
        );
    }

    #[test]
    fn tampered_signature_is_rejected() {
        let (keypair, public_key) = generate_test_keypair();
        let manifest_bytes = sample_manifest_bytes();
        let mut signature = sign(&keypair, &manifest_bytes);
        signature[0] ^= 0xFF;

        let result =
            verify_manifest_signature(&manifest_bytes, &b64(&signature), &b64(&public_key));
        assert!(
            result.is_err(),
            "corrupted signature must fail verification"
        );
    }

    #[test]
    fn signature_from_wrong_key_is_rejected() {
        let (keypair_a, _public_key_a) = generate_test_keypair();
        let (_keypair_b, public_key_b) = generate_test_keypair();
        let manifest_bytes = sample_manifest_bytes();
        let signature = sign(&keypair_a, &manifest_bytes);

        // Signed with key A, verified against key B's public key.
        let result =
            verify_manifest_signature(&manifest_bytes, &b64(&signature), &b64(&public_key_b));
        assert!(
            result.is_err(),
            "signature made with a different key must fail verification"
        );
    }

    #[test]
    fn malformed_base64_is_rejected() {
        let (_keypair, public_key) = generate_test_keypair();
        let manifest_bytes = sample_manifest_bytes();

        let result = verify_manifest_signature(&manifest_bytes, "not-base64!!!", &b64(&public_key));
        assert!(result.is_err());
    }

    #[test]
    fn checksum_match_is_accepted() {
        let data = b"hello world, this is a release artifact";
        let mut hasher = Sha256::new();
        hasher.update(data);
        let digest = hex_encode(&hasher.finalize());

        verify_artifact_checksum(data, &digest).expect("matching checksum must be accepted");
        // Case-insensitivity.
        verify_artifact_checksum(data, &digest.to_uppercase())
            .expect("uppercase hex checksum must also be accepted");
    }

    #[test]
    fn checksum_mismatch_is_rejected() {
        let data = b"hello world, this is a release artifact";
        let wrong_digest = "0".repeat(64);
        let result = verify_artifact_checksum(data, &wrong_digest);
        assert!(result.is_err(), "wrong checksum must be rejected");
    }

    #[test]
    fn truncated_artifact_is_rejected() {
        let data = b"hello world, this is a release artifact";
        let mut hasher = Sha256::new();
        hasher.update(data);
        let digest = hex_encode(&hasher.finalize());

        // Simulate a truncated download: same expected digest, less data.
        let truncated = &data[..data.len() - 5];
        let result = verify_artifact_checksum(truncated, &digest);
        assert!(result.is_err(), "truncated artifact must fail its checksum");
    }

    #[test]
    fn find_artifact_looks_up_by_platform() {
        let manifest_bytes = sample_manifest_bytes();
        let manifest: ReleaseManifest = serde_json::from_slice(&manifest_bytes).unwrap();

        let found = find_artifact(&manifest, "linux-x86_64").expect("platform present");
        assert_eq!(found.filename, "simplicio-code-0.3.0-beta.1-linux-x86_64");

        let missing = find_artifact(&manifest, "windows-x86_64");
        assert!(
            missing.is_err(),
            "unlisted platform must error, not silently pass"
        );
    }
}
