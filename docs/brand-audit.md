# Brand audit — Simplicio Code (issue #7)

This document is the paper trail for the "remove leftover prior-brand
references" epic. Simplicio Code is a rebrand of an internal xAI "Grok" CLI
fork, which itself vendors tool implementations from `sst/opencode`. There
are therefore three brand layers in this tree:

1. **opencode** — the outer upstream project some tool implementations were
   ported from (Apache-2.0 attributed).
2. **xAI / Grok** — the immediate fork this product is built on top of.
   ~40 crates are still named `xai-grok-*`, and a large amount of CLI help
   text, config paths (`~/.grok/`), and OAuth wiring (`auth.x.ai`) still
   refers to it.
3. **Simplicio** — the current brand. `README.md`, the model identity
   (`Simplicio-1`), the default theme name ("Simplicio Brasil"), and most
   top-level docs already use it correctly.

Full rebranding of layer 2 (renaming ~40 crates, all CLI flags, the OAuth
host, the `~/.grok/` config directory, and hundreds of string literals) is a
multi-week, cross-cutting mechanical effort and is **out of scope for this
session**. What follows is what was actually fixed, what is deliberately
left alone and why, and the automated tooling added so the backlog doesn't
grow silently.

## Fixed in this change

| Surface | File:line | Before | After |
|---|---|---|---|
| Model self-identification sent to the LLM in every session | `crates/codegen/xai-grok-agent/src/prompt/context.rs:153` (`DEFAULT_SYSTEM_PROMPT_LABEL`) | `"Grok"` | `"Simplicio Code"` |
| Primary system prompt template | `crates/codegen/xai-grok-agent/templates/prompt.md:1` | `"You are ${{ system_prompt_label }} released by xAI."` | `"You are ${{ system_prompt_label }}."` (drops the false xAI attribution entirely; re-encrypted via `python3 scripts/encrypt_templates.py`, see `prompt_encrypted.rs`) |
| Desktop/OS "session ready" notification title | `crates/codegen/xai-grok-pager/src/app/dispatch/status.rs:361` | `title: "Grok".into()` | `title: "Simplicio Code".into()` |

These three were chosen because they are the highest-visibility leaks: one
is what the *model itself* says when asked "what are you", one is what a
user sees pop up on their desktop after every turn, and one is the literal
system prompt shipped with every request.

## Explicitly out of scope (documented, not silently ignored)

- **`~/.grok/` config/cache directory, `auth.x.ai` OAuth host, `grok.com`
  relay, `GROK_*` env vars, and the bulk of `crates/codegen/xai-grok-pager/src/app/cli.rs`
  help text.** These are functional integration points (real auth flow,
  real config file location), not just strings — renaming them requires a
  coordinated migration (config-path aliasing, OAuth re-registration) so
  existing installs don't break. Renaming the *text* without renaming the
  *behavior* would be actively misleading. Tracked here, not fixed this
  session.
- **`xai-grok-*` crate and package names** (~40 crates). Internal Cargo
  package identifiers; renaming is a pure mechanical rename with no runtime
  behavior change, but it touches every `Cargo.toml`, every `use` path, and
  CI. Left as-is; see `scripts/brand_audit.py`'s `STRICT_ALLOWLIST` for the
  README lines that reference these paths by their real (unrenamed) names.
- **`ThemeKind::GrokNight` / `ThemeKind::GrokDay`** internal enum
  variants/module names. `from_name()` already accepts `"simplicio"` /
  `"simplicio-brasil"` / `"brasil"` as user-facing aliases
  (`crates/codegen/xai-grok-pager-render/src/theme/mod.rs:104-109`), and
  `display_name_for_canonical()` renders `"groknight"` as **"Simplicio
  Brasil"** to the user (`mod.rs:145-152`). The Rust identifier itself is
  internal and not user-visible.
- **"SpaceXAI"** (`LICENSE`, `CONTRIBUTING.md`, several
  `xai-grok-pager` docs/settings screens). This is a deliberate,
  repo-wide, already-consistent choice (not a stray leftover — verified via
  full-repo grep, it's the *only* spelling used, never a mix of "xAI" and
  "SpaceXAI" in the same context). Whether "SpaceXAI" or a Simplicio-owned
  legal entity should be the copyright holder is a business/legal decision,
  not something this session can decide on the codebase's behalf. Flagged
  for a human owner; not changed.
