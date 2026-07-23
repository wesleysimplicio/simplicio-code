//! `CodexGrepFilesTool` — file-path-only regex search via ripgrep.
//!
//! This is a faithful port of `codex-rs/core/src/tools/handlers/grep_files.rs`.
//! It returns **file paths only** (`--files-with-matches`), sorted by
//! modification time. See the plan document for the full diff vs the
//! grok-build `GrepTool`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tokio::time::timeout;

use crate::implementations::grok_build::grep::ripgrep::rg_path;
use crate::types::output::CodexGrepFilesOutput;
use crate::types::requirements::Expr;
#[allow(unused_imports)]
use crate::types::resources::{Cwd, SearchBackend};
use crate::types::tool::{ToolKind, ToolNamespace};

// ─── Constants ──────────────────────────────────────────────────────

const DEFAULT_LIMIT: usize = 100;
const MAX_LIMIT: usize = 2000;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

// ─── Description ────────────────────────────────────────────────────

const DESCRIPTION: &str =
    "Finds files whose contents match the pattern and lists them by modification time.";

// ─── Input ──────────────────────────────────────────────────────────

fn default_limit() -> usize {
    DEFAULT_LIMIT
}

/// Input for the codex `grep_files` tool.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CodexGrepFilesInput {
    /// Regular expression pattern to search for.
    pub pattern: String,

    /// Optional glob that limits which files are searched (e.g. "*.rs" or "*.{ts,tsx}").
    #[serde(default)]
    pub include: Option<String>,

    /// Directory or file path to search. Defaults to the session's working directory.
    #[serde(default)]
    pub path: Option<String>,

    /// Maximum number of file paths to return (defaults to 100).
    #[serde(default = "default_limit")]
    pub limit: usize,
}

// ─── Tool ───────────────────────────────────────────────────────────

/// Codex-namespace grep_files tool — file-path-only regex search.
///
/// Shares `ToolKind::Search` with the grok-build `GrepTool`. These tools are
/// namespace-exclusive — consumers enable either `GrokBuild` or `Codex` search,
/// never both simultaneously. This follows the same pattern as
/// `CodexListDirTool`/`ListDirTool` (`ToolKind::ListDir`) and
/// `CodexReadFileTool`/`ReadFileImpl` (`ToolKind::Read`).
#[derive(Debug, Default)]
pub struct CodexGrepFilesTool;

// ─── rg execution ───────────────────────────────────────────────────

/// Run `rg --files-with-matches` and return matching file paths.
///
/// Direct port from `codex-rs/core/src/tools/handlers/grep_files.rs`.
async fn run_rg_search(
    pattern: &str,
    include: Option<&str>,
    search_path: &Path,
    limit: usize,
    cwd: &Path,
) -> Result<Vec<String>, String> {
    let rg_exec = rg_path();
    let mut command = Command::new(rg_exec);
    command
        .current_dir(cwd)
        .arg("--files-with-matches")
        .arg("--sortr=modified")
        .arg("--regexp")
        .arg(pattern)
        .arg("--no-messages");

    if let Some(glob) = include {
        command.arg("--glob").arg(glob);
    }

    command.arg("--").arg(search_path);
    crate::util::detach_command(&mut command);
    command.stdin(std::process::Stdio::null());

    let output = timeout(COMMAND_TIMEOUT, command.output())
        .await
        .map_err(|_| "rg timed out after 30 seconds".to_string())?
        .map_err(|err| {
            format!("failed to launch rg: {err}. Ensure ripgrep is installed and on PATH.")
        })?;

    match output.status.code() {
        Some(0) => Ok(parse_results(&output.stdout, limit)),
        Some(1) => Ok(Vec::new()),
        _ => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(format!("rg failed: {stderr}"))
        }
    }
}

/// Parse newline-separated file paths from rg stdout.
///
/// Direct port from `codex-rs/core/src/tools/handlers/grep_files.rs`.
fn parse_results(stdout: &[u8], limit: usize) -> Vec<String> {
    let mut results = Vec::new();
    for line in stdout.split(|byte| *byte == b'\n') {
        if line.is_empty() {
            continue;
        }
        if let Ok(text) = std::str::from_utf8(line) {
            if text.is_empty() {
                continue;
            }
            results.push(text.to_string());
            if results.len() == limit {
                break;
            }
        }
    }
    results
}

