# Reproducible Snake benchmark

This benchmark is intentionally evidence-first. It compares the same challenge,
model and repetition count in isolated temporary workspaces. It does not claim
token savings when the provider did not return token usage, and it does not
claim pure Runtime execution without a valid receipt.

## Run

From the Simplicio Code checkout:

    python3 scripts/benchmark_snake.py \
      --model openrouter/<model> \
      --simplicio-cmd 'simplicio-code -p {prompt} --output-format json' \
      --hermes-cmd 'hermes --model {model} --accept-hooks --prompt {prompt}' \
      --repetitions 3 \
      --timeout 900 \
      --output .simplicio/benchmarks/snake

Commands are templates. They may use {workspace}, {model} and {prompt}.
Both agents receive the same prompt and model value.

## Output

- benchmark-result.json: raw run result and status.
- events.hbp: lifecycle events as Runtime-compatible HBP records whose bounded,
  typed TOML payloads are read back and verified before the run is reported.
- cost-ledger.json: measured token usage only.
- benchmark-report.md: compact comparison.

Statuses are:

- PASS: process and structural checks passed.
- FAIL: process, timeout, or product validation failed.
- UNVERIFIED: the process completed but a required evidence gate, such as
  provider usage or the pure Runtime receipt, was unavailable.

The structural validator checks for a package, React/Vite dependencies and
Snake-related source files. It runs npm test/build when those scripts exist.
Browser behavior remains UNVERIFIED until a browser validator writes evidence.

## Pure Runtime gate

For a Simplicio result to be marked as pure Runtime, provide an external
Runtime receipt in the agent workspace or pass --runtime-receipt. The input
adapter accepts the provider's JSON protocol, but the benchmark's persisted
evidence is always HBP. The receipt must contain:

    {
      "server_name": "simplicio",
      "fallback_used": false,
      "operations": ["map", "read", "edit", "exec"]
    }

This gate is deliberately strict because the current Simplicio Code
architecture still documents local write/delete behavior. A benchmark must not
turn that limitation into a marketing claim.

## Limitations

The harness measures process wall time and Linux process RSS/CPU when available.
Token fields are extracted only from provider output. A token estimate is never
reported as billed usage. Configure an external Playwright/browser validator for
scoreboard persistence and gameplay evidence.
