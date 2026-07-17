//! `simplicio-bench`: run the golden-task corpus and print a JSON `BenchReport`.
//!
//! Usage:
//!   simplicio-bench [--repeats N] [--out PATH]
//!
//! Env:
//!   SIMPLICIO_BIN - optional path to a Simplicio Runtime binary. If unset (the
//!   common case, since no Runtime binary ships with or is built by this
//!   repo), the `runtime_attempt` path for every task will fail closed and be
//!   reported as such (not silently skipped, not faked as a success).

use simplicio_perf_bench::{
    BenchReport, GOLDEN_TASKS, LatencyStats, TaskKind, TaskResult, approx_tokens,
    generate_large_synthetic, resolve_fixture_path, time_repeats,
};
use std::path::PathBuf;

fn fixtures_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut repeats: usize = 20;
    let mut out_path: Option<PathBuf> = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--repeats" => {
                repeats = args
                    .get(i + 1)
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(repeats);
                i += 2;
            }
            "--out" => {
                out_path = args.get(i + 1).map(PathBuf::from);
                i += 2;
            }
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(2);
            }
        }
    }

    let fixtures_root = fixtures_root();
    let scratch_dir = std::env::temp_dir().join("simplicio-perf-bench-scratch");
    std::fs::create_dir_all(&scratch_dir).expect("create scratch dir");

    // Materialize the large-synthetic fixture deterministically once, outside
    // git, so the "large file" scenario is reproducible without a committed
    // multi-megabyte blob.
    let large_bytes = generate_large_synthetic(4 * 1024 * 1024);
    std::fs::write(scratch_dir.join("large_synthetic.bin"), &large_bytes)
        .expect("write synthetic large fixture");

    let mut tasks_out = Vec::new();

    for task in GOLDEN_TASKS {
        let path = resolve_fixture_path(&fixtures_root, task, &scratch_dir);
        let expect_success = !matches!(task.kind, TaskKind::InvalidPath);

        // Path A: direct read, equivalent to `LocalFs::read_file` (the code
        // path `SimplicioRuntimeFs` itself wraps for writes and that the
        // Runtime read replaces for reads).
        let (samples, results): (Vec<f64>, Vec<Result<Vec<u8>, std::io::Error>>) =
            time_repeats(repeats, || std::fs::read(&path));
        let successes = results.iter().filter(|r| r.is_ok()).count();
        let success_rate = successes as f64 / results.len() as f64;
        let last_len = results.iter().rev().find_map(|r| r.as_ref().ok()).map(Vec::len);
        let tokens = last_len.map(approx_tokens);

        tasks_out.push(TaskResult {
            id: task.id.to_string(),
            path_kind: "direct_read".to_string(),
            expected_success: expect_success,
            success_rate,
            latency: LatencyStats::from_samples(samples),
            approx_tokens: tokens,
            notes: if expect_success == (success_rate > 0.0) {
                None
            } else {
                Some("success rate did not match the expected outcome for this task".into())
            },
        });

        // Path B: attempt the real Simplicio Runtime read via the MCP client.
        // This is measured honestly: if no Runtime binary is resolvable (the
        // default in this repo, since the Runtime server is not implemented
        // here), every attempt fails closed and that is exactly what gets
        // reported.
        let repo_root = fixtures_root.clone();
        let relative = task
            .fixture_relative_path
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("does/not/exist.rs"));
        let (rt_samples, rt_results): (Vec<f64>, Vec<bool>) = time_repeats(repeats, || {
            match simplicio_runtime_client::RuntimeClient::spawn_in(&repo_root) {
                Ok(mut client) => client
                    .read_file(
                        &repo_root,
                        &relative,
                        simplicio_runtime_client::DEFAULT_MAX_FILE_BYTES,
                    )
                    .is_ok(),
                Err(_) => false,
            }
        });
        let rt_successes = rt_results.iter().filter(|ok| **ok).count();
        let rt_success_rate = rt_successes as f64 / rt_results.len().max(1) as f64;

        tasks_out.push(TaskResult {
            id: task.id.to_string(),
            path_kind: "runtime_attempt".to_string(),
            // No Runtime binary ships with (or is built by) this repo; whether this
            // succeeds depends entirely on whatever SIMPLICIO_BIN/PATH resolves to
            // on the machine running the benchmark. Do not assume either outcome.
            expected_success: expect_success,
            success_rate: rt_success_rate,
            latency: LatencyStats::from_samples(rt_samples),
            approx_tokens: None,
            notes: Some(
                "Runtime binary is an external dependency (SIMPLICIO_BIN/PATH), not built \
                 by this repo. Result depends on whatever Runtime happens to be resolvable \
                 in this environment. See docs/perf/token-latency-benchmark.md."
                    .to_string(),
            ),
        });
    }

    let report = BenchReport {
        schema: BenchReport::SCHEMA.to_string(),
        repeats,
        tasks: tasks_out,
    };

    let json = serde_json::to_string_pretty(&report).expect("serialize report");
    if let Some(path) = out_path {
        std::fs::write(&path, &json).expect("write report");
        eprintln!("wrote report to {}", path.display());
    }
    println!("{json}");
}
