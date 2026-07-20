# simplicio-code 0.3.0-beta.2 (prerelease)

This beta updates the Simplicio Runtime integration to runtime `3.5.3` and
ships the latest agent-panel, TUI attention-panel, and AgentHost enforcement
changes from this repository.

This is a prerelease for testing and feedback, not production/unattended use.

## Runtime integration

- Runtime handshake and map-cache fixtures now target Simplicio Runtime
  `3.5.3`.
- The release remains fail-closed when a genuine Simplicio Runtime is not
  available for project-file reads.

## Release assets

The release workflow builds Linux x86_64 and macOS aarch64 artifacts on tag
push, generates checksums and a dependency SBOM, and publishes the GitHub
prerelease with its signed manifest.

Known beta limitations from `0.3.0-beta.1` remain in effect, including
best-effort Windows builds and placeholder CI signing-key continuity.

## Installing this beta

```sh
curl -fsSL https://raw.githubusercontent.com/wesleysimplicio/simplicio-code/main/install.sh | bash -s 0.3.0-beta.2
```

```powershell
irm https://raw.githubusercontent.com/wesleysimplicio/simplicio-code/main/install.ps1 | iex -Version 0.3.0-beta.2
```
