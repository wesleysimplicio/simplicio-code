# simplicio-code 0.3.0-beta.4 (prerelease)

This beta publishes the current `main` line at commit `2ae6ca6e09d96bfc741066bd1312d3fe78ac2ceb`.

## Included changes

- Added the Fast surface read model and receipt-backed Fast status exposure.
- Restored benchmark event-writer compatibility on the latest main line.
- Kept the Runtime client, Loop Hub transport, and Agent integration surfaces aligned with the ecosystem release chain.

## Release status

This is a prerelease for testing and feedback, not production or unattended use. Existing repository-wide migration and cross-repository acceptance gaps remain follow-up work.

## Release assets

The tag-driven workflow builds configured platform artifacts, checksums, dependency SBOM, and the signed manifest when GitHub Actions capacity is available. The signing key in CI remains a generated placeholder, not a production trust root.

## Installing this beta

```sh
curl -fsSL https://raw.githubusercontent.com/wesleysimplicio/simplicio-code/main/install.sh | bash -s 0.3.0-beta.4
```

```powershell
irm https://raw.githubusercontent.com/wesleysimplicio/simplicio-code/main/install.ps1 | iex -Version 0.3.0-beta.4
```
