#!/usr/bin/env bash
# Generate a throwaway Ed25519 keypair for manifest signing.
#
# *** NOT FOR PRODUCTION USE. *** See sign_manifest.sh header. This exists
# so CI (or a developer) can produce *a* keypair to demonstrate the signed
# manifest -> verify pipeline end-to-end without checking in a real key.
#
# Usage: scripts/release/gen_dev_key.sh <out-prefix>
#   writes <out-prefix>.pem (private) and <out-prefix>.pub.pem (public),
#   plus <out-prefix>.pub.b64 — the raw 32-byte public key, base64-encoded,
#   which is the format manifest_verify::verify_manifest_signature expects.

set -euo pipefail

prefix="$1"

openssl genpkey -algorithm ed25519 -out "${prefix}.pem"
openssl pkey -in "${prefix}.pem" -pubout -out "${prefix}.pub.pem"

# Extract the raw 32-byte Ed25519 public key from the DER SubjectPublicKeyInfo
# (last 32 bytes of the DER encoding) and base64-encode it, since
# manifest_verify expects raw key bytes, not a PEM/DER wrapper.
openssl pkey -in "${prefix}.pem" -pubout -outform DER 2>/dev/null \
    | tail -c 32 \
    | base64 -w0 > "${prefix}.pub.b64" 2>/dev/null \
    || openssl pkey -in "${prefix}.pem" -pubout -outform DER 2>/dev/null \
    | tail -c 32 \
    | base64 | tr -d '\n' > "${prefix}.pub.b64"

echo "Wrote ${prefix}.pem (private, keep secret), ${prefix}.pub.pem, ${prefix}.pub.b64 (raw pubkey b64)"
