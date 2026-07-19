# Loop contract gate

Before a generated plan can be sent to `run` or `batch`, validate the JSON
boundary:

```bash
python3 scripts/validate_loop_contract.py \
  --mode plan \
  --json \
  /tmp/issue.contract.json
```

The gate fails closed when the plan contains parser errors, an empty identity,
no scenarios, no rules, duplicate/missing stable IDs, unknown rule references,
or unresolved required questions. Its JSON receipt keeps `errors` and
`warnings` visible to callers instead of leaving them only in a log or spill.

The same validator must be run with `--mode run` or `--mode batch` immediately
before dispatch. This protects consumers from contracts produced by older
versions that were written before the validation gate existed.
