#!/usr/bin/env bash
# Sign a release manifest with Ed25519 (raw signature, base64-encoded),
# matching what crates/codegen/xai-grok-update/src/manifest_verify.rs
# verifies.
#
# *** DEV / PLACEHOLDER SIGNING ONLY ***
# This script is a mechanism demo, not a production signing pipeline. It
# signs with whatever private key it's given — in CI that's currently a
# throwaway key generated fresh per workflow run (see
# .github/workflows/release.yml), NOT a real, org-custodied production
# signing key. Anyone who can read the CI logs/artifacts for that run could
# in principle regenerate the same "trust", so this must not be treated as
# a security boundary until real key management (HSM or a properly
# access-controlled CI secret, generated once and rotated deliberately) is
# in place. See the release PR description for what remains open.
#
# Usage:
#   scripts/release/gen_dev_key.sh dev_key   # writes dev_key.pem + dev_key.pub.pem
#   scripts/release/sign_manifest.sh dist/manifest.json dev_key.pem dist/manifest.json.sig
#
# Verification (manual, mirrors manifest_verify::verify_manifest_signature):
#   openssl pkeyutl -verify -pubin -inkey dev_key.pub.pem -rawin \
#       -in dist/manifest.json -sigfile dist/manifest.json.raw.sig

set -euo pipefail

manifest="$1"
priv_key="$2"
sig_out="$3"

if [ ! -f "$manifest" ]; then
    echo "manifest not found: $manifest" >&2
    exit 1
fi
if [ ! -f "$priv_key" ]; then
    echo "private key not found: $priv_key" >&2
    exit 1
fi

raw_sig="${sig_out%.sig}.raw.sig"
openssl pkeyutl -sign -inkey "$priv_key" -rawin -in "$manifest" -out "$raw_sig"
base64 -w0 "$raw_sig" > "$sig_out" 2>/dev/null || base64 "$raw_sig" | tr -d '\n' > "$sig_out"

echo "Wrote base64 signature to $sig_out (raw bytes also at $raw_sig)"