// ─── Tests ──────────────────────────────────────────────────────────

impl crate::types::tool_metadata::ToolMetadata for CodexGrepFilesTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Search
    }

    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::Codex
    }

    fn description_template(&self) -> &str {
        DESCRIPTION
    }

    fn requires_expr(&self) -> Expr<crate::types::requirements::ToolRequirement> {
        Expr::True
    }
}

impl xai_tool_runtime::Tool for CodexGrepFilesTool {
    type Args = CodexGrepFilesInput;
    type Output = CodexGrepFilesOutput;

    fn id(&self) -> xai_tool_protocol::ToolId {
        xai_tool_protocol::ToolId::new("grep_files").expect("valid tool id")
    }

    fn description(
        &self,
        _ctx: &::xai_tool_runtime::ListToolsContext,
    ) -> xai_tool_types::ToolDescription {
        xai_tool_types::ToolDescription::new(
            "grep_files",
            crate::types::tool_metadata::ToolMetadata::description_template(self),
        )
    }

    fn capabilities(&self) -> xai_tool_protocol::ToolCapabilities {
        xai_tool_protocol::ToolCapabilities {
            is_read_only: true,
            tool_scope: Some(xai_tool_protocol::ToolScope::Read),
            ..Default::default()
        }
    }

    #[tracing::instrument(name = "tool.codex_grep_files", skip_all)]
    async fn run(
        &self,
        ctx: xai_tool_runtime::ToolCallContext,
        input: CodexGrepFilesInput,
    ) -> Result<CodexGrepFilesOutput, xai_tool_runtime::ToolError> {
        use crate::types::tool_metadata::shared_resources;
        let resources = shared_resources(&ctx)?;

        let cwd = crate::types::tool_metadata::resolve_cwd(&ctx, &resources).await?;

        // Validation (exact codex rules)
        let pattern = input.pattern.trim().to_string();
        if pattern.is_empty() {
            return Ok(CodexGrepFilesOutput::Error(
                "pattern must not be empty".to_string(),
            ));
        }
        if input.limit == 0 {
            return Ok(CodexGrepFilesOutput::Error(
                "limit must be greater than zero".to_string(),
            ));
        }

        let limit = input.limit.min(MAX_LIMIT);

        // Resolve search path
        let search_path = match &input.path {
            Some(p) if !p.is_empty() => {
                let p = PathBuf::from(p);
                if p.is_absolute() { p } else { cwd.join(p) }
            }
            _ => cwd.clone(),
        };

        // Clean up include glob
        let include = input.include.as_deref().map(str::trim).and_then(|v| {
            if v.is_empty() {
                None
            } else {
                Some(v.to_string())
            }
        });

        // A `SearchBackend` resource (currently: the Simplicio Runtime MCP
        // adapter, see `SimplicioRuntimeFs::search`) is only present when the
        // session was explicitly constructed with one — mirroring how
        // `SimplicioRuntimeFs` only replaces `LocalFs` at specific call
        // sites, not everywhere. When present it is used exclusively and
        // fails closed on its own errors: there is no fallback to
        // `run_rg_search` below in that branch, matching the fail-closed
        // contract `SimplicioRuntimeFs::read_file`/`write_file`/`delete_file`
        // already established. When absent, behavior is unchanged from
        // before this backend existed.
        let search_backend = resources
            .lock()
            .await
            .get::<SearchBackend>()
            .map(|b| b.0.clone());
        if let Some(backend) = search_backend {
            let globs = include.iter().cloned().collect::<Vec<_>>();
            return match backend
                .search(
                    &pattern,
                    Some(search_path.as_path()),
                    &globs,
                    false,
                    false,
                    limit,
                    limit,
                )
                .await
            {
                Ok(outcome) => {
                    let mut files: Vec<String> = Vec::new();
                    for m in outcome.matches {
                        if !files.contains(&m.path) {
                            files.push(m.path);
                        }
                        if files.len() == limit {
                            break;
                        }
                    }
                    if files.is_empty() {
                        Ok(CodexGrepFilesOutput::NoMatches(
                            "No matches found.".to_string(),
                        ))
                    } else {
                        let file_count = files.len();
                        Ok(CodexGrepFilesOutput::Matches {
                            content: files.join("\n"),
                            file_count,
                        })
                    }
                }
                Err(err) => Ok(CodexGrepFilesOutput::Error(format!(
                    "Simplicio Runtime search failed: {err}"
                ))),
            };
        }

        // Local metadata and ripgrep are retained only for explicitly local
        // test/legacy sessions. Productive Runtime sessions return above
        // before touching the workspace through tokio::fs.
        if let Err(err) = tokio::fs::metadata(&search_path).await {
            return Ok(CodexGrepFilesOutput::Error(format!(
                "unable to access `{}`: {err}",
                search_path.display()
            )));
        }

        // Run rg
        let results = run_rg_search(&pattern, include.as_deref(), &search_path, limit, &cwd).await;

        match results {
            Ok(files) if files.is_empty() => Ok(CodexGrepFilesOutput::NoMatches(
                "No matches found.".to_string(),
            )),
            Ok(files) => {
                let file_count = files.len();
                Ok(CodexGrepFilesOutput::Matches {
                    content: files.join("\n"),
                    file_count,
                })
            }
            Err(msg) => Ok(CodexGrepFilesOutput::Error(msg)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::computer::types::{AsyncSearch, ComputerError, SearchMatch, SearchOutcome};
    use crate::types::resources::Resources;
    use std::process::Command as StdCommand;
    use tempfile::TempDir;

    /// Build a runtime `ToolCallContext` with the given resources.
    fn test_ctx(cwd: &Path) -> xai_tool_runtime::ToolCallContext {
        let mut resources = Resources::new();
        resources.insert(Cwd(cwd.to_path_buf()));
        let mut ctx = xai_tool_runtime::ToolCallContext::default();
        ctx.extensions.insert(resources.into_shared());
        ctx
    }

    /// Build a `ToolCallContext` with a `SearchBackend` resource injected
    /// alongside `Cwd`, so `run()` takes the Runtime-search branch instead of
    /// shelling out to local `rg`.
    fn test_ctx_with_search_backend(
        cwd: &Path,
        backend: std::sync::Arc<dyn AsyncSearch>,
    ) -> xai_tool_runtime::ToolCallContext {
        let mut resources = Resources::new();
        resources.insert(Cwd(cwd.to_path_buf()));
        resources.insert(SearchBackend(backend));
        let mut ctx = xai_tool_runtime::ToolCallContext::default();
        ctx.extensions.insert(resources.into_shared());
        ctx
    }

    /// Test double for [`AsyncSearch`] that returns a fixed outcome (or
    /// error) and records the last call's arguments, so tests can assert
    /// both "the tool used the backend, not local rg" and "the backend saw
    /// the arguments the tool was supposed to pass".
    #[derive(Default)]
    struct FakeSearchBackend {
        outcome: std::sync::Mutex<Option<Result<SearchOutcome, String>>>,
        last_call: std::sync::Mutex<Option<(String, Option<PathBuf>, Vec<String>)>>,
    }

    impl FakeSearchBackend {
        fn with_outcome(outcome: Result<SearchOutcome, String>) -> Self {
            Self {
                outcome: std::sync::Mutex::new(Some(outcome)),
                last_call: std::sync::Mutex::new(None),
            }
        }
    }

    #[async_trait::async_trait]
    impl AsyncSearch for FakeSearchBackend {
        async fn search(
            &self,
            pattern: &str,
            path: Option<&Path>,
            globs: &[String],
            _case_insensitive: bool,
            _literal: bool,
            _max_files: usize,
            _max_matches: usize,
        ) -> Result<SearchOutcome, ComputerError> {
            *self.last_call.lock().unwrap() = Some((
                pattern.to_owned(),
                path.map(Path::to_path_buf),
                globs.to_vec(),
            ));
            match self.outcome.lock().unwrap().take() {
                Some(Ok(outcome)) => Ok(outcome),
                Some(Err(msg)) => Err(ComputerError::io(msg)),
                None => panic!("FakeSearchBackend::search called more than once in this test"),
            }
        }
    }
    fn rg_available() -> bool {
        StdCommand::new("rg")
            .arg("--version")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }

    /// Build a runtime `ToolCallContext` with the given resources.
    // ── Unit tests (parse_results) ──────────────────────────────

    #[test]
    fn parses_basic_results() {
        let stdout = b"/tmp/file_a.rs\n/tmp/file_b.rs\n";
        let parsed = parse_results(stdout, 10);
        assert_eq!(
            parsed,
            vec!["/tmp/file_a.rs".to_string(), "/tmp/file_b.rs".to_string()]
        );
    }

    #[test]
    fn parse_truncates_after_limit() {
        let stdout = b"/tmp/file_a.rs\n/tmp/file_b.rs\n/tmp/file_c.rs\n";
        let parsed = parse_results(stdout, 2);
        assert_eq!(
            parsed,
            vec!["/tmp/file_a.rs".to_string(), "/tmp/file_b.rs".to_string()]
        );
    }

    #[test]
    fn parse_skips_empty_lines() {
        let stdout = b"/tmp/file_a.rs\n\n\n/tmp/file_b.rs\n";
        let parsed = parse_results(stdout, 10);
        assert_eq!(
            parsed,
            vec!["/tmp/file_a.rs".to_string(), "/tmp/file_b.rs".to_string()]
        );
    }

    #[test]
    fn parse_returns_empty_for_empty_input() {
        let stdout = b"";
        let parsed = parse_results(stdout, 10);
        assert!(parsed.is_empty());
    }

    // ── Integration tests (run_rg_search) ───────────────────────

    #[tokio::test]
    async fn run_search_returns_results() {
        if !rg_available() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("match.rs"), "needle in haystack").unwrap();
        std::fs::write(tmp.path().join("nomatch.rs"), "just hay").unwrap();

        let results = run_rg_search("needle", None, tmp.path(), 100, tmp.path())
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].contains("match.rs"));
    }