- **`opencode` as a selectable built-in agent name** in
  `crates/codegen/xai-grok-agent/src/config.rs` (`BuiltinAgentName::Opencode`,
  string literal `"opencode"`). This is a legitimate feature name (an
  opencode-compatible tool/parameter convention preset a user can select),
  not self-branding — analogous to how `docs/ARCHITECTURE.md:7` legitimately
  names "Grok, OpenRouter, OpenCode Go/Zen" as *external providers* the
  Simplicio gateway abstracts over. Not a leak.
- **Light theme (`grokday.rs`)** still uses the original TokyoNight-derived
  blue/magenta accent palette, not Brazil green/yellow. The issue's
  acceptance criteria describe a green/yellow **default** theme, which this
  session verified (see below); a matching light Simplicio Brasil palette
  is a design decision (new colors, not just a rename) and is left as a
  follow-up.

## Accessibility fix: WCAG AA contrast

While auditing the default ("Simplicio Brasil" / `groknight`) theme's
palette in `crates/codegen/xai-grok-pager-render/src/theme/groknight.rs`,
computed contrast ratios (not eyeballed — see `scripts/contrast_audit.py`)
found that `accent_system`, `accent_skill`, `fuzzy_accent`, and
`accent_verify` all used `BLUE` (`#002776`, the literal Brazil flag navy),
which measures **~1.4:1** against both theme backgrounds (`#0a0a0a`,
`#141414`) — far under the WCAG AA minimum of 4.5:1 for text (or 3:1 for UI
components). This was a real accessibility bug, not just a branding
nit — any UI text rendered in that color would have been effectively
invisible on the dark background.

Fix: those four fields now use `BLUE1` (`#3A95AB`), which measures
**5.3–5.7:1** against both backgrounds — comfortably AA. `BLUE` itself is
kept in the palette (documented as decorative-only, not used for text) in
case a future non-text UI element wants the literal flag color.

Every other semantic color already passed AA (worst case:
`accent_success`/`GREEN` at 4.81:1 on `bg_base` — still above the 4.5:1
floor). Full numbers:

```
$ python3 scripts/contrast_audit.py
[contrast-audit] backgrounds: bg_terminal=#0a0a0a bg_base=#141414

  [OK] text_primary (FG): bg_terminal=15.14:1 bg_base=14.09:1 (need >= 4.5:1)
  [OK] text_secondary (FG_DARK): bg_terminal=11.83:1 bg_base=11.01:1 (need >= 4.5:1)
  [OK] accent_success (GREEN): bg_terminal=5.17:1 bg_base=4.81:1 (need >= 4.5:1)
  [OK] accent_error (RED): bg_terminal=7.48:1 bg_base=6.96:1 (need >= 4.5:1)
  [OK] warning/command (YELLOW): bg_terminal=14.90:1 bg_base=13.87:1 (need >= 4.5:1)
  [OK] accent_assistant/accent_running (GREEN1): bg_terminal=8.85:1 bg_base=8.23:1 (need >= 4.5:1)
  [OK] accent_system/accent_skill/fuzzy_accent/accent_verify (BLUE1): bg_terminal=5.72:1 bg_base=5.32:1 (need >= 4.5:1)

  [INFO] BLUE (#002776, decorative-only, not used as text): bg_terminal=1.46:1 — confirmed unfit for text (as expected, not used for text)

[contrast-audit] PASS
```

`scripts/contrast_audit.py` transcribes the palette's hex values by hand
(documented at the top of the file) because the Rust workspace could not be
compiled in this sandbox (see "Known blocker" below) — a real Rust unit
test doing this same check directly against `Theme::groknight()` is the
better long-term home for this and is called out as follow-up work.

## Automated tooling added

