# simplicio-code 0.3.0-beta.6

This prerelease is built from the exact commit identified by the `v0.3.0-beta.6` tag.

## Included

- Runtime process-contract diagnostics for start/status/cancel/wait flows.
- Strict workspace-audit and event-identity validation with deterministic receipts.
- Component release provenance and trusted-key checks for installed surfaces.
- Advisory evidence requirements and invariant receipt counts for Loop Hub runs.
- Installed-surface inventory and benchmark-receipt integrity checks.
- Repository-wide LLM architecture policy documentation.

## Release artifacts

The release workflow is required to produce Linux x86_64, macOS arm64, and Windows x86_64 artifacts, checksums, an SBOM, and a signed manifest. The exact tag, commit, and published assets are the source of truth for this prerelease.

## Compatibility and limitations

- This is a beta prerelease; production trust-root and signing-key custody are not established by this workflow.
- Runtime, Loop, Mapper, Fast, and Dev CLI component provenance must be supplied by their manifests; this release does not claim that a locally resolved Runtime binary is a substitute for a component release receipt.
- Parent integration issues remain open when their full cross-repository acceptance evidence is not present; merged slices must not be interpreted as full issue closure.
