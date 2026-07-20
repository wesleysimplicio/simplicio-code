#!/usr/bin/env bash
#
# simplicio-code installer — downloads a release published by THIS repo
# (github.com/wesleysimplicio/simplicio-code), never the upstream x.ai
# grok-build infrastructure.
#
# This is intentionally separate from
# crates/codegen/xai-grok-pager/scripts/install.sh, which is inherited
# upstream tooling that still points at x.ai's own CDN/GCS bucket for the
# `grok`/`agent` binaries — see that script's header and the release PR
# description (issue #8) for why it hasn't been repointed in this change.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/wesleysimplicio/simplicio-code/main/install.sh | bash
#   curl -fsSL .../install.sh | bash -s 0.3.0-beta.2   # specific version
#
# Env:
#   SIMPLICIO_BIN_DIR   install dir (default: $HOME/.simplicio-code/bin)
#   SIMPLICIO_REPO      override repo (default: wesleysimplicio/simplicio-code)
#   SIMPLICIO_SKIP_SIG_VERIFY=1   skip Ed25519 manifest-signature verification
#                                  (checksum verification always runs)
#
# Verification performed before any binary is installed:
#   1. SHA256SUMS.txt (published alongside the release) must contain the
#      exact artifact's checksum, and the downloaded bytes must hash to it.
#   2. manifest.json + manifest.json.sig + manifest_signing_public_key.b64
#      (published by .github/workflows/release.yml) are verified with
#      `openssl pkeyutl -verify` when openssl is available. Note: the
#      signing key used by that workflow today is generated fresh per CI
#      run — a placeholder, not a real distributed trust root. See
#      RELEASE_NOTES_0.3.0-beta.2.md.

set -euo pipefail

REPO="${SIMPLICIO_REPO:-wesleysimplicio/simplicio-code}"
TARGET="${1:-}"
BIN_DIR="${SIMPLICIO_BIN_DIR:-$HOME/.simplicio-code/bin}"
API_BASE="https://api.github.com/repos/${REPO}"
DL_BASE="https://github.com/${REPO}/releases/download"

if [[ -n "$TARGET" ]] && [[ ! "$TARGET" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[A-Za-z0-9._]+)?$ ]]; then
    echo "Invalid version format: $TARGET (expected X.Y.Z or X.Y.Z-suffix)" >&2
    exit 1
fi

need() { command -v "$1" >/dev/null 2>&1; }

if ! need curl; then
    echo "curl is required" >&2
    exit 1
fi

case "$(uname -s)" in
    Darwin) os="macos" ;;
    Linux)  os="linux" ;;
    MINGW* | MSYS* | CYGWIN*) os="windows" ;;
    *) echo "Unsupported OS: $(uname -s)" >&2; exit 1 ;;
esac

case "$(uname -m)" in
    x86_64|amd64|AMD64) arch="x86_64" ;;
    arm64|aarch64|ARM64) arch="aarch64" ;;
    *) echo "Unsupported architecture: $(uname -m)" >&2; exit 1 ;;
esac

platform="${os}-${arch}"

if [ -z "$TARGET" ]; then
    echo "Resolving latest release for ${REPO}..." >&2
    version=$(curl -fsSL "${API_BASE}/releases/latest" | \
        sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"v\{0,1\}\([^"]*\)".*/\1/p' | head -1)
    if [ -z "$version" ]; then
        echo "Error: could not resolve the latest release from ${API_BASE}/releases/latest" >&2
        exit 1
    fi
else
    version="$TARGET"
fi

tag="v${version}"
artifact="simplicio-code-${version}-${platform}"
if [ "$os" = "windows" ]; then
    artifact="${artifact}.exe"
fi

workdir=$(mktemp -d)
trap 'rm -rf "$workdir"' EXIT

echo "Downloading simplicio-code ${version} (${platform})..." >&2
curl -fsSL -o "${workdir}/${artifact}" "${DL_BASE}/${tag}/${artifact}" || {
    echo "Error: artifact not found for ${platform} in release ${tag}. simplicio-code may not yet publish this platform." >&2
    exit 1
}
curl -fsSL -o "${workdir}/SHA256SUMS.txt" "${DL_BASE}/${tag}/SHA256SUMS.txt" || {
    echo "Error: SHA256SUMS.txt not found for release ${tag}; refusing to install an unverifiable artifact." >&2
    exit 1
}

echo "Verifying checksum..." >&2
# sha256sum's output is "<hash> <mode-flag><filename>" where mode-flag is
# "*" (binary) or " " (text) — strip a leading "*" before comparing so both
# forms match.
expected=$(awk -v f="$artifact" '{ fn=$2; sub(/^\*/, "", fn); if (fn == f) { print $1; exit } }' "${workdir}/SHA256SUMS.txt")
if [ -z "$expected" ]; then
    echo "Error: ${artifact} has no entry in SHA256SUMS.txt; refusing to install." >&2
    exit 1
fi
if need sha256sum; then
    actual=$(sha256sum "${workdir}/${artifact}" | awk '{print $1}')
elif need shasum; then
    actual=$(shasum -a 256 "${workdir}/${artifact}" | awk '{print $1}')
