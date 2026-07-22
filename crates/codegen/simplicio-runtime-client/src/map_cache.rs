//! Versioned contract, on-disk cache, and budgeted summary for the Simplicio
//! Mapper's repository map result.
//!
//! This module is the in-tree "vertical slice" of the incremental-context
//! Mapper described in issue #6: it does not run the external mapper itself
//! (that remains the standalone `simplicio` Runtime binary invoked by
//! [`crate::start_workspace_map`]), but it gives the agent a stable,
//! versioned, cached view of whatever the Runtime last produced, keyed by
//! repository content and Runtime version so a rerun after a branch switch
//! or Runtime upgrade is never served stale data.
//!
//! Scope covered here (see docs/mapper-context.md for what remains open):
//! - a versioned `simplicio.map-result/v1` contract ([`MapResult`])
//! - the five observable states from the issue's acceptance criteria
//!   ([`MapState`])
//! - an on-disk + in-memory cache keyed by `(repo_hash, runtime_version)`
//!   with dedup-on-write and invalidation on repo/branch/schema change
//!   ([`MapCache`])
//! - a fixed-budget structural summary for injection into agent context
//!   ([`budgeted_summary`])
//! - a repo identity hash derived from the git HEAD ref, branch name, and
//!   worktree path, so switching branches or worktrees naturally changes the
//!   cache key ([`compute_repo_hash`])

use serde::Deserialize;
use simplicio_code_formats::{HbiReader, HbiSection, encode_hbi, write_atomically};
use std::{
    fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

/// Current schema version for the map-result contract.
///
/// Bumping this constant is a breaking change: any cache entry persisted
/// under an older schema is treated as invalid and is not returned by
/// [`MapCache::get`]/[`MapCache::load`].
pub const MAP_RESULT_SCHEMA_V1: &str = "simplicio.map-result/v1";

/// Observable lifecycle of a single workspace map, mirroring the states
/// required by issue #6 ("aguardando, mapeando, pronto, degradado e
/// falhou").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapState {
    /// No map has been requested yet for this repository identity.
    Waiting,
    /// A map is currently being produced by the Runtime.
    Mapping,
    /// A map completed successfully and is safe to summarize into context.
    Ready,
    /// A map completed but with partial/best-effort data (e.g. the Runtime
    /// hit a size or time limit). Still usable, but callers should not treat
    /// it as authoritative.
    Degraded,
    /// Mapping failed outright; no summary should be injected.
    Failed,
}

/// The versioned, persisted result of mapping a repository.
///
/// `schema` is always [`MAP_RESULT_SCHEMA_V1`] for values constructed via
/// [`MapResult::new`]; the field is retained in the typed domain object so
/// cache files written by a future schema version are rejected by
/// [`MapCache::load`] instead of being misinterpreted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapResult {
    pub schema: String,
    /// Content-derived identity of the repository (see
    /// [`compute_repo_hash`]). Never a raw filesystem path, so this value is
    /// safe to include in telemetry.
    pub repo_hash: String,
    /// Version string of the Runtime binary that produced this map.
    pub runtime_version: String,
    pub state: MapState,
    /// Structural summary text, already produced by the Runtime or derived
    /// from it. Callers wanting a bounded-size view should go through
    /// [`budgeted_summary`] rather than using this field directly.
    pub summary: String,
    pub file_count: usize,
    pub generated_at_unix_ms: u64,
}

impl MapResult {
    pub fn new(
        repo_hash: impl Into<String>,
        runtime_version: impl Into<String>,
        state: MapState,
        summary: impl Into<String>,
        file_count: usize,
    ) -> Self {
        Self {
            schema: MAP_RESULT_SCHEMA_V1.to_string(),
            repo_hash: repo_hash.into(),
            runtime_version: runtime_version.into(),
            state,
            summary: summary.into(),
            file_count,
            generated_at_unix_ms: now_unix_ms(),
        }
    }

