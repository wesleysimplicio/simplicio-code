# Simplicio Code operational examples

## Preflight before a Loop plan

```bash
python3 scripts/preflight.py --root . --json
```

Continue only when the report contains `"status": "ready"`.

## Explicit tool selection

```bash
python3 scripts/preflight.py \
  --dev-cli /path/to/simplicio-dev-cli \
  --runtime /path/to/simplicio \
  --json
```

Explicit paths make PATH ambiguity visible and reproducible in a receipt.
