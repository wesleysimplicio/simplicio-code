# Definition of Done — Simplicio-owned surface

This file is Simplicio's local Definition of Done for `simplicio-code`. It is
**scoped**, not a blanket policy for the repository: `simplicio-code` is a
private fork (Apache-2.0, upstream copyright preserved) of a third-party
product — SpaceXAI's own code under `crates/codegen/xai-grok-*` (66+ crates)
is inherited technical debt, not something the Simplicio team took on the
job of deeply testing. Applying a heavyweight DoD to the entire fork would
either be ignored or would slow down every unrelated change to code the team
doesn't own the design of. See
[code#67](https://github.com/wesleysimplicio/simplicio-code/issues/67) and
the cross-repo hub issue
[simplicio-loop#579](https://github.com/wesleysimplicio/simplicio-loop/issues/579)
for the full reasoning.

## Scope

This DoD applies to the points genuinely added or modified by Simplicio:

- `crates/codegen/simplicio-runtime-client` — the fail-closed MCP client
  Simplicio Code uses for every project file operation (`read`, `write`,
  `delete`, `list`, `stat`, `search`, `exec`, `edit`) against the embedded
  Simplicio Runtime. No local-filesystem fallback exists in this crate by
  design; the handshake (`initialize` → `tools/list`) is the boundary that
  must fail closed.
- `crates/codegen/simplicio-agent-client` — Code's fail-closed client for the
  independently shipped AgentHost. Its version/capability handshake, private
  socket check, bounded advisory replay, and fixed no-free-form attention
  vocabulary are part of the owned boundary.
- `crates/codegen/xai-grok-tools/src/computer/local/simplicio_runtime.rs` —
  the narrow inherited adapter modified by Simplicio to require Agent first
  and Runtime second for every operation it owns. Neither dependency has a
  built-in/local fallback.
- `crates/codegen/xai-grok-models` — the crate that loads Simplicio's model
  configuration (`Simplicio-1` / `tencent/hy3:free` via OpenRouter, the
  default model surfaced by this fork).
- Any headless permission/approval logic (`--always-approve`,
  `--permission-mode`, and the invocation-mode dispatch around them) —
  Simplicio-specific automation/CI surface, not upstream behavior.

Everything else in the tree (the `xai-grok-*`/`xai-acp-*` crates not listed
above) is out of scope for this DoD. Fuzzing or deep property-testing that
inherited surface is explicitly not something this DoD asks for — see
code#67 §4.

## Motivation: 3 real bugs found in this scope this session

- **[code#63](https://github.com/wesleysimplicio/simplicio-code/issues/63)**
  (closed, PR #65) — `cargo build --release` bundled `ripgrep` from GitHub
  Releases at build time with no offline/proxy fallback, breaking any build
  behind restricted egress. A pure availability/environment bug, but it
  meant nobody could build the product from source in a hermetic or
  air-gapped CI runner without discovering two separate undocumented
  workaround env vars one at a time.
- **[code#64](https://github.com/wesleysimplicio/simplicio-code/issues/64)**
  (closed, PR #65) — `AgentRebuildSpec { .. }` at
  `xai-grok-shell/src/session/acp_session_impl/spawn.rs:826` was missing the
  `search_backend` field, even though a `search_backend` variable was
  already constructed earlier in the very same function. It only blocked
  the build because the field was a non-`Option`-with-default,
  compiler-enforced-required field; had it been optional with an implicit
  default, this would have compiled clean and silently dropped the search
  backend on every session rebuild — a functional regression with **zero**
  compiler or test signal. This is exactly the "constructed, never
  referenced in the struct literal" class of bug the invariant-review
  question below targets.
- **[code#66](https://github.com/wesleysimplicio/simplicio-code/issues/66)**
  (open, in progress) — `--always-approve` hangs indefinitely in headless
  invocation (no spinner, no output, not even a debug log file gets created)
  while the equivalent `--permission-mode bypassPermissions` works fine in
  the same environment. Two flags documented as serving the same purpose
  diverge silently, and the failure mode (infinite hang, not a fast error)
  is the worst possible shape for an automation/CI caller.

None of these three bugs is caught by "does it compile" or "does the
existing test suite pass" alone — #63 is an environment/network assumption
never exercised in CI, #64 is a cross-function invariant no single-function
unit test would catch, and #66 is a divergence between two code paths that
look equivalent from the CLI surface but are not. The four gates below are
aimed specifically at that gap.

## The gates

### 1. Build & lint (existing CI gates, scoped)

- `cargo check -p simplicio-runtime-client` / `cargo check -p
  simplicio-agent-client` / `cargo check -p xai-grok-models` — must succeed
  without a network dependency beyond what
  `Cargo.lock` already resolves (regression guard for the class of bug in
  code#63: any new build-time network fetch must have a documented,
  discoverable offline/`PATH`-fallback override, same shape as
  `GROK_TOOLS_BUNDLE_RG_PATH`).
- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --
  -D warnings` for touched crates (`.githooks/pre-commit`,
  `.github/workflows/ci.yml`) — both must be green for any file this DoD's
  scope covers before merge.

### 2. Unit + property tests

- Ordinary unit tests as already exist in
  `crates/codegen/simplicio-runtime-client/src/lib.rs` (path-escape
  rejection, schema validation, capability negotiation, etc.).
- **Property test of the handshake fail-closed guarantee** (code#67 item 1):
  `crates/codegen/simplicio-runtime-client/tests/fake_server_handshake_proptest.rs`
  generates malformed/partial MCP `initialize` responses (non-JSON noise, a
  JSON-RPC envelope missing `result`, a JSON-RPC `error` reply, a `result`
  missing `serverInfo` entirely, an identity-mismatched `serverInfo.name`,
  and a byte-truncated prefix of an otherwise well-formed response) via
  `proptest`, and asserts `RuntimeClient::spawn_in` always returns `Err` —
  never `Ok`, and never any local-disk read as a fallback (this client has
  none). Any new capability or response field this client parses should get
  a scenario added to this property test, not just a hand-picked example.

### 3. Integration / system

- The existing fake-runtime-server integration tests
  (`tests/fake_server_search.rs`, `tests/fake_server_malformed_handshake.rs`)
  exercise the full `initialize` → `tools/list` → `tools/call` round trip
  against a real subprocess, not just in-process function calls.
- **Planned** (tracked in the follow-up issue this session opens): a CI
  matrix over Simplicio Code's invocation modes — interactive vs. headless
  (`-p`/positional), with/without a real TTY, and every `--permission-mode`
  value crossed against `--always-approve` — the exact combination that
  would have caught code#66 before it shipped.

### 4. Regression + invariant review

- Every real bug fixed in this scope gets a regression test that fails
  without the fix (already the pattern for
  `fake_server_malformed_handshake.rs`, added for code#38/#45).
- **Struct-initializer invariant question**, added to the PR checklist
  (`.github/PULL_REQUEST_TEMPLATE.md`) for the class of bug code#64 is:
  *"If this PR constructs a variable and passes it to a struct literal, does
  every relevant struct field actually receive that variable, or was one
  forgotten?"* This is a reviewer prompt, not a mechanical check yet — the
  follow-up issue proposes a `grep`-based CI script to make it mechanical
  (code#67 item 2).

## Not in scope here (see the follow-up issue)

- Fuzzing/property-testing the inherited `xai-grok-*` tree.
- The grep-based custom lint for "constructed-but-unused-in-struct-literal".
- The invocation-mode CI matrix described in gate 3 above.

Both are laid out as a concrete step-by-step plan in the follow-up issue
this session opens against this repo, referencing this file,
[code#67](https://github.com/wesleysimplicio/simplicio-code/issues/67), and
[simplicio-loop#579](https://github.com/wesleysimplicio/simplicio-loop/issues/579).
