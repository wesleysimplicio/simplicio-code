# simplicio-code 0.3.0-beta.5 (prerelease)

## Included changes

- Fast doctor surface for Full and Loop standalone (#294 / PR #308)
- Read-only Agent Fabric cockpit (#296 / PR #309)
- LiteRT provider selection via Runtime only (#302 / PR #310)
- Code-owned binary-format migration proof and release gate (#100)
- Onboarding bundle pins: mapper **0.26.0**, loop **3.38.6**, dev-cli **0.18.0**
- PyPI companion package `simplicio-code` (doctor/quality probes)

## Install

Binary (GitHub):

```sh
curl -fsSL https://raw.githubusercontent.com/wesleysimplicio/simplicio-code/main/install.sh | bash -s 0.3.0-beta.5
```

PyPI tooling:

```sh
pip install simplicio-code==0.3.0b5
```

## Status

Prerelease for testing. Productive filesystem still fail-closed through Runtime.
