# Local recovery evidence

Issue #315 Phase 0 provides a deterministic local evidence bundle without
requiring paid GitHub Actions.

Run from the repository root:

    python scripts/recovery_evidence.py --root . --profile fast \
      --output-dir dist/recovery-evidence --timeout 60

The command records:

- the exact Git commit and checkout cleanliness before and after the run;
- the resolved paths and semver versions for Runtime, Mapper, Fast, Dev CLI, and Loop;
- the selected local validation profile and its verified HBP receipt, when enabled;
- a canonical JSON bundle with an evidence_sha256 digest and a short Markdown summary.

The default profile invokes scripts/validate_local.py, which owns the validation
gates and HBP receipt format for issue #316. Use --skip-validation only when
diagnosing the toolchain or testing the coordinator; that mode is explicitly
UNVERIFIED and cannot return PASS.

Status is fail-closed:

- PASS requires every tool version, the validation run, the receipt digest, and
  both Git cleanliness checks to pass;
- FAIL means a required command, validation gate, receipt, or cleanliness check
  failed or timed out;
- UNVERIFIED means validation was skipped or a required fact could not be
  established.

The bundle does not claim GitHub Actions, remote CI, release, installation,
downgrade, downstream integration, or security proof. It makes no network calls
and does not persist command-shaped secrets. Generated output belongs under
dist/ or .simplicio/ and should not be committed unless a task explicitly
requires a fixture.