    #[tokio::test]
    async fn run_search_with_glob_filter() {
        if !rg_available() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("alpha.rs"), "needle").unwrap();
        std::fs::write(tmp.path().join("beta.txt"), "needle").unwrap();

        let results = run_rg_search("needle", Some("*.rs"), tmp.path(), 100, tmp.path())
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].contains("alpha.rs"));
    }

    #[tokio::test]
    async fn run_search_respects_limit() {
        if !rg_available() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        for i in 0..5 {
            std::fs::write(tmp.path().join(format!("file_{i}.rs")), "needle").unwrap();
        }

        let results = run_rg_search("needle", None, tmp.path(), 2, tmp.path())
            .await
            .unwrap();
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn run_search_handles_no_matches() {
        if !rg_available() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("file.rs"), "no match here").unwrap();

        let results = run_rg_search("nonexistent_pattern_xyz", None, tmp.path(), 100, tmp.path())
            .await
            .unwrap();
        assert!(results.is_empty());
    }

    // ── Tool-level tests ────────────────────────────────────────

    #[tokio::test]
    async fn tool_reports_empty_pattern_error() {
        let tmp = TempDir::new().unwrap();
        let tool = CodexGrepFilesTool;

        let input = CodexGrepFilesInput {
            pattern: "  ".to_string(),
            include: None,
            path: None,
            limit: 100,
        };

        let result = xai_tool_runtime::Tool::run(&tool, test_ctx(tmp.path()), input)
            .await
            .unwrap();
        match result {
            CodexGrepFilesOutput::Error(msg) => {
                assert_eq!(msg, "pattern must not be empty");
            }
            other => panic!("Expected Error, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn tool_reports_zero_limit_error() {
        let tmp = TempDir::new().unwrap();
        let tool = CodexGrepFilesTool;

        let input = CodexGrepFilesInput {
            pattern: "test".to_string(),
            include: None,
            path: None,
            limit: 0,
        };

        let result = xai_tool_runtime::Tool::run(&tool, test_ctx(tmp.path()), input)
            .await
            .unwrap();
        match result {
            CodexGrepFilesOutput::Error(msg) => {
                assert_eq!(msg, "limit must be greater than zero");
            }
            other => panic!("Expected Error, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn tool_reports_no_matches() {
        if !rg_available() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("file.rs"), "nothing interesting").unwrap();

        let tool = CodexGrepFilesTool;

        let input = CodexGrepFilesInput {
            pattern: "nonexistent_pattern_xyz".to_string(),
            include: None,
            path: None,
            limit: 100,
        };

        let result = xai_tool_runtime::Tool::run(&tool, test_ctx(tmp.path()), input)
            .await
            .unwrap();
        match result {
            CodexGrepFilesOutput::NoMatches(msg) => {
                assert_eq!(msg, "No matches found.");
            }
            other => panic!("Expected NoMatches, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn tool_collects_matches() {
        if !rg_available() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("alpha.rs"), "needle here").unwrap();
        std::fs::write(tmp.path().join("beta.rs"), "needle there").unwrap();
        std::fs::write(tmp.path().join("gamma.txt"), "no match").unwrap();

        let tool = CodexGrepFilesTool;

        let input = CodexGrepFilesInput {
            pattern: "needle".to_string(),
            include: Some("*.rs".to_string()),
            path: None,
            limit: 100,
        };

        let result = xai_tool_runtime::Tool::run(&tool, test_ctx(tmp.path()), input)
            .await
            .unwrap();
        match result {
            CodexGrepFilesOutput::Matches {
                file_count,
                content,
            } => {
                assert_eq!(file_count, 2);
                assert!(content.contains("alpha.rs"));
                assert!(content.contains("beta.rs"));
                assert!(!content.contains("gamma.txt"));
            }
            other => panic!("Expected Matches, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn tool_reports_nonexistent_path_error() {
        let tmp = TempDir::new().unwrap();
        let tool = CodexGrepFilesTool;

        let input = CodexGrepFilesInput {
            pattern: "test".to_string(),
            include: None,
            path: Some("nonexistent_dir".to_string()),
            limit: 100,
        };

        let result = xai_tool_runtime::Tool::run(&tool, test_ctx(tmp.path()), input)
            .await
            .unwrap();
        match result {
            CodexGrepFilesOutput::Error(msg) => {
                assert!(
                    msg.contains("unable to access"),
                    "Expected path error, got: {msg}"
                );
            }
            other => panic!("Expected Error, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn tool_clamps_limit_to_max() {
        if !rg_available() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("file.rs"), "needle").unwrap();

        let tool = CodexGrepFilesTool;

        let input = CodexGrepFilesInput {
            pattern: "needle".to_string(),
            include: None,
            path: None,
            limit: 5000, // exceeds MAX_LIMIT (2000)
        };

        let result = xai_tool_runtime::Tool::run(&tool, test_ctx(tmp.path()), input)
            .await
            .unwrap();
        match result {
            CodexGrepFilesOutput::Matches { file_count, .. } => {
                assert_eq!(file_count, 1);
            }
            other => panic!("Expected Matches, got: {other:?}"),
        }
    }

    // ── SearchBackend (Runtime MCP search) wiring ───────────────

    /// When a `SearchBackend` resource is present, the tool must use it
    /// instead of shelling out to local `rg` — even if `rg` is installed and
    /// would have produced different results. Also asserts the backend
    /// receives a deduplicated, limit-respecting file list.
    #[tokio::test]
    async fn uses_search_backend_when_present_instead_of_local_rg() {
        let tmp = TempDir::new().unwrap();
        let backend = std::sync::Arc::new(FakeSearchBackend::with_outcome(Ok(SearchOutcome {
            matches: vec![
                SearchMatch {
                    path: "src/a.rs".into(),
                    line: 1,
                    text: "needle".into(),
                },
                SearchMatch {
                    path: "src/a.rs".into(),
                    line: 5,
                    text: "needle again".into(),
                },
                SearchMatch {
                    path: "src/b.rs".into(),
                    line: 2,
                    text: "needle".into(),
                },
            ],
            truncated: false,
        })));
        let tool = CodexGrepFilesTool;
        let input = CodexGrepFilesInput {
            pattern: "needle".to_string(),
            include: Some("*.rs".to_string()),
            path: None,
            limit: 100,
        };

        let result = xai_tool_runtime::Tool::run(
            &tool,
            test_ctx_with_search_backend(tmp.path(), backend.clone()),
            input,
        )
        .await
        .unwrap();

        match result {
            CodexGrepFilesOutput::Matches {
                file_count,
                content,
            } => {
                // Two distinct files, even though `src/a.rs` matched twice.
                assert_eq!(file_count, 2);
                assert!(content.contains("src/a.rs"));
                assert!(content.contains("src/b.rs"));
            }
            other => panic!("Expected Matches, got: {other:?}"),
        }

        let (pattern, path, globs) = backend.last_call.lock().unwrap().clone().unwrap();
        assert_eq!(pattern, "needle");
        assert_eq!(path, Some(tmp.path().to_path_buf()));
        assert_eq!(globs, vec!["*.rs".to_string()]);
    }

    /// Fail-closed regression: when the `SearchBackend` resource is present
    /// but the backend itself errors (Runtime missing/incompatible/rejected),
    /// the tool must surface that error and must NOT fall back to local
    /// `rg` — even when `rg` is installed and would find real matches on
    /// disk. This is the same fail-closed guarantee
    /// `SimplicioRuntimeFs::read_file`/`write_file`/`delete_file` already
    /// provide; this test proves the `grep_files` consumer inherits it too.
    #[tokio::test]
    async fn search_backend_failure_does_not_fall_back_to_local_rg() {
        let tmp = TempDir::new().unwrap();
        // A file that a local rg search WOULD match, proving that if the
        // tool fell back to rg after the backend error, we'd see a
        // `Matches` result instead of the expected `Error`.
        std::fs::write(tmp.path().join("real_match.rs"), "needle in a haystack").unwrap();

        let backend = std::sync::Arc::new(FakeSearchBackend::with_outcome(Err(
            "Simplicio Runtime capability 'search' is unavailable".to_string(),
        )));
        let tool = CodexGrepFilesTool;
        let input = CodexGrepFilesInput {
            pattern: "needle".to_string(),
            include: None,
            path: None,
            limit: 100,
        };

        let result = xai_tool_runtime::Tool::run(
            &tool,
            test_ctx_with_search_backend(tmp.path(), backend),
            input,
        )
        .await
        .unwrap();

        match result {
            CodexGrepFilesOutput::Error(msg) => {
                assert!(
                    msg.contains("Simplicio Runtime search failed"),
                    "unexpected error message: {msg}"
                );
                assert!(
                    msg.contains("capability 'search' is unavailable"),
                    "underlying backend error should be surfaced: {msg}"
                );
            }
            other => panic!(
                "expected a fail-closed Error, got a result that implies local rg fallback ran: {other:?}"
            ),
        }
    }

    /// Regression: with no `SearchBackend` resource at all (the default for
    /// every existing call site), behavior must be byte-for-byte unchanged
    /// from before this backend existed — local `rg` still runs.
    #[tokio::test]
    async fn falls_back_to_local_rg_when_no_search_backend_configured() {
        if !rg_available() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("match.rs"), "needle in haystack").unwrap();

        let tool = CodexGrepFilesTool;
        let input = CodexGrepFilesInput {
            pattern: "needle".to_string(),
            include: None,
            path: None,
            limit: 100,
        };

        // `test_ctx` (no `SearchBackend`) — same helper the pre-existing
        // tests above use.
        let result = xai_tool_runtime::Tool::run(&tool, test_ctx(tmp.path()), input)
            .await
            .unwrap();
        match result {
            CodexGrepFilesOutput::Matches { file_count, .. } => {
                assert_eq!(file_count, 1);
            }
            other => panic!("Expected Matches via local rg, got: {other:?}"),
        }
    }
}
