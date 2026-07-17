# Windows native build status (issue #8 follow-up)

This documents an actual local verification run on a native Windows 11 machine,
done after PR #40 (`fix(xai-proto-build): use portable temp files instead of
/dev/stdout and /dev/null`) landed on `main`. It is not a claim from CI logs —
CI is not currently able to run at all (see "CI status" below) — everything
here was run and observed locally.

## What PR #40 fixed

`crates/build/xai-proto-build/src/lib.rs` used to hardcode
`--dependency_out=/dev/stdout` and `--descriptor_set_out=/dev/null` when
shelling out to `protoc`. Those are POSIX-only pseudo-paths; `protoc.exe`
rejects them on Windows, so every crate that transitively depends on
`xai-proto-build` (`xai-grok-tools-api` and, downstream of it, most of the
codegen workspace) failed to build natively on Windows even with a working
`protoc.exe` available. PR #40 replaced the special paths with real files in a
`tempfile::TempDir`, fixed the Makefile-dependency-rule parser to split on the
first `": "` separator instead of assuming a `/dev/null:` prefix (also safer
against Windows drive-letter colons), and added a `windows-x86_64` platform
entry to `bin/protoc`'s DotSlash manifest.

PR #40 also documented a residual, separate gap in
`crates/build/xai-proto-build/src/find_protoc.rs`: `bin/protoc` is a DotSlash
shebang script, which Windows cannot execute directly (`os error 193`, "not a
valid Win32 application") without either `dotslash` installed or a DotSlash
Windows shim binary alongside it. Until that shim exists, Windows builds must
set `$PROTOC` to a real `protoc.exe`, or have `protoc` on `$PATH`. This is
tracked separately in issue #35 and is **not** re-fixed here.

## Verification performed (this session)

Machine: native Windows 11, `cargo 1.92.0`, `rustc 1.92.0`
(`1.92.0-x86_64-pc-windows-msvc`), `libprotoc 35.0` available locally (not on
`$PATH` by default — pointed to via `$PROTOC`).

1. **Without `$PROTOC` set, `bin/protoc` present**: confirms the documented
   DotSlash gap still applies exactly as described —
   ```
   bin/protoc found at `..\..\..\bin/protoc` but failed to execute: Failed to
   execute protoc: %1 não é um aplicativo Win32 válido. (os error 193);
   trying protoc from PATH as fallback
   `protoc` not found; likely it is missing in docker image
   thread 'main' panicked at crates\codegen\xai-grok-tools-api\build.rs:33:10:
   called `Result::unwrap()` on an `Err` value: protoc command failed
   ```
   This is the expected, already-known-and-documented residual gap, not a
   regression from PR #40.

2. **With `$PROTOC` pointed at a real `protoc.exe`**:
   ```
   $ cargo check -p xai-grok-tools-api
      Compiling xai-proto-build v0.0.0 (...)
      Compiling xai-grok-tools-api v0.1.220-alpha.4 (...)
       Finished `dev` profile [unoptimized + debuginfo] target(s) in 43.94s
   ```
   Clean build — the crate that was the canonical repro for the Windows
   /dev/stdout bug builds with zero errors. This directly confirms the PR #40
   fix works.

3. **`cargo check --workspace` (bounded runs, ~24 minutes of wall-clock cargo
   time across three timeout-bounded invocations, incremental/resumable)**:
   got through the overwhelming majority of the workspace — including every
   `xai-grok-*` codegen crate, `aws-sdk-s3`, `pdf_oxide`, `resvg`/`usvg`,
   `alacritty_terminal`, `keyring`, `xai-grok-shell`'s dependency tree, etc. —
   with **zero protoc-related failures** and only pre-existing, unrelated
   `unused_imports`/`unused_mut`/`unused_assignments` warnings in a few files
   (`xai-tty-utils`, `xai-grok-tools`, `xai-grok-shared`,
   `xai-grok-pager-render`). This is a materially larger portion of the
   workspace building cleanly on Windows than was possible before PR #40.

4. **One real, unrelated compile error surfaced**:
   ```
   error[E0063]: missing field `search_backend` in initializer of `AgentRebuildSpec`
     --> crates\codegen\xai-grok-shell\src\session\acp_session_impl\spawn.rs:826:44
   error: could not compile `xai-grok-shell` (lib) due to 1 previous error
   ```
   This is **not** a Windows or protoc issue — the struct field and the call
   site are both plain, non-`cfg`-gated Rust, so this would fail identically
   on Linux/macOS. It appears to have been introduced by the `main`-tip merge
   commit `878031ea3` ("feat(#5): wire search through the Runtime MCP client
   (grep_files consumer) (#41)"), which added a `search_backend` field to
   `AgentRebuildSpec` but missed updating this one construction site. This is
   currently breaking `cargo check --workspace` on `main` for every platform.
   It is out of scope for this Windows-CI verification task and is tracked as
   a separate follow-up rather than fixed here.

## CI status (important caveat)

At the time of this check, `wesleysimplicio/simplicio-code`'s GitHub Actions
is **not executing any jobs at all** — every job on every recent `main` run
fails immediately with:
```
The job was not started because your account is locked due to a billing issue.
```
(confirmed via `gh run view` on the latest `main` push run). So none of this
— including the pre-existing `lint-build-test (ubuntu-latest)` /
`(macos-latest)` legs — is actually being exercised by CI right now; the
verification in this doc is local-only. The `.github/workflows/ci.yml`
Windows job comments have been updated to reflect the PR #40 fix (protoc
`/dev/stdout`/`/dev/null` bug fixed) while being explicit that the job is
still deliberately scoped narrower than the full workspace, both because of
the residual DotSlash-shim gap (needs `$PROTOC`/`$PATH` protoc, which the
Windows job doesn't install yet) and because of the unrelated E0063 bug above
that would make a full `--workspace` check red for reasons that have nothing
to do with Windows.

## Bottom line for issue #8

The specific Windows blocker issue #8's acceptance criteria called out
(`/dev/stdout`/`/dev/null` protoc bug preventing Windows builds) is fixed and
locally verified: `xai-grok-tools-api` and the great majority of the
workspace now build cleanly on native Windows once a `protoc.exe` is
reachable via `$PROTOC` or `$PATH`. Full "CI green on Windows" is still
blocked by two independent, smaller items: (a) the DotSlash-shim gap for
resolving `bin/protoc` without a manual `$PROTOC` override (tracked in #35),
and (b) the unrelated `AgentRebuildSpec.search_backend` compile error on
`main` flagged above. Neither of those is a re-emergence of the original
/dev/stdout bug.