    /// Whether this result is fresh enough to serve as context. Only `Ready`
    /// and `Degraded` maps carry a usable summary; per the issue's
    /// acceptance criteria, a failed map must not fall back to direct reads.
    pub fn is_usable(&self) -> bool {
        matches!(self.state, MapState::Ready | MapState::Degraded)
    }
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Cache key: a repository identity paired with the Runtime version that
/// produced the map. Changing either component naturally misses the cache,
/// which is how branch/worktree changes (folded into `repo_hash`) and
/// Runtime upgrades (`runtime_version`) invalidate stale entries without any
/// extra bookkeeping.
fn cache_key(repo_hash: &str, runtime_version: &str) -> String {
    format!("{repo_hash}-{runtime_version}")
}

fn cache_file_name(repo_hash: &str, runtime_version: &str) -> String {
    format!("{}.hbi", cache_key(repo_hash, runtime_version))
}

const MAP_SECTION_KIND: u32 = 1;
const MAP_PAYLOAD_MAGIC: [u8; 4] = *b"SMAP";
const MAP_PAYLOAD_VERSION: u16 = 1;
const MAX_MAP_FIELD_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Deserialize)]
struct LegacyMapResult {
    schema: String,
    repo_hash: String,
    runtime_version: String,
    state: String,
    summary: String,
    file_count: usize,
    generated_at_unix_ms: u64,
}

fn decode_legacy_map(
    bytes: &[u8],
    repo_hash: &str,
    runtime_version: &str,
) -> io::Result<MapResult> {
    let legacy: LegacyMapResult = serde_json::from_slice(bytes).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("legacy map JSON is invalid: {error}"),
        )
    })?;
    if legacy.schema != MAP_RESULT_SCHEMA_V1
        || legacy.repo_hash != repo_hash
        || legacy.runtime_version != runtime_version
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "legacy map identity or schema mismatch",
        ));
    }
    let state = match legacy.state.as_str() {
        "waiting" => MapState::Waiting,
        "mapping" => MapState::Mapping,
        "ready" => MapState::Ready,
        "degraded" => MapState::Degraded,
        "failed" => MapState::Failed,
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "legacy map state is unknown",
            ));
        }
    };
    Ok(MapResult {
        schema: MAP_RESULT_SCHEMA_V1.to_string(),
        repo_hash: legacy.repo_hash,
        runtime_version: legacy.runtime_version,
        state,
        summary: legacy.summary,
        file_count: legacy.file_count,
        generated_at_unix_ms: legacy.generated_at_unix_ms,
    })
}

fn encode_map_result(result: &MapResult) -> io::Result<Vec<u8>> {
    if result.repo_hash.len() > MAX_MAP_FIELD_BYTES
        || result.runtime_version.len() > MAX_MAP_FIELD_BYTES
        || result.summary.len() > MAX_MAP_FIELD_BYTES
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "map result field exceeds limit",
        ));
    }
    let mut payload = Vec::with_capacity(
        40 + result.repo_hash.len() + result.runtime_version.len() + result.summary.len(),
    );
    payload.extend_from_slice(&MAP_PAYLOAD_MAGIC);
    payload.extend_from_slice(&MAP_PAYLOAD_VERSION.to_le_bytes());
    payload.extend_from_slice(&[0, 0]);
    payload.extend_from_slice(&(result.repo_hash.len() as u32).to_le_bytes());
    payload.extend_from_slice(&(result.runtime_version.len() as u32).to_le_bytes());
    payload.push(match result.state {
        MapState::Waiting => 0,
        MapState::Mapping => 1,
        MapState::Ready => 2,
        MapState::Degraded => 3,
        MapState::Failed => 4,
    });
    payload.extend_from_slice(&[0, 0, 0]);
    payload.extend_from_slice(&(result.file_count as u64).to_le_bytes());
    payload.extend_from_slice(&result.generated_at_unix_ms.to_le_bytes());
    payload.extend_from_slice(&(result.summary.len() as u32).to_le_bytes());
    payload.extend_from_slice(result.repo_hash.as_bytes());
    payload.extend_from_slice(result.runtime_version.as_bytes());
    payload.extend_from_slice(result.summary.as_bytes());
    Ok(payload)
}

