# Simplicio Code operational preflight

Before running Loop plans, builds, or contract smoke tests, run:

```bash
python3 scripts/preflight.py --root . --json
```

The report is `READY` only when a `simplicio-dev-cli` candidate returns a
trustworthy version from `--version --json`, the `task` surface is discoverable,
the required repository artifacts exist, and the Runtime contract smoke passes.

An installation that has a task command but no reliable version is reported as
`version_unknown`; it is never represented as version `0.0.0`.

For reproducible selection, pass the exact binaries:

```bash
python3 scripts/preflight.py \
  --dev-cli "$HOME/.local/bin/simplicio-dev-cli" \
  --runtime "$HOME/.local/bin/simplicio" \
  --json
```

The JSON report records every candidate path, the command used, the selected
candidate, artifact gaps, and Runtime smoke output. A non-zero exit code is a
hard stop for automation.
