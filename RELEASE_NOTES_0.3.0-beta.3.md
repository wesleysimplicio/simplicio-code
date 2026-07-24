# simplicio-code 0.3.0-beta.3 (prerelease)

This beta includes the merged issue #100 policy and installed-evidence slice
from `main` at merge commit `b0c4381f7e2c77803f7ff916d9b64f7157238d22`.

## JSON boundary and migration evidence

- Python and Node policy scanners now share the exact JSON-boundary inventory.
- Unknown, migration-pending, and expired findings remain separate and fail
  closed in strict mode.
- Dependency metadata, protocol documentation, and external adapter fixtures
  have explicit path owners instead of implicit exclusions.
- Installed Code evidence covers retry/idempotency, rollback, redacted
  receipts, and deterministic Windows TCP fixture startup.

## Release status

This is a prerelease for testing and feedback, not production/unattended use.
The repository-wide internal JSON migration and full cross-repository HBI/HBP
acceptance remain open follow-up work; this release does not claim issue #100
closed.

## Release assets

The tag-driven release workflow builds the configured platform artifacts,
checksums, dependency SBOM, and signed manifest when GitHub Actions capacity is
available. The signing key in CI remains a generated placeholder, not a
production trust root.

## Installing this beta

```sh
curl -fsSL https://raw.githubusercontent.com/wesleysimplicio/simplicio-code/main/install.sh | bash -s 0.3.0-beta.3
```

```powershell
irm https://raw.githubusercontent.com/wesleysimplicio/simplicio-code/main/install.ps1 | iex -Version 0.3.0-beta.3
```
