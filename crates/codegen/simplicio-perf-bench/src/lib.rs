//! Benchmark harness for issue #12: measure tokens and latency of Runtime-mediated
//! file reads versus a direct filesystem read, over a small versioned corpus of
//! "golden tasks" with verifiable expected output.
//!
//! # What this measures today
//!
//! The Simplicio Runtime (the `simplicio` binary invoked via `runtime map` /
//! `serve --mcp --stdio`) is an external dependency resolved at runtime via
//! `SIMPLICIO_BIN` or `PATH` (see `simplicio-runtime-client::resolve_binary`).
//! It is **not implemented inside this repository** — only the MCP client that
//! talks to it lives here (`crates/codegen/simplicio-runtime-client`). So the
//! "Runtime cold/warm/incremental" comparison called for by issue #12 cannot be
//! executed end-to-end from this workspace alone; every `runtime_attempt` task
//! below fails closed with `Error::RuntimeNotFound` (or whatever the resolver
//! finds first), and that failure is measured and reported honestly rather than
//! faked.
//!
//! What *is* real and reproducible here: the direct-read baseline (equivalent
//! to `LocalFs::read_file`, the code path `SimplicioRuntimeFs` wraps) measured
//! over a versioned fixture corpus, plus the harness/regression-gate scaffolding
//! that a real Runtime comparison will plug into once #5/#6 land a runtime that
//! can run in CI without an external binary.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Approximate-tokens heuristic: ~4 bytes/token for English-ish source text.
/// This is *not* a real tokenizer count. It is a documented, cheap proxy so we
/// have a stable, dependency-free number to trend over time. Do not present it
/// as an exact model token count in reporting.
pub const BYTES_PER_APPROX_TOKEN: f64 = 4.0;

pub fn approx_tokens(byte_len: usize) -> f64 {
    byte_len as f64 / BYTES_PER_APPROX_TOKEN
}