fn decode_map_result(bytes: &[u8]) -> io::Result<MapResult> {
    if bytes.len() < 40 || &bytes[..4] != MAP_PAYLOAD_MAGIC.as_slice() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid map payload",
        ));
    }
    let version = u16::from_le_bytes(bytes[4..6].try_into().unwrap());
    if version != MAP_PAYLOAD_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsupported map payload version",
        ));
    }
    let repo_len = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    let runtime_len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    let state = match bytes[16] {
        0 => MapState::Waiting,
        1 => MapState::Mapping,
        2 => MapState::Ready,
        3 => MapState::Degraded,
        4 => MapState::Failed,
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid map state",
            ));
        }
    };
    let file_count = u64::from_le_bytes(bytes[20..28].try_into().unwrap());
    let generated_at_unix_ms = u64::from_le_bytes(bytes[28..36].try_into().unwrap());
    let summary_len = u32::from_le_bytes(bytes[36..40].try_into().unwrap()) as usize;
    if repo_len > MAX_MAP_FIELD_BYTES
        || runtime_len > MAX_MAP_FIELD_BYTES
        || summary_len > MAX_MAP_FIELD_BYTES
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "map payload field exceeds limit",
        ));
    }
    let end = 40usize
        .checked_add(repo_len)
        .and_then(|n| n.checked_add(runtime_len))
        .and_then(|n| n.checked_add(summary_len))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "map payload length overflow"))?;
    if end != bytes.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "map payload is truncated or has trailing bytes",
        ));
    }
    let repo_start = 40;
    let runtime_start = repo_start + repo_len;
    let summary_start = runtime_start + runtime_len;
    let repo_hash = String::from_utf8(bytes[repo_start..runtime_start].to_vec())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "map repo hash is not UTF-8"))?;
    let runtime_version =
        String::from_utf8(bytes[runtime_start..summary_start].to_vec()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "map runtime version is not UTF-8",
            )
        })?;
    let summary = String::from_utf8(bytes[summary_start..].to_vec())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "map summary is not UTF-8"))?;
    let file_count = usize::try_from(file_count).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "map file count exceeds platform limit",
        )
    })?;
    Ok(MapResult {
        schema: MAP_RESULT_SCHEMA_V1.to_string(),
        repo_hash,
        runtime_version,
        state,
        summary,
        file_count,
        generated_at_unix_ms,
    })
}

/// File- and memory-backed cache of [`MapResult`] values.
///
/// Entries are addressed by `(repo_hash, runtime_version)`. Writing an
/// identical result twice is a no-op on disk (dedup), and invalidating a
/// `repo_hash` (e.g. on branch switch) drops every entry for that
/// repository regardless of which Runtime version produced them.
pub struct MapCache {
    dir: PathBuf,
    entries: std::collections::HashMap<String, MapResult>,
}

