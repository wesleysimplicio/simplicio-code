# Local validation

`scripts/validate_local.py` is the repository-local validation entrypoint for
issue #316. It does not download tools, read credentials, start an LLM, or
turn an unavailable dependency into a pass.

Run the cheapest lane first:

```powershell
python scripts/validate_local.py --profile fast --root . --output-dir dist/local-validation/fast
```

The available profiles are:

- `fast`: preflight, workspace audit, status drift, Python compile, workflow
  contract, deterministic invariants, Rust toolchain, rustfmt and the headless
  invocation matrix;
- `deep`: `fast` plus workspace Cargo check and tests;
- `release`: `deep` plus the release binary build.

Every run writes a bounded `validation-summary.md`, one JSON artifact per gate,
and the canonical `validation-receipt.hbp`. The receipt includes the report,
SHA-256 hashes for every gate artifact, and a digest of the complete report.
It is verified before the command returns. JSON is available only as an
explicit export:

```powershell
python scripts/validate_local.py --profile fast --json-export dist/local-validation/fast.json
```

Recheck a receipt without rerunning gates:

```powershell
python -c "from scripts.validate_local import verify_hbp; verify_hbp('dist/local-validation/validation-receipt.hbp')"
```

Exit code `0` means every selected gate passed. Exit code `1` means a gate
failed, timed out, was unavailable, or the run was cancelled. Gates after the
first failure are recorded as `NOT_EXECUTED` with an explicit `job was not
started` reason; they never become implicit passes.

The command records the commit, dirty state, platform and interpreter. Output
is bounded and redacts configured and secret-shaped values before it is stored.
A release profile must therefore be green; `UNKNOWN`, `FAIL`, `NOT_EXECUTED`
and `CANCELLED` evidence cannot authorize publication.