/// One golden task: a file read whose expected outcome is known ahead of time,
/// so a benchmark run also doubles as a correctness check (acceptance
/// criteria: correctness gates before token/latency comparisons).
#[derive(Debug, Clone)]
pub struct GoldenTask {
    pub id: &'static str,
    pub description: &'static str,
    /// Path relative to the fixtures root, or `None` for the synthetic-large /
    /// invalid-path scenarios which are generated or intentionally absent.
    pub fixture_relative_path: Option<&'static str>,
    pub kind: TaskKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskKind {
    /// Small/medium fixture file, direct byte-for-byte comparison against a
    /// known length.
    Small,
    Medium,
    /// Deliberately nested several directories deep inside a synthetic
    /// multi-package fixture, exercising monorepo-style path resolution.
    MonorepoNested,
    /// Large file generated deterministically at bench time (not committed to
    /// the repo to avoid bloating it); expected to succeed but stress
    /// read/allocate latency.
    LargeSynthetic,
    /// A path that does not exist / escapes the fixture root ("invalid map"
    /// scenario): the correct behavior is a clean error, not a partial read.
    InvalidPath,
}

pub const GOLDEN_TASKS: &[GoldenTask] = &[
    GoldenTask {
        id: "read_small",
        description: "Read a ~1KB fixture file end to end.",
        fixture_relative_path: Some("small.rs"),
        kind: TaskKind::Small,
    },
    GoldenTask {
        id: "read_medium",
        description: "Read a ~10KB, 300-line fixture module.",
        fixture_relative_path: Some("medium.rs"),
        kind: TaskKind::Medium,
    },
    GoldenTask {
        id: "read_monorepo_nested",
        description: "Read a file 4 directories deep in a synthetic monorepo fixture.",
        fixture_relative_path: Some("monorepo/pkg-c/nested/deep/module.rs"),
        kind: TaskKind::MonorepoNested,
    },
    GoldenTask {
        id: "read_large_synthetic",
        description: "Read a deterministically-generated 4MiB file (not committed).",
        fixture_relative_path: None,
        kind: TaskKind::LargeSynthetic,
    },
    GoldenTask {
        id: "read_invalid_path",
        description: "Attempt to read a nonexistent path; must fail closed, not partially succeed.",
        fixture_relative_path: None,
        kind: TaskKind::InvalidPath,
    },
];

/// Deterministically generate the "large synthetic" fixture content so repeat
/// runs are reproducible without committing a multi-megabyte blob to git.
pub fn generate_large_synthetic(target_bytes: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(target_bytes);
    let mut state: u64 = 0x9E3779B97F4A7C15;
    while out.len() < target_bytes {
        // xorshift64* - deterministic, no external RNG dependency.
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let word = state.wrapping_mul(0x2545F4914F6CDD1D);
        out.extend_from_slice(format!("{word:016x}\n").as_bytes());
    }
    out.truncate(target_bytes);
    out
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatencyStats {
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub mean_ms: f64,
    pub stdev_ms: f64,
    pub samples: usize,
}

impl LatencyStats {
    pub fn from_samples(mut samples: Vec<f64>) -> Self {
        samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let n = samples.len().max(1);
        let percentile = |p: f64| -> f64 {
            if samples.is_empty() {
                return 0.0;
            }
            let idx = ((p * (samples.len() as f64 - 1.0)).round() as usize).min(samples.len() - 1);
            samples[idx]
        };
        let mean = samples.iter().sum::<f64>() / n as f64;
        let variance = if samples.len() > 1 {
            samples.iter().map(|s| (s - mean).powi(2)).sum::<f64>() / (samples.len() - 1) as f64
        } else {
            0.0
        };
        Self {
            p50_ms: percentile(0.50),
            p95_ms: percentile(0.95),
            mean_ms: mean,
            stdev_ms: variance.sqrt(),
            samples: samples.len(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskResult {
    pub id: String,
    pub path_kind: String,
    pub expected_success: bool,
    pub success_rate: f64,
    pub latency: LatencyStats,
    pub approx_tokens: Option<f64>,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchReport {
    pub schema: String,
    pub repeats: usize,
    pub tasks: Vec<TaskResult>,
}

impl BenchReport {
    pub const SCHEMA: &'static str = "simplicio.perf-bench-result/v1";
}

/// Time a closure `repeats` times, returning per-iteration millisecond samples.
pub fn time_repeats<T>(repeats: usize, mut f: impl FnMut() -> T) -> (Vec<f64>, Vec<T>) {
    let mut samples = Vec::with_capacity(repeats);
    let mut outputs = Vec::with_capacity(repeats);
    for _ in 0..repeats {
        let start = Instant::now();
        let out = f();
        let elapsed: Duration = start.elapsed();
        samples.push(elapsed.as_secs_f64() * 1000.0);
        outputs.push(out);
    }
    (samples, outputs)
}

/// Resolve the on-disk path for a golden task's direct-read baseline, given
/// the fixtures root and (for the large-synthetic task) a scratch dir to
/// materialize the generated file into.
pub fn resolve_fixture_path(
    fixtures_root: &Path,
    task: &GoldenTask,
    scratch_dir: &Path,
) -> PathBuf {
    match task.kind {
        TaskKind::LargeSynthetic => scratch_dir.join("large_synthetic.bin"),
        TaskKind::InvalidPath => fixtures_root.join("does/not/exist.rs"),
        _ => fixtures_root.join(task.fixture_relative_path.expect("fixture path set")),
    }
}

// ---------------------------------------------------------------------------
// Regression gate
// ---------------------------------------------------------------------------

/// A single metric regression relative to a committed baseline.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Regression {
    pub task_id: String,
    pub metric: String,
    pub baseline: f64,
    pub current: f64,
    pub pct_change: f64,
}

/// Threshold from the issue's acceptance criteria: a regression greater than
/// 10% in tokens or latency blocks the release unless waived.
pub const REGRESSION_THRESHOLD_PCT: f64 = 10.0;

/// Compare a current report to a committed baseline and return any
/// regressions. A dropped `success_rate` on any task is always reported
/// regardless of the numeric threshold, because correctness is checked before
/// token/latency wins (acceptance criteria: "nenhum ganho aceito se reduzir
/// taxa de sucesso").
pub fn find_regressions(baseline: &BenchReport, current: &BenchReport) -> Vec<Regression> {
    let mut regressions = Vec::new();
    for base_task in &baseline.tasks {
        let Some(cur_task) = current
            .tasks
            .iter()
            .find(|t| t.id == base_task.id && t.path_kind == base_task.path_kind)
        else {
            continue;
        };

        if cur_task.success_rate < base_task.success_rate {
            regressions.push(Regression {
                task_id: base_task.id.clone(),
                metric: "success_rate".to_string(),
                baseline: base_task.success_rate,
                current: cur_task.success_rate,
                pct_change: pct_change(base_task.success_rate, cur_task.success_rate),
            });
        }

        push_if_regressed(
            &mut regressions,
            &base_task.id,
            "latency_p50_ms",
            base_task.latency.p50_ms,
            cur_task.latency.p50_ms,
        );

        if let (Some(base_tokens), Some(cur_tokens)) =
            (base_task.approx_tokens, cur_task.approx_tokens)
        {
            push_if_regressed(
                &mut regressions,
                &base_task.id,
                "approx_tokens",
                base_tokens,
                cur_tokens,
            );
        }
    }
    regressions
}

fn pct_change(baseline: f64, current: f64) -> f64 {
    if baseline == 0.0 {
        if current == 0.0 { 0.0 } else { 100.0 }
    } else {
        ((current - baseline) / baseline) * 100.0
    }
}

fn push_if_regressed(
    out: &mut Vec<Regression>,
    task_id: &str,
    metric: &str,
    baseline: f64,
    current: f64,
) {
    let change = pct_change(baseline, current);
    if change > REGRESSION_THRESHOLD_PCT {
        out.push(Regression {
            task_id: task_id.to_string(),
            metric: metric.to_string(),
            baseline,
            current,
            pct_change: change,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str, path_kind: &str, p50: f64, tokens: f64, success: f64) -> TaskResult {
        TaskResult {
            id: id.to_string(),
            path_kind: path_kind.to_string(),
            expected_success: true,
            success_rate: success,
            latency: LatencyStats {
                p50_ms: p50,
                p95_ms: p50 * 1.2,
                mean_ms: p50,
                stdev_ms: 0.1,
                samples: 20,
            },
            approx_tokens: Some(tokens),
            notes: None,
        }
    }

    #[test]
    fn identical_reports_have_no_regressions() {
        let report = BenchReport {
            schema: BenchReport::SCHEMA.to_string(),
            repeats: 20,
            tasks: vec![task("read_small", "direct_read", 1.0, 250.0, 1.0)],
        };
        assert!(find_regressions(&report, &report).is_empty());
    }

    #[test]
    fn latency_regression_over_10_percent_is_flagged() {
        let baseline = BenchReport {
            schema: BenchReport::SCHEMA.to_string(),
            repeats: 20,
            tasks: vec![task("read_small", "direct_read", 1.0, 250.0, 1.0)],
        };
        let current = BenchReport {
            schema: BenchReport::SCHEMA.to_string(),
            repeats: 20,
            tasks: vec![task("read_small", "direct_read", 1.2, 250.0, 1.0)],
        };
        let regressions = find_regressions(&baseline, &current);
        assert_eq!(regressions.len(), 1);
        assert_eq!(regressions[0].metric, "latency_p50_ms");
    }

    #[test]
    fn latency_regression_under_10_percent_is_not_flagged() {
        let baseline = BenchReport {
            schema: BenchReport::SCHEMA.to_string(),
            repeats: 20,
            tasks: vec![task("read_small", "direct_read", 1.0, 250.0, 1.0)],
        };
        let current = BenchReport {
            schema: BenchReport::SCHEMA.to_string(),
            repeats: 20,
            tasks: vec![task("read_small", "direct_read", 1.05, 250.0, 1.0)],
        };
        assert!(find_regressions(&baseline, &current).is_empty());
    }

    #[test]
    fn token_regression_is_flagged_independently_of_latency() {
        let baseline = BenchReport {
            schema: BenchReport::SCHEMA.to_string(),
            repeats: 20,
            tasks: vec![task("read_medium", "direct_read", 2.0, 2500.0, 1.0)],
        };
        let current = BenchReport {
            schema: BenchReport::SCHEMA.to_string(),
            repeats: 20,
            tasks: vec![task("read_medium", "direct_read", 2.0, 3000.0, 1.0)],
        };
        let regressions = find_regressions(&baseline, &current);
        assert_eq!(regressions.len(), 1);
        assert_eq!(regressions[0].metric, "approx_tokens");
    }

    #[test]
    fn any_success_rate_drop_is_flagged_even_if_faster() {
        let baseline = BenchReport {
            schema: BenchReport::SCHEMA.to_string(),
            repeats: 20,
            tasks: vec![task("read_invalid_path", "direct_read", 5.0, 0.0, 1.0)],
        };
        let current = BenchReport {
            schema: BenchReport::SCHEMA.to_string(),
            repeats: 20,
            tasks: vec![task("read_invalid_path", "direct_read", 1.0, 0.0, 0.5)],
        };
        let regressions = find_regressions(&baseline, &current);
        assert!(regressions.iter().any(|r| r.metric == "success_rate"));
    }

    #[test]
    fn large_synthetic_fixture_is_deterministic_across_calls() {
        let a = generate_large_synthetic(4 * 1024 * 1024);
        let b = generate_large_synthetic(4 * 1024 * 1024);
        assert_eq!(a, b);
        assert_eq!(a.len(), 4 * 1024 * 1024);
    }

    #[test]
    fn golden_task_ids_are_unique() {
        let mut ids: Vec<&str> = GOLDEN_TASKS.iter().map(|t| t.id).collect();
        ids.sort();
        let mut deduped = ids.clone();
        deduped.dedup();
        assert_eq!(ids, deduped);
    }
}
