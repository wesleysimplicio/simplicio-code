//! Redaction of sensitive information from crash report text.
//!
//! Crash reports are built from debug info embedded in the binary (via the
//! `backtrace` crate) and can therefore leak information about the machine
//! that *built* or *ran* the binary: absolute filesystem paths that embed a
//! home directory and username, absolute workspace paths that reveal local
//! directory layout, environment-variable values, command-line arguments,
//! and — in the worst case — accidentally-embedded secrets (API keys,
//! tokens, "canary" values used to detect leaks).
//!
//! [`redact_report`] is the single choke point that MUST be applied to a
//! crash report's text before it is written to disk
//! (`last-crash-report.txt`), archived (`history/crash-*.txt`), or
//! displayed to the user (e.g. printed to stderr). It is called from
//! [`crate::symbolicate::format_report`], so every code path that builds a
//! report through the public API gets it for free — do not write crash
//! report text anywhere without routing it through `format_report` (or,
//! for text built by other means, calling `redact_report` directly).
//!
//! The function preserves what makes a report *useful*: relative path
//! fragments (e.g. `crates/codegen/xai-crash-handler/src/symbolicate.rs`),
//! line numbers, and symbol names. It only strips the parts of a path or
//! line that reveal identity or local machine layout.

use std::sync::OnceLock;

use regex::Regex;

/// Placeholder inserted in place of a redacted absolute path prefix.
const PATH_REDACTED: &str = "<REDACTED>";
/// Placeholder inserted in place of a redacted secret/token/env value.
const VALUE_REDACTED: &str = "<REDACTED>";

/// Path component names that anchor "the interesting, relative part" of a
/// source path. When one of these appears in an absolute path, everything
/// from that component onward is kept (joined with `/`) and everything
/// before it (which is where the home directory / username / workspace
/// checkout location lives) is replaced with [`PATH_REDACTED`].
const ANCHOR_COMPONENTS: &[&str] = &["src", "crates", "tests", "examples", "benches", "bin"];

fn path_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // A single alternation covering UNC (`\\server\share\...`),
    // drive-letter (`C:\...` / `C:/...`), and Unix (`/a/b`) absolute
    // paths, matched in one pass. This is important for two reasons:
    //
    // 1. Doing three separate sequential `replace_all` passes would let a
    //    later pass re-match the `/`-prefixed tail left behind by an
    //    earlier path's replacement (`<REDACTED>/crates/...`),
    //    double-redacting it.
    // 2. The regex crate has no lookbehind, so to avoid matching a `/`
    //    that occurs *inside* an already-relative path (e.g. the second
    //    slash in `crates/codegen/foo.rs`, which is not an absolute path
    //    at all), group 1 captures a required boundary character (start
    //    of string, whitespace, quote, or a handful of common separators)
    //    immediately before the path, and callers must re-emit it.
    RE.get_or_init(|| {
        Regex::new(r#"(^|[\s"'=:,(\[{])(\\\\[^\s"']+|[A-Za-z]:[\\/][^\s"']*|/[^\s"']*/[^\s"']*)"#)
            .unwrap()
    })
}

fn env_assignment_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // `KEY=value` style environment variable dumps, e.g. from a `KEY=value`
    // env listing embedded in a report. Requires an uppercase-led
    // SCREAMING_SNAKE_CASE key so we don't clobber unrelated `x=y` text.
    RE.get_or_init(|| Regex::new(r"\b([A-Z][A-Z0-9_]{2,})=(\S+)").unwrap())
}

fn bearer_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)\bBearer\s+[A-Za-z0-9._~+/=-]+").unwrap())
}

fn known_secret_prefix_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // Common vendor API-key / token prefixes, plus a generic "canary"
    // secret convention used in tests/fixtures to prove a leak scanner
    // works end-to-end (e.g. `canary_<random>` or `CANARY-<random>`).
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:sk|pk)-[A-Za-z0-9]{10,}\b|\bgh[pousr]_[A-Za-z0-9]{20,}\b|\bAKIA[0-9A-Z]{12,}\b|\bxox[baprs]-[A-Za-z0-9-]{10,}\b|\bcanary[_-][A-Za-z0-9]{6,}\b",
        )
        .unwrap()
    })
}

fn args_line_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // A line explicitly labelled as command-line arguments, e.g.
    // `Args: foo --token=bar /home/alice/secret`. Redacts everything after
    // the label since argv content is attacker/user controlled and may
    // contain anything.
    RE.get_or_init(|| Regex::new(r"(?im)^(\s*(?:Args|Cmd|Command)\s*:\s*).*$").unwrap())
}