- **`scripts/brand_audit.py`** — two-tier scanner:
  - *Strict surfaces* (`README.md`, `CONTRIBUTING.md`, `SECURITY.md`,
    `docs/**/*.md`, prompt templates): zero tolerance for "opencode" /
    "xai" / "grok" tokens outside a short, rationale-carrying allowlist
    (`STRICT_ALLOWLIST` in the script). Currently **passes with zero
    unexplained hits**.
  - *Regression guard* over `crates/**/*.rs` string literals containing
    "Grok" or "xAI": counts current occurrences (2035 at the time of this
    change) and fails if that number ever goes *up*. This doesn't force
    fixing the backlog, but it means new code can't add to it without the
    build noticing. Run: `python3 scripts/brand_audit.py`.
- **`scripts/contrast_audit.py`** — WCAG AA contrast check for the default
  theme's semantic colors (see above). Run: `python3 scripts/contrast_audit.py`.

## Known blocker: workspace does not build in this sandbox

`crates/codegen/xai-grok-tools-api/build.rs` invokes `protoc` via a
`bin/protoc` [dotslash](https://dotslash-cli.com/) pointer file. This
sandbox has neither a real `protoc` binary nor the `dotslash` launcher
needed to fetch one (no matching binary found anywhere on `PATH` or disk;
`dotslash` itself is not installed), so **any crate that transitively
depends on `xai-grok-tools-api`** — which includes `xai-grok-agent`,
`xai-grok-pager`, and `xai-grok-pager-render`, i.e. every crate touched for
the three brand-string fixes and the theme fix above — fails to compile
here with:

```
bin/protoc found at `..\..\..\bin/protoc` but failed to execute: ...
`protoc` not found; likely it is missing in docker image
thread 'main' panicked at crates\codegen\xai-grok-tools-api\build.rs:33:10:
called `Result::unwrap()` on an `Err` value: protoc command failed
```

This is **pre-existing and unrelated** to the changes in this PR (verified
by inspecting `bin/protoc`'s contents, which is a dotslash manifest, not an
executable). It means the three brand-string edits and the theme palette
fix could **not** be compiled or test-run in this environment. They were
kept intentionally minimal and low-risk (single string-literal swaps and a
const-to-const color reference, matching existing patterns already used
elsewhere in the same files) and reviewed line-by-line against every test
in the repo that references the touched constants/strings (see the PR
description for the specific greps run), but this is not a substitute for
`cargo build`/`cargo test` actually passing. **A reviewer with a working
`protoc` must run `cargo test -p xai-grok-agent -p xai-grok-pager -p
xai-grok-pager-render` before merging.**

The one crate genuinely unaffected by this blocker —
`simplicio-runtime-client` (no dependency on `xai-grok-tools-api`) — was
built and tested successfully; see the PR description for output.

## Acceptance criteria status (issue #7)

- [x] Automated scan finds no *unexplained* old-brand reference on strict
      public surfaces (`scripts/brand_audit.py`, tier 1).
- [ ] "busca automatizada não encontra o nome antigo em superfícies
      públicas" **in full** — true only for the strict-surface tier; the
      CLI help text / config paths / crate names backlog remains (see
      "Explicitly out of scope" above). A regression guard now prevents it
      from growing.
- [ ] Binary/all modes show "Simplicio Code" everywhere — the three fixes
      above cover the model identity and one notification; CLI `--help`
      output in `xai-grok-pager/src/app/cli.rs` still says "Grok" in many
      places (not fixed this session).
- [x] Default theme green/yellow palette does not break contrast — verified
      quantitatively, one real AA violation found and fixed (`accent_system`
      et al., see above).
- [ ] WCAG AA audited across *all* screens/themes — this session audited
      the semantic color set of the default dark theme only, not every
      rendered screen, and not the light theme.
- [ ] Internal compatibility identifiers documented — partially done above
      (theme enum, opencode agent name, SpaceXAI); crate-name and
      `~/.grok/` compatibility surface is large and only summarized, not
      exhaustively enumerated.
- [ ] THIRD-PARTY-NOTICES / LICENSE correctness review — not performed this
      session; `LICENSE`'s "SpaceXAI" copyright line is flagged as a
      business decision above but the notices file itself was not audited
      line-by-line.