else
    echo "Error: neither sha256sum nor shasum is available; cannot verify checksum, refusing to install." >&2
    exit 1
fi
if [ "$expected" != "$actual" ]; then
    echo "Error: checksum mismatch for ${artifact} (expected ${expected}, got ${actual}). Download may be truncated or tampered. Aborting." >&2
    exit 1
fi
echo "  Checksum OK against SHA256SUMS.txt (${actual})." >&2

# SHA256SUMS.txt itself is NOT signed, so the check above only catches a
# truncated/corrupted transfer, not a coordinated substitution of both the
# artifact and the checksums file. When a signed manifest is available and
# verifies, its per-platform sha256 (which *is* covered by the Ed25519
# signature) becomes the authoritative check: a mismatch here aborts the
# install even though SHA256SUMS.txt matched, since that combination is a
# strong tamper signal (the two checksum sources disagree).
if [ "${SIMPLICIO_SKIP_SIG_VERIFY:-0}" != "1" ] && need openssl; then
    if curl -fsSL -o "${workdir}/manifest.json" "${DL_BASE}/${tag}/manifest.json" 2>/dev/null \
        && curl -fsSL -o "${workdir}/manifest.json.sig" "${DL_BASE}/${tag}/manifest.json.sig" 2>/dev/null \
        && curl -fsSL -o "${workdir}/pubkey.b64" "${DL_BASE}/${tag}/manifest_signing_public_key.b64" 2>/dev/null; then
        echo "Verifying release manifest signature..." >&2
        base64 -d "${workdir}/manifest.json.sig" > "${workdir}/manifest.json.raw.sig" 2>/dev/null \
            || base64 --decode "${workdir}/manifest.json.sig" > "${workdir}/manifest.json.raw.sig"
        # Rebuild a DER SubjectPublicKeyInfo around the raw 32-byte Ed25519 key
        # so openssl can load it (mirrors the encoding gen_dev_key.sh strips).
        {
            printf '302a300506032b6570032100'
            base64 -d "${workdir}/pubkey.b64" 2>/dev/null | xxd -p | tr -d '\n' \
                || base64 --decode "${workdir}/pubkey.b64" | xxd -p | tr -d '\n'
        } | xxd -r -p > "${workdir}/pubkey.der" 2>/dev/null || true
        if [ -s "${workdir}/pubkey.der" ] && openssl pkey -pubin -inform DER -in "${workdir}/pubkey.der" -out "${workdir}/pubkey.pem" 2>/dev/null \
            && openssl pkeyutl -verify -pubin -inkey "${workdir}/pubkey.pem" -rawin \
                -in "${workdir}/manifest.json" -sigfile "${workdir}/manifest.json.raw.sig" >/dev/null 2>&1; then
            echo "  Manifest signature OK." >&2
            # Extract the sha256 for this platform from the (now-trusted)
            # manifest JSON. Small, schema-specific parser — good enough for
            # the flat, known shape produced by generate_manifest.py; not a
            # general JSON parser.
            manifest_sha256=$(tr -d '\n' < "${workdir}/manifest.json" \
                | sed -n 's/.*"platform"[[:space:]]*:[[:space:]]*"'"${platform}"'"[^}]*"sha256"[[:space:]]*:[[:space:]]*"\([a-fA-F0-9]*\)".*/\1/p' \
                | head -1)
            if [ -n "$manifest_sha256" ]; then
                if [ "$(printf '%s' "$manifest_sha256" | tr 'A-F' 'a-f')" != "$actual" ]; then
                    echo "Error: signed manifest's checksum for ${platform} (${manifest_sha256}) does not match the downloaded artifact (${actual})." >&2
                    echo "  SHA256SUMS.txt and the signed manifest disagree — refusing to install." >&2
                    exit 1
                fi
                echo "  Signed manifest checksum also matches (${manifest_sha256})." >&2
            else
                echo "Note: signed manifest has no entry for platform '${platform}'; proceeding on SHA256SUMS.txt verification alone." >&2
            fi
        else
            echo "Warning: could not verify the release manifest signature (missing xxd, or signature check failed)." >&2
            echo "  Proceeding on checksum verification alone. Set SIMPLICIO_SKIP_SIG_VERIFY=1 to silence this." >&2
        fi
    else
        echo "Note: release ${tag} did not publish a signed manifest; proceeding on checksum verification alone." >&2
    fi
fi

mkdir -p "$BIN_DIR"
dest="${BIN_DIR}/simplicio-code"
if [ "$os" = "windows" ]; then
    dest="${dest}.exe"
fi
chmod +x "${workdir}/${artifact}" 2>/dev/null || true
mv -f "${workdir}/${artifact}" "$dest"

echo "" >&2
echo "simplicio-code ${version} installed to ${dest}" >&2
case ":$PATH:" in
    *":$BIN_DIR:"*) echo "Run 'simplicio-code --version' to get started." >&2 ;;
    *) echo "Add ${BIN_DIR} to your PATH, then run 'simplicio-code --version':" >&2
       echo "  export PATH=\"${BIN_DIR}:\$PATH\"" >&2 ;;
esac
