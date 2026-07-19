<!--
This repository does not accept external pull requests (see CONTRIBUTING.md).
This template is for internal Simplicio work only.
-->

## Summary

<!-- What changed and why. Link the issue(s) this closes/relates to. -->

## Scope check

- [ ] This PR touches only Simplicio-owned surface (`simplicio-runtime-client`,
      `xai-grok-models`, headless permission/approval logic) and/or is a
      narrowly-scoped fix elsewhere — see [`DOD.md`](../DOD.md) for what
      "Simplicio-owned" means in this fork.

## Struct-initializer invariant (code#64 pattern)

- [ ] If this PR constructs a variable and passes it to a struct literal,
      **every** relevant field of that struct actually receives the
      constructed variable — none were built and then silently left out of
      the initializer (the exact shape of
      [code#64](https://github.com/wesleysimplicio/simplicio-code/issues/64):
      `search_backend` was constructed, then never referenced in
      `AgentRebuildSpec { .. }`, and only a non-`Option` field type turned
      that into a compile error instead of a silent functional regression).

## Local gates run

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` (or scoped to
      the touched crate(s) when the full workspace build is impractical —
      state which)
- [ ] `cargo test` (workspace, or scoped to the touched crate(s) — state
      which, and why if scoped)
- [ ] New/changed behavior has a regression test that fails without the fix
- [ ] Parsing/transformation logic touched has property-test coverage
      (`proptest`) where applicable, not just hand-picked examples

## Evidence

<!-- Paste the relevant command output (build/test/clippy), or link the CI run. -->