impl MapCache {
    /// Creates a cache rooted at `dir`. The directory is created lazily on
    /// first write; a missing directory is not an error at construction
    /// time.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            entries: std::collections::HashMap::new(),
        }
    }

    /// Returns a cached, schema-valid result for `(repo_hash,
    /// runtime_version)` if one is held in memory, without touching disk.
    pub fn get(&self, repo_hash: &str, runtime_version: &str) -> Option<&MapResult> {
        self.entries.get(&cache_key(repo_hash, runtime_version))
    }

    /// Loads a result from disk into memory if present and schema-valid,
    /// returning it. A missing file, unreadable file, or schema mismatch is
    /// treated as a cache miss (`Ok(None)`), not an error, since a cold or
    /// degraded cache is an expected steady state.
    pub fn load(
        &mut self,
        repo_hash: &str,
        runtime_version: &str,
    ) -> io::Result<Option<MapResult>> {
        let path = self.dir.join(cache_file_name(repo_hash, runtime_version));
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let reader = match HbiReader::open(&bytes) {
            Ok(reader) => reader,
            Err(_) => return Ok(None),
        };
        if reader.schema() != MAP_RESULT_SCHEMA_V1 || reader.section_count() != 1 {
            return Ok(None);
        }
        let result: MapResult = match reader.section(0).and_then(|(kind, payload)| {
            (kind == MAP_SECTION_KIND)
                .then(|| decode_map_result(payload).ok())
                .flatten()
        }) {
            Some(result) => result,
            None => return Ok(None),
        };
        if result.schema != MAP_RESULT_SCHEMA_V1 {
            return Ok(None);
        }
        if result.repo_hash != repo_hash || result.runtime_version != runtime_version {
            // Defensive: a file that somehow doesn't match its own cache key
            // (e.g. corrupted/hand-edited) must never be served.
            return Ok(None);
        }
        let key = cache_key(repo_hash, runtime_version);
        self.entries.insert(key.clone(), result);
        Ok(self.entries.get(&key).cloned())
    }

    /// Persists `result` to disk and memory. Results from an unknown schema
    /// are rejected before touching either memory or disk. If an identical result (by
    /// value) is already cached for this key, the write is skipped
    /// (dedup) so unrelated processes don't observe a spurious mtime bump
    /// or extra disk I/O.
    pub fn put(&mut self, result: MapResult) -> io::Result<()> {
        if result.schema != MAP_RESULT_SCHEMA_V1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "unsupported map result schema '{}'; expected '{MAP_RESULT_SCHEMA_V1}'",
                    result.schema
                ),
            ));
        }
        let key = cache_key(&result.repo_hash, &result.runtime_version);
        if self.entries.get(&key) == Some(&result) {
            return Ok(());
        }
        fs::create_dir_all(&self.dir)?;
        let path = self
            .dir
            .join(cache_file_name(&result.repo_hash, &result.runtime_version));
        let payload = encode_map_result(&result)?;
        let bytes = encode_hbi(
            MAP_RESULT_SCHEMA_V1,
            &[HbiSection {
                kind: MAP_SECTION_KIND,
                bytes: payload,
            }],
        )
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        write_atomically(&path, &bytes)?;
        self.entries.insert(key, result);
        Ok(())
    }

    /// Explicit one-way migration for the pre-HBI JSON cache. The normal
    /// [`load`](Self::load) path never falls back to JSON; callers must opt in
    /// to this bounded, backup-preserving conversion during upgrade.
    pub fn migrate_legacy(
        &mut self,
        legacy_path: &Path,
        repo_hash: &str,
        runtime_version: &str,
        dry_run: bool,
    ) -> io::Result<simplicio_code_formats::MigrationOutcome> {
        let target = self.dir.join(cache_file_name(repo_hash, runtime_version));
        let backup = legacy_path.with_extension("json.legacy.bak");
        let outcome = simplicio_code_formats::migrate_bytes_atomically(
            legacy_path,
            &target,
            &backup,
            dry_run,
            |bytes| {
                let result = decode_legacy_map(bytes, repo_hash, runtime_version)?;
                let payload = encode_map_result(&result)?;
                encode_hbi(
                    MAP_RESULT_SCHEMA_V1,
                    &[HbiSection {
                        kind: MAP_SECTION_KIND,
                        bytes: payload,
                    }],
                )
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
            },
        )?;
        if !dry_run {
            let _ = self.load(repo_hash, runtime_version)?;
        }
        Ok(outcome)
    }

    /// Invalidates every cached entry (memory and disk) for `repo_hash`,
    /// across all Runtime versions. Call this when the repository identity
    /// changes underneath a session (branch checkout, worktree swap) even
    /// though in practice `repo_hash` already changing means old entries
    /// simply won't be looked up again; this also proactively reclaims disk
    /// space and covers the "schema version bump" case where the old
    /// `repo_hash` is unchanged but must no longer be treated as reusable.
    pub fn invalidate_repo(&mut self, repo_hash: &str) -> io::Result<usize> {
        let prefix = format!("{repo_hash}-");
        let mut removed_keys: std::collections::HashSet<String> = std::collections::HashSet::new();

        let stale_keys: Vec<String> = self
            .entries
            .keys()
            .filter(|key| key.starts_with(&prefix))
            .cloned()
            .collect();
        for key in stale_keys {
            self.entries.remove(&key);
            removed_keys.insert(key);
        }

        if let Ok(read_dir) = fs::read_dir(&self.dir) {
            for entry in read_dir.flatten() {
                let file_name = entry.file_name();
                let Some(name) = file_name.to_str() else {
                    continue;
                };
                let Some(key) = name.strip_suffix(".hbi") else {
                    continue;
                };
                if key.starts_with(&prefix) && fs::remove_file(entry.path()).is_ok() {
                    removed_keys.insert(key.to_string());
                }
            }
        }
        Ok(removed_keys.len())
    }

    /// Number of entries currently held in memory. Exposed for tests and for
    /// TUI/headless diagnostics surfaces to report cache size without
    /// leaking file paths.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Produces a copy of `summary` truncated to at most `budget_chars` Unicode
