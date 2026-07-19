# Disk preflight and disposable cleanup

Protect Loop plans, builds, and tests with a receipt-producing gate:

```bash
python3 scripts/run_with_disk_preflight.py \
  --root . \
  --receipt .simplicio/receipts/disk-preflight.json \
  -- simplicio-loop plan --task issue.md --out contract.json
```

The command does not start when free space is below the configured budget. If
the command runs, the receipt records both the initial and final measurements,
audited cache paths, command, exit code, and whether the budget was breached
during execution.

The read-only preflight never deletes anything. The only disposable cleanup
helper is explicitly scoped to `target/`:

```bash
python3 scripts/cleanup_disposable.py --workspace . --json
SIMPLICIO_ALLOW_DISPOSABLE_CLEANUP=1 \
  python3 scripts/cleanup_disposable.py --workspace . --delete --json
```

The second command requires deliberate operator confirmation and refuses
symlinks or paths that resolve outside the workspace. It does not touch source,
branches, worktrees, Cargo registry data, or `.simplicio` state.