/// Redact a single absolute filesystem path, keeping the relative "tail"
/// starting at the first recognized anchor component (`src`, `crates`,
/// `tests`, ...) if one is present, so line/symbol context stays useful.
/// If no anchor is found, only the final path component is kept, and only
/// when it looks like an actual filename (has a `.` extension) rather than
/// a bare directory name — a directory name with no anchor below it is
/// often the username itself (e.g. `/home/alice`), which must not survive.
fn redact_path(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let components: Vec<&str> = normalized.split('/').filter(|c| !c.is_empty()).collect();
    if components.is_empty() {
        return PATH_REDACTED.to_string();
    }

    if let Some(anchor_idx) = components
        .iter()
        .position(|c| ANCHOR_COMPONENTS.contains(&c.to_ascii_lowercase().as_str()))
    {
        let tail = components[anchor_idx..].join("/");
        return format!("{PATH_REDACTED}/{tail}");
    }

    // No recognizable anchor (e.g. a bare home-directory file with no
    // project structure below it). Only keep the final component if it
    // looks like a filename (contains a `.`); otherwise it's most likely a
    // directory name — potentially the username itself — so drop it too.
    let filename = components.last().copied().unwrap_or("");
    if !filename.contains('.') {
        return PATH_REDACTED.to_string();
    }
    format!("{PATH_REDACTED}/{filename}")
}

/// Redact all absolute paths (Windows drive-letter, UNC, and Unix) in
/// `text`, preserving relative path/line context where possible.
fn redact_paths(text: &str) -> String {
    path_re()
        .replace_all(text, |caps: &regex::Captures| {
            format!("{}{}", &caps[1], redact_path(&caps[2]))
        })
        .into_owned()
}

/// Redact `KEY=value` environment-variable-style assignments.
fn redact_env_vars(text: &str) -> String {
    env_assignment_re()
        .replace_all(text, |caps: &regex::Captures| {
            format!("{}={VALUE_REDACTED}", &caps[1])
        })
        .into_owned()
}

/// Redact known secret/token formats (vendor API key prefixes, `Bearer`
/// auth headers, and the generic `canary_*` convention used to test leak
/// scanners end-to-end).
fn redact_secrets(text: &str) -> String {
    let text = bearer_re().replace_all(text, format!("Bearer {VALUE_REDACTED}"));
    let text = known_secret_prefix_re().replace_all(&text, VALUE_REDACTED);
    text.into_owned()
}

/// Redact whole command-line-argument lines (`Args:` / `Cmd:` / `Command:`
/// labelled lines), since argv content is arbitrary and may contain
/// anything (paths, secrets, personal data).
fn redact_args(text: &str) -> String {
    args_line_re()
        .replace_all(text, |caps: &regex::Captures| {
            format!("{}{VALUE_REDACTED}", &caps[1])
        })
        .into_owned()
}