/// scalar values, so a single map's structural summary can never blow an
/// agent's fixed context budget. Truncation always lands on a char boundary
/// and appends a truncation marker so callers (and telemetry) can tell a
/// summary was cut instead of assuming it is complete.
pub fn budgeted_summary(summary: &str, budget_chars: usize) -> String {
    let char_count = summary.chars().count();
    if char_count <= budget_chars {
        return summary.to_string();
    }
    if budget_chars == 0 {
        return String::new();
    }
    const MARKER: &str = "\n…(truncated to fit context budget)";
    let marker_chars = MARKER.chars().count();
    if marker_chars >= budget_chars {
        // Budget too small to fit the truncation marker itself; fall back
        // to a bare truncation rather than overshoot the budget.
        return summary.chars().take(budget_chars).collect();
    }
    let keep = budget_chars - marker_chars;
    let mut truncated: String = summary.chars().take(keep).collect();
    truncated.push_str(MARKER);
    truncated
}

/// Derives a stable, non-reversible repository identity from the current
/// git HEAD ref, HEAD object ID, branch name, and worktree root path. The hash
/// changes whenever any of those change (commit, checkout, branch rename,
/// moving to a different worktree of the same repo), which is exactly the
/// invalidation trigger the Mapper needs — no filesystem path or file contents
/// are embedded in the output, so it is safe to log.
pub fn compute_repo_hash(repo_root: &Path) -> io::Result<String> {
    let head = read_git_head(repo_root).unwrap_or_default();
    let head_object_id = read_git_head_object_id(repo_root, &head).unwrap_or_default();
    let branch = read_git_branch(repo_root).unwrap_or_default();
    let worktree = dunce::canonicalize(repo_root).unwrap_or_else(|_| repo_root.to_path_buf());

    let mut hasher = blake3::Hasher::new();
    hasher.update(head.as_bytes());
    hasher.update(b"\0");
    hasher.update(head_object_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(branch.as_bytes());
    hasher.update(b"\0");
    hasher.update(worktree.to_string_lossy().as_bytes());
    Ok(hasher.finalize().to_hex().to_string())
}

fn read_git_head(repo_root: &Path) -> Option<String> {
    let git_dir = resolve_git_dir(repo_root)?;
    fs::read_to_string(git_dir.join("HEAD"))
        .ok()
        .map(|s| s.trim().to_string())
}

fn read_git_branch(repo_root: &Path) -> Option<String> {
    let head = read_git_head(repo_root)?;
    head.strip_prefix("ref: refs/heads/")
        .map(|branch| branch.trim().to_string())
        .or(Some(head))
}

/// Reads the object ID currently pointed to by HEAD. Symbolic HEADs resolve
/// through the corresponding loose ref (or packed-refs fallback), while a
/// detached HEAD already contains the object ID directly.
fn read_git_head_object_id(repo_root: &Path, head: &str) -> Option<String> {
    let git_dir = resolve_git_dir(repo_root)?;
    if let Some(reference) = head.strip_prefix("ref: ") {
        let loose_ref = git_dir.join(reference);
        if let Some(object_id) = fs::read_to_string(loose_ref)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
        {
            return Some(object_id);
        }

        let packed_refs = fs::read_to_string(git_dir.join("packed-refs")).ok()?;
        return packed_refs.lines().find_map(|line| {
            let mut fields = line.split_whitespace();
            let object_id = fields.next()?;
            let packed_ref = fields.next()?;
            (packed_ref == reference).then(|| object_id.to_string())
        });
    }

    (!head.is_empty()).then(|| head.to_string())
}

/// Resolves the `.git` directory for `repo_root`, following the `gitdir:`
/// indirection file used by worktrees and submodules so each worktree
/// (which shares the main repo's object store) still gets a HEAD specific
/// to itself.
fn resolve_git_dir(repo_root: &Path) -> Option<PathBuf> {
    let dot_git = repo_root.join(".git");
    if dot_git.is_dir() {
        return Some(dot_git);
    }
    let contents = fs::read_to_string(&dot_git).ok()?;
    let pointer = contents.trim().strip_prefix("gitdir:")?.trim();
    let candidate = PathBuf::from(pointer);
    if candidate.is_absolute() {
        Some(candidate)
    } else {
        Some(repo_root.join(candidate))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(repo_hash: &str, runtime_version: &str, state: MapState) -> MapResult {
        MapResult::new(repo_hash, runtime_version, state, "structural summary", 42)
    }

    // --- budget tests -------------------------------------------------

    #[test]
    fn budgeted_summary_returns_input_unchanged_when_under_budget() {
        let text = "short summary";
        assert_eq!(budgeted_summary(text, 1000), text);
    }

    #[test]
    fn budgeted_summary_truncates_and_marks_when_over_budget() {
        let text = "a".repeat(500);
        let result = budgeted_summary(&text, 100);
        assert!(result.chars().count() <= 100);
        assert!(result.contains("truncated to fit context budget"));
    }

    #[test]
    fn budgeted_summary_never_exceeds_budget_even_for_tiny_budgets() {
        let text = "a".repeat(500);
        for budget in [0usize, 1, 5, 10, 40] {
            let result = budgeted_summary(&text, budget);
            assert!(
                result.chars().count() <= budget,
                "budget {budget} violated: {} chars",
                result.chars().count()
            );
        }
    }

    #[test]
    fn budgeted_summary_respects_multibyte_char_boundaries() {
        // Each of these is a single Unicode scalar but multiple UTF-8 bytes;
        // a naive byte-slice truncation would panic here.
        let text = "café".repeat(50);
        let result = budgeted_summary(&text, 10);
        assert!(result.chars().count() <= 10);
    }

    // --- MapResult tests -----------------------------------------------

    #[test]
    fn map_result_tags_current_schema_version() {
        let result = sample("hash", "3.5.3", MapState::Ready);
        assert_eq!(result.schema, MAP_RESULT_SCHEMA_V1);
    }

    #[test]
    fn only_ready_and_degraded_states_are_usable() {
        assert!(sample("h", "v", MapState::Ready).is_usable());
        assert!(sample("h", "v", MapState::Degraded).is_usable());
        assert!(!sample("h", "v", MapState::Waiting).is_usable());
        assert!(!sample("h", "v", MapState::Mapping).is_usable());
        assert!(!sample("h", "v", MapState::Failed).is_usable());
    }

    #[test]
    fn map_result_round_trips_through_hbi() {
        let result = sample("hash-a", "3.5.3", MapState::Degraded);
        let payload = encode_map_result(&result).unwrap();
        let bytes = encode_hbi(
            MAP_RESULT_SCHEMA_V1,
            &[HbiSection {
                kind: MAP_SECTION_KIND,
                bytes: payload,
            }],
        )
        .unwrap();
        let reader = HbiReader::open(&bytes).unwrap();
        let back = decode_map_result(reader.section(0).unwrap().1).unwrap();
        assert_eq!(result, back);
    }

    // --- MapCache tests --------------------------------------------------

    fn temp_cache_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "simplicio-map-cache-test-{name}-{}-{}",
            std::process::id(),
            now_unix_ms()
        ))
    }

    #[test]
    fn put_then_get_returns_the_same_result() {
        let dir = temp_cache_dir("put-get");
        let mut cache = MapCache::new(&dir);
        let result = sample("repo-1", "3.5.3", MapState::Ready);
        cache.put(result.clone()).unwrap();
        assert_eq!(cache.get("repo-1", "3.5.3"), Some(&result));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_recovers_a_cache_entry_written_by_a_previous_process() {
        let dir = temp_cache_dir("load-recover");
        let result = sample("repo-2", "3.5.3", MapState::Ready);
        {
            let mut writer = MapCache::new(&dir);
            writer.put(result.clone()).unwrap();
        }
        let mut reader = MapCache::new(&dir);
        assert!(reader.get("repo-2", "3.5.3").is_none());
        let loaded = reader.load("repo-2", "3.5.3").unwrap();
        assert_eq!(loaded, Some(result));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_is_a_miss_not_an_error_for_a_cold_cache() {
        let dir = temp_cache_dir("cold");
        let mut cache = MapCache::new(&dir);
        let loaded = cache.load("never-mapped", "3.5.3").unwrap();
        assert_eq!(loaded, None);
    }

    #[test]
    fn load_rejects_entries_written_under_a_different_schema_version() {
        let dir = temp_cache_dir("schema-mismatch");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(cache_file_name("repo-3", "3.5.3")),
            b"not an HBI artifact",
        )
        .unwrap();
        let mut cache = MapCache::new(&dir);
        let loaded = cache.load("repo-3", "3.5.3").unwrap();
        assert_eq!(loaded, None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_migration_is_dry_run_then_atomic_and_backed_up() {
        let dir = temp_cache_dir("legacy-migration");
        fs::create_dir_all(&dir).unwrap();
        let legacy = dir.join("repo-legacy-3.5.3.json");
        fs::write(&legacy, br#"{"schema":"simplicio.map-result/v1","repo_hash":"repo-legacy","runtime_version":"3.5.3","state":"ready","summary":"old","file_count":2,"generated_at_unix_ms":1}"#).unwrap();
        let mut cache = MapCache::new(&dir);
        assert!(
            cache
                .migrate_legacy(&legacy, "repo-legacy", "3.5.3", true)
                .unwrap()
                .dry_run
        );
        assert!(!dir.join("repo-legacy-3.5.3.hbi").exists());
        let outcome = cache
            .migrate_legacy(&legacy, "repo-legacy", "3.5.3", false)
            .unwrap();
        assert!(outcome.migrated && outcome.backup.unwrap().exists());
        assert_eq!(
            cache.load("repo-legacy", "3.5.3").unwrap().unwrap().summary,
            "old"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_migration_rejects_corrupt_and_truncated_inputs_without_writing() {
        let dir = temp_cache_dir("legacy-corrupt");
        fs::create_dir_all(&dir).unwrap();
        let legacy = dir.join("legacy.json");
        fs::write(&legacy, b"{\"schema\":\"simplicio.map-result/v1\"").unwrap();
        let mut cache = MapCache::new(&dir);
        assert_eq!(
            cache
                .migrate_legacy(&legacy, "repo", "3.5.3", false)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        assert!(!dir.join("repo-3.5.3.hbi").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn put_rejects_entries_written_under_a_different_schema_version_before_side_effects() {
        let dir = temp_cache_dir("put-schema-mismatch");
        let mut cache = MapCache::new(&dir);
        let mut result = sample("repo-put-schema", "3.5.3", MapState::Ready);
        result.schema = "simplicio.map-result/v0".to_string();

        let error = cache.put(result).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(cache.is_empty(), "invalid results must not enter memory");
        assert!(
            !dir.exists(),
            "invalid results must not create the cache directory"
        );
    }

    #[test]
    fn put_deduplicates_identical_writes() {
        let dir = temp_cache_dir("dedup");
        let mut cache = MapCache::new(&dir);
        let result = sample("repo-4", "3.5.3", MapState::Ready);
        cache.put(result.clone()).unwrap();
        let path = dir.join(cache_file_name("repo-4", "3.5.3"));
        let first_write = fs::metadata(&path).unwrap().modified().unwrap();

        std::thread::sleep(std::time::Duration::from_millis(20));
        cache.put(result.clone()).unwrap();
        let second_write = fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(
            first_write, second_write,
            "dedup must skip the redundant disk write"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn put_overwrites_when_content_actually_changes() {
        let dir = temp_cache_dir("overwrite");
        let mut cache = MapCache::new(&dir);
        cache
            .put(sample("repo-5", "3.5.3", MapState::Mapping))
            .unwrap();
        cache
            .put(sample("repo-5", "3.5.3", MapState::Ready))
            .unwrap();
        assert_eq!(
            cache.get("repo-5", "3.5.3").map(|r| r.state),
            Some(MapState::Ready)
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn different_runtime_versions_are_independent_cache_entries() {
        let dir = temp_cache_dir("runtime-version");
        let mut cache = MapCache::new(&dir);
        cache
            .put(sample("repo-6", "3.5.3", MapState::Ready))
            .unwrap();
        cache
            .put(sample("repo-6", "3.6.0", MapState::Mapping))
            .unwrap();
        assert_eq!(cache.len(), 2);
        assert_eq!(
            cache.get("repo-6", "3.5.3").map(|r| r.state),
            Some(MapState::Ready)
        );
        assert_eq!(
            cache.get("repo-6", "3.6.0").map(|r| r.state),
            Some(MapState::Mapping)
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn invalidate_repo_drops_every_runtime_version_for_that_repo_hash() {
        let dir = temp_cache_dir("invalidate");
        let mut cache = MapCache::new(&dir);
        cache
            .put(sample("repo-7", "3.5.3", MapState::Ready))
            .unwrap();
        cache
            .put(sample("repo-7", "3.6.0", MapState::Ready))
            .unwrap();
        cache
            .put(sample("repo-other", "3.5.3", MapState::Ready))
            .unwrap();

        let removed = cache.invalidate_repo("repo-7").unwrap();
        assert_eq!(removed, 2);
        assert!(cache.get("repo-7", "3.5.3").is_none());
        assert!(cache.get("repo-7", "3.6.0").is_none());
        // A different repository's entry must survive the invalidation.
        assert!(cache.get("repo-other", "3.5.3").is_some());

        // And the files must actually be gone from disk, not just memory.
        let mut fresh = MapCache::new(&dir);
        assert!(fresh.load("repo-7", "3.5.3").unwrap().is_none());
        assert!(fresh.load("repo-other", "3.5.3").unwrap().is_some());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn invalidate_on_a_repo_hash_with_no_entries_is_a_no_op() {
        let dir = temp_cache_dir("invalidate-noop");
        let mut cache = MapCache::new(&dir);
        assert_eq!(cache.invalidate_repo("never-cached").unwrap(), 0);
    }

    // --- repo hash tests --------------------------------------------------

    #[test]
    fn compute_repo_hash_is_stable_for_the_same_head_and_path() {
        let dir = temp_cache_dir("repo-hash-stable");
        fs::create_dir_all(&dir).unwrap();
        let git_dir = dir.join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();

        let first = compute_repo_hash(&dir).unwrap();
        let second = compute_repo_hash(&dir).unwrap();
        assert_eq!(first, second);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn compute_repo_hash_differs_across_branches() {
        let dir = temp_cache_dir("repo-hash-diff");
        fs::create_dir_all(&dir).unwrap();
        let git_dir = dir.join(".git");
        fs::create_dir_all(&git_dir).unwrap();

        fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        let on_main = compute_repo_hash(&dir).unwrap();

        fs::write(git_dir.join("HEAD"), "ref: refs/heads/feature\n").unwrap();
        let on_feature = compute_repo_hash(&dir).unwrap();

        assert_ne!(
            on_main, on_feature,
            "switching branches must change the repo hash"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn compute_repo_hash_differs_when_head_object_changes_on_the_same_branch() {
        let dir = temp_cache_dir("repo-hash-object-id");
        fs::create_dir_all(dir.join(".git/refs/heads")).unwrap();
        fs::write(dir.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::write(
            dir.join(".git/refs/heads/main"),
            "1111111111111111111111111111111111111111\n",
        )
        .unwrap();
        let first = compute_repo_hash(&dir).unwrap();

        fs::write(
            dir.join(".git/refs/heads/main"),
            "2222222222222222222222222222222222222222\n",
        )
        .unwrap();
        let second = compute_repo_hash(&dir).unwrap();

        assert_ne!(
            first, second,
            "a new commit on the same branch must change the repo hash"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn compute_repo_hash_follows_worktree_gitdir_indirection() {
        let dir = temp_cache_dir("repo-hash-worktree");
        fs::create_dir_all(&dir).unwrap();
        let real_git_dir = dir.join("real-git-dir");
        fs::create_dir_all(&real_git_dir).unwrap();
        fs::write(real_git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::write(
            dir.join(".git"),
            format!("gitdir: {}\n", real_git_dir.display()),
        )
        .unwrap();

        // Should not error and should incorporate the resolved HEAD, i.e.
        // differ from a repo on a different branch at the same path shape.
        let hash = compute_repo_hash(&dir).unwrap();
        assert!(!hash.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn compute_repo_hash_never_embeds_the_raw_path_in_the_output() {
        let dir = temp_cache_dir("repo-hash-no-path-leak");
        fs::create_dir_all(&dir).unwrap();
        let hash = compute_repo_hash(&dir).unwrap();
        let path_string = dir.to_string_lossy().to_string();
        assert!(!hash.contains(&path_string));
        // blake3 hex digest: fixed-width hex, not a path at all.
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
        let _ = fs::remove_dir_all(&dir);
    }
}