/// Redact sensitive information from crash report text.
///
/// Strips, in order:
/// 1. Command-line-argument lines (`Args:`/`Cmd:`/`Command:` labels).
/// 2. Absolute filesystem paths (Windows, UNC, and Unix forms), keeping a
///    relative tail (starting at a recognized anchor like `src/` or
///    `crates/`) so file/line context remains useful for debugging.
/// 3. `KEY=value` environment-variable assignments.
/// 4. Known secret/token formats (vendor API key prefixes, `Bearer`
///    tokens, canary secrets).
///
/// This must be applied to any text before it is written to disk,
/// archived, or displayed to the user. [`crate::symbolicate::format_report`]
/// already calls this, so reports built through the public API are safe by
/// construction; callers building report text by other means must call
/// this directly.
pub fn redact_report(text: &str) -> String {
    let text = redact_args(text);
    let text = redact_paths(&text);
    let text = redact_env_vars(&text);
    redact_secrets(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_unix_absolute_path_keeping_relative_tail() {
        let input = "at /home/alice/repos/simplicio-code/crates/codegen/xai-crash-handler/src/symbolicate.rs:42";
        let out = redact_report(input);
        assert!(!out.contains("/home/alice"), "raw home dir leaked: {out}");
        assert!(!out.contains("alice"), "username leaked: {out}");
        assert!(
            out.contains("crates/codegen/xai-crash-handler/src/symbolicate.rs:42"),
            "relative tail lost: {out}"
        );
        assert!(out.contains(PATH_REDACTED));
    }

    #[test]
    fn redacts_windows_absolute_path_keeping_relative_tail() {
        let input = r"at C:\Users\alice\repos\simplicio-code\crates\codegen\xai-crash-handler\src\symbolicate.rs:42";
        let out = redact_report(input);
        assert!(
            !out.contains(r"C:\Users\alice"),
            "raw home dir leaked: {out}"
        );
        assert!(!out.contains("alice"), "username leaked: {out}");
        assert!(
            out.contains("crates/codegen/xai-crash-handler/src/symbolicate.rs:42"),
            "relative tail lost: {out}"
        );
    }

    #[test]
    fn redacts_windows_forward_slash_path() {
        let input = "at C:/Users/alice/repos/proj/src/main.rs:1";
        let out = redact_report(input);
        assert!(!out.contains("alice"));
        assert!(out.contains("src/main.rs:1"));
    }

    #[test]
    fn redacts_unc_path() {
        let input = r"at \\build-server\share\alice\proj\src\lib.rs:7";
        let out = redact_report(input);
        assert!(!out.contains("build-server"));
        assert!(!out.contains("alice"));
        assert!(out.contains("src/lib.rs:7"));
    }

    #[test]
    fn redacts_unicode_username_unix() {
        let input = "at /home/wéslÿ日本語/repos/proj/src/main.rs:9";
        let out = redact_report(input);
        assert!(
            !out.contains("wéslÿ日本語"),
            "unicode username leaked: {out}"
        );
        assert!(out.contains("src/main.rs:9"));
    }

    #[test]
    fn redacts_unicode_username_windows() {
        let input = r"at C:\Users\日本語ユーザー\repos\proj\src\main.rs:9";
        let out = redact_report(input);
        assert!(
            !out.contains("日本語ユーザー"),
            "unicode username leaked: {out}"
        );
        assert!(out.contains("src/main.rs:9"));
    }

    #[test]
    fn redacts_nested_workspace_path() {
        // Nested workspace: checkout-of-a-checkout, worktree under a
        // username-bearing path, several directories deep before the
        // anchor component.
        let input = "at /home/bob/m/repos/wt/simplicio-code-drain-25/crates/codegen/xai-crash-handler/src/handler.rs:100";
        let out = redact_report(input);
        assert!(!out.contains("/home/bob"));
        assert!(!out.contains("simplicio-code-drain-25"));
        assert!(out.contains("crates/codegen/xai-crash-handler/src/handler.rs:100"));
    }

    #[test]
    fn redacts_cargo_registry_path_to_filename_only() {
        // Registry deps have no recognized anchor before the crate's own
        // internal `src/`, which *is* an anchor — verify we don't
        // over-redact when the anchor legitimately appears.
        let input = "at /home/alice/.cargo/registry/src/index.crates.io-abc123/backtrace-0.3.71/src/lib.rs:200";
        let out = redact_report(input);
        assert!(!out.contains("/home/alice"));
        assert!(out.contains("src/lib.rs:200"));
    }

    #[test]
    fn bare_home_directory_with_no_anchor_is_fully_redacted() {
        let input = "Env: HOME=/home/alice";
        let out = redact_report(input);
        assert!(
            !out.contains("alice"),
            "username leaked as bogus filename: {out}"
        );
        assert!(out.contains("HOME=<REDACTED>"));
    }

    #[test]
    fn windows_bare_home_directory_is_fully_redacted() {
        let input = r"at C:\Users\alice";
        let out = redact_report(input);
        assert!(
            !out.contains("alice"),
            "username leaked as bogus filename: {out}"
        );
    }

    #[test]
    fn redacts_env_var_assignment() {
        let input = "Env: HOME=/home/alice API_KEY=super-secret-value PATH=/usr/bin:/bin";
        let out = redact_report(input);
        assert!(!out.contains("super-secret-value"));
        assert!(!out.contains("/home/alice"));
        assert!(out.contains("API_KEY=<REDACTED>"));
    }

    #[test]
    fn redacts_command_args_line() {
        let input = "Args: --token=abc123 /home/alice/secret-notes.txt\nSignal: SIGSEGV";
        let out = redact_report(input);
        assert!(!out.contains("abc123"));
        assert!(!out.contains("secret-notes.txt"));
        assert!(out.contains("Signal: SIGSEGV"));
        assert!(out.starts_with("Args: <REDACTED>") || out.contains("Args: <REDACTED>"));
    }

    #[test]
    fn redacts_canary_secret() {
        let input = "leaked token canary_ABCDEF1234567890 found in report";
        let out = redact_report(input);
        assert!(!out.contains("canary_ABCDEF1234567890"));
        assert!(out.contains(VALUE_REDACTED));
    }

    #[test]
    fn redacts_known_vendor_secret_prefixes() {
        for secret in [
            "sk-abcdefghijklmnopqrstuvwx",
            "ghp_abcdefghijklmnopqrstuvwxyz012345",
            "AKIAABCDEFGHIJKLMNOP",
            "xoxb-1234567890-abcdefghij",
        ] {
            let input = format!("token={secret}");
            let out = redact_report(&input);
            assert!(!out.contains(secret), "secret leaked: {out}");
        }
    }

    #[test]
    fn redacts_bearer_token() {
        let input = "Authorization: Bearer sekrit.jwt.value-here";
        let out = redact_report(input);
        assert!(!out.contains("sekrit.jwt.value-here"));
        assert!(out.contains("Bearer <REDACTED>"));
    }

    #[test]
    fn preserves_non_sensitive_text() {
        let input = "Signal:  SIGBUS (Bus error)\nPID:     42\nVersion: 0.1.169\n  0: 0x00000000deadbeef - my_crate::main\n           at src/main.rs:42";
        let out = redact_report(input);
        assert_eq!(out, input, "non-sensitive report text should be unchanged");
    }

    #[test]
    fn relative_paths_are_not_touched() {
        let input = "at crates/codegen/xai-crash-handler/src/symbolicate.rs:10";
        let out = redact_report(input);
        assert_eq!(out, input);
    }
}